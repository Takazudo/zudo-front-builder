//! Production [`ClientBundler`] implementation on zfb's in-process native
//! bundler (`zfb_rolldown_prototype`).
//!
//! The trait + shared types ([`crate::bundler::ClientBundler`],
//! [`crate::bundler::Island`], [`crate::bundler::BundleConfig`],
//! [`crate::bundler::BundleOutput`]) are owned by [`crate::bundler`]; this
//! module builds one typed [`BundleRequest`] per job (the shared islands entry,
//! each module worker, each client script), runs it into a private staging
//! directory, and reads every output back into memory — the bundler does NOT
//! write `islands.js` to disk. Downstream consumers (the prod pipeline, the dev
//! server) own all disk writes. `ProductionAssetPipeline` is the single source
//! of truth for content hashing per the Prod Asset Graph epic.
//!
//! The scanner supplies only validated concrete boundary targets here.
//! The generated entry imports each target through its selected export,
//! registers its actual function name as the marker, and keeps ordinary
//! dependencies alive through that target's import graph. An unused helper
//! may be tree-shaken without receiving a registry slot. Production keeps
//! function names so the browser checks match the SSR marker and build token.
//!
//! ## CSS-import policy (issue #1395)
//!
//! `"use client"` islands are allowed to `import "./x.css"` (and
//! `import styles from "./x.module.css"`), same as any other component in
//! the project. Every job maps `.css` **and** `.module.css` to the empty
//! loader, mirroring the SSR bundler: the import resolves to an empty module
//! and no sibling CSS output is emitted for the read-back to reject. Authored
//! CSS is shipped separately via the zfb CSS pipeline's `styles-<hash>.css` —
//! islands never carry those bytes in their own JS bundle. A configured raw
//! `css` loader is rejected by the native bundler with its own message.
//!
//! **Out of scope (deliberately):** unlike the SSR bundler,
//! `.module.css` imports inside an island do NOT get a scoped
//! class-name map; `import styles from "./x.module.css"` resolves to
//! `styles === undefined` in the client bundle (#1404, documented by #1406).

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use zfb_build::metafile_deps::audit_metafile_stage_escape_at_path;
use zfb_rolldown_prototype::{BundleRequest, External, JsxRuntime, Loader, OutputLayout, Platform};
use zfb_types::json_string;

use crate::bundler::{
    bundle_link_href, BundleChunk, BundleConfig, BundleOutput, BundleResource, ClientBundler,
    Island, ModuleId, ModuleWorkerBundleEntry,
};
use crate::client_scripts::ClientScriptWorkerEntry;

/// Invalid or missing owned build identity for a nonempty public islands bundle.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IslandsBundleBuildIdentityError {
    #[error("nonempty owned islands bundle requires zudo_react_build")]
    Missing,
    #[error("nonempty owned islands bundle requires a 16-character lowercase hex zudo_react_build token")]
    Invalid,
}

fn validate_owned_build_identity(
    islands: &[Island],
    config: &BundleConfig,
) -> std::result::Result<(), IslandsBundleBuildIdentityError> {
    if islands.is_empty() {
        return Ok(());
    }
    let build = config
        .zudo_react_build
        .as_deref()
        .ok_or(IslandsBundleBuildIdentityError::Missing)?;
    if build.len() != 16
        || !build
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(IslandsBundleBuildIdentityError::Invalid);
    }
    Ok(())
}

/// In-memory output of one client-script bundle plus its module-worker
/// companions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientScriptBundleOutput {
    /// Bundled client-script entry source.
    pub js: String,
    /// Stable, flat worker files emitted beside the entry.
    pub companions: Vec<BundleChunk>,
}

/// Entry file pattern for the shared islands bundle.
///
/// `islands` (no extension — the bundler appends `.js`) pins the entry's
/// basename to the stable `STABLE_ISLANDS_FILENAME` so the staging
/// read-back can recognise the entry file among the staged outputs. A
/// `debug_assert!` in [`build_bundle_request`] ties this template to
/// `STABLE_ISLANDS_FILENAME`'s stem so the two can never silently drift.
pub(crate) const ENTRY_NAME_TEMPLATE: &str = "islands";

/// Chunk file pattern for code-split chunks.
///
/// `islands-chunk-[hash]` puts the bundler's own content hash in each chunk's
/// **flat** basename, in the SAME directory as the entry. This is the core
/// of the bundler split contract: the bundler — not the prod pipeline — owns
/// the chunk hash, and bakes relative `import("./islands-chunk-<hash>.js")`
/// references between chunks/entry. Downstream therefore MUST ship chunks
/// under these exact names (never rename them); only the entry may later be
/// content-hashed by the prod pipeline, because its relative chunk imports
/// still resolve from the shared directory. See [`crate::BundleOutput::chunks`]
/// and `CHUNK_FILENAME_PREFIX` (the read-back validation gate).
pub(crate) const CHUNK_NAME_TEMPLATE: &str = "islands-chunk-[hash]";

/// Basename prefix every emitted chunk file must carry
/// (`islands-chunk-...`), derived from [`CHUNK_NAME_TEMPLATE`].
/// The read-back rejects any non-entry output file that is not flat (no path
/// separators / traversal) and does not start with this prefix.
pub(crate) const CHUNK_FILENAME_PREFIX: &str = "islands-chunk-";

/// Prefix for each per-worker entry file pattern.
///
/// The complete entry name is derived from
/// [`zfb_types::module_worker_filename`] with the trailing `.js` removed;
/// the bundler appends that extension again. Aliasing the shared constant
/// pins this layer to the #1500 contract at compile time.
pub(crate) const WORKER_ENTRY_NAME_PREFIX: &str = zfb_types::MODULE_WORKER_FILENAME_PREFIX;

/// Asset file pattern for locked non-entry browser resources.
///
/// The template is flat and self-hashed so the copied resource can remain
/// beside a production-hashed islands entry without a later writer having to
/// rename (and therefore break) the URL the bundler already emitted.
pub(crate) const RESOURCE_NAME_TEMPLATE: &str = "islands-resource-[name]-[hash]";

/// Prefix every locked file-loader resource output must carry.
pub(crate) const RESOURCE_FILENAME_PREFIX: &str = "islands-resource-";

/// Fixed module-graph basename inside the private shared-islands staging
/// outdir. It is an oracle used only during read-back and is never itself
/// returned as a companion.
const RESOURCE_METAFILE_FILENAME: &str = "meta.json";

const ZFB_RESOURCE_MJS_EXTENSION: &str = ".zfb-resource.mjs";
const WASM_EXTENSION: &str = ".wasm";

#[derive(Debug, Clone)]
pub struct IslandsBundlerConfig {
    /// Working directory of every bundle job. Entry points and `node_modules`
    /// lookups resolve relative to it, and module-graph keys are spelled
    /// relative to it.
    ///
    /// Default: the current working directory at engine construction time.
    pub working_dir: PathBuf,

    /// Optional closest-parent config discovery boundary for entries inside a
    /// preprocessing shadow. Physical/no-shadow entries leave this `None` and
    /// retain normal monorepo ancestor discovery. Shadow callers set it to the
    /// staged root, which contains an explicit real-ancestor or empty boundary
    /// config and must never inherit an unrelated config above the tempdir.
    pub tsconfig_search_boundary: Option<PathBuf>,

    /// Extra specifiers left unbundled by every job, after the built-in
    /// `node:` prefix.
    ///
    /// **Trust boundary**: callers are expected to assemble these from
    /// build configuration, NOT from end-user input.
    pub externals: Vec<External>,

    /// Extra package export conditions for every job, added to the platform
    /// defaults. `None` keeps the defaults alone.
    pub conditions: Option<Vec<String>>,

    /// When true, the bundler will return a *fake* JS payload instead of
    /// bundling. Used by unit tests that only exercise entry synthesis and
    /// output assembly. The string returned is taken from
    /// [`Self::mock_output`].
    pub mock_bundle: bool,

    /// Output to return when `mock_bundle` is true.
    pub mock_output: String,

    /// Module search path consulted after `node_modules`, in `NODE_PATH`
    /// syntax. Used to resolve bare imports against an externally-staged
    /// `node_modules` tree (e.g. the `include_dir!`-extracted `@takazudo/*` +
    /// framework runtime tree from the cargo-installed binary; issue zfb#221).
    /// `None` falls back to the process's inherited `NODE_PATH` (the package
    /// manager's bin shim exports one naming its hidden-hoist store, #4430);
    /// an empty value means no extra directories.
    pub node_path: Option<OsString>,

    /// Import alias rewrites consumed by the islands bundler (#261, #269).
    ///
    /// Each entry is an exact-match `(from, to)` pair where `to` is the
    /// absolute filesystem path the alias resolves to. Populated from
    /// [`zfb_build::AliasMap`] by the build/dev orchestrator after the
    /// plugin `setup` hook runs.
    ///
    /// **Exact-match only** (mirrors `AliasMap`'s contract): a
    /// registration for `"@/foo"` matches `import "@/foo"` but NOT
    /// `import "@/foo/bar"`. To achieve this, `bundle_one_entry`
    /// writes a synthetic `tsconfig.json` whose `compilerOptions.paths`
    /// map carries one entry per pair (wildcard-free, which is the
    /// literal exact-match shape in TS path mapping) and hands it to the
    /// job as its tsconfig. Directory targets are never registered as
    /// prefix aliases, which would silently rewrite `@/foo/bar` to
    /// `<to>/bar`, contradicting the V8 host's exact-match contract
    /// (`zfb-render::BundleModuleLoader::resolve_alias`).
    ///
    /// Populated only when plugin registrations are present. An empty
    /// `Vec` (the default) skips both the helper call and the
    /// synthetic tsconfig, so the bundle output is byte-identical to
    /// a build without any plugin registrations (#261
    /// zero-registration regression guard).
    pub alias_entries: Vec<(String, String)>,

    /// Virtual-module source strings consumed by the islands bundler (#261, #269).
    ///
    /// Each entry is a `(specifier, source)` pair. The specifier is the
    /// bare import string an island uses (e.g. `"virtual:my-data"`); the
    /// source is the ESM module text the plugin's loader produced (already
    /// fetched from the plugin host via
    /// [`zfb_build::PluginHost::invoke_virtual_loader`] before constructing
    /// this config).
    ///
    /// In `bundle_one_entry` each source string is written to a
    /// `.zfb-virtual-*.mjs` temp file (allocated by
    /// `zfb_plugin_resolver::build_resolver_inputs`, inside
    /// `working_dir` or the system tempdir as a fallback) and the temp
    /// file's path is added to the synthetic tsconfig's
    /// `compilerOptions.paths` map under the bare specifier — same
    /// exact-match shape as `alias_entries`. Temp files are held alive
    /// for the duration of the bundle job and dropped afterwards.
    ///
    /// An empty `Vec` (the default) adds no temp files and no
    /// synthetic tsconfig entry — zero-registration builds are
    /// byte-identical to today.
    pub virtual_modules: Vec<(String, String)>,
    /// Authored virtual sources before CLI shadow import remapping. These
    /// bytes identify the source; the bundler still resolves the staged text.
    pub original_virtual_modules: BTreeMap<String, Vec<u8>>,

    /// Optional per-job stage-escape audit policy (issue #1705, epic #1702's
    /// guard (b) wiring wave). When `Some`, every real (non-mock) bundle job
    /// writes its module graph and is audited immediately after it succeeds
    /// via [`zfb_build::metafile_deps::audit_metafile_stage_escape`] — the
    /// same fail-closed check the sibling-mirror epic's guard (b)
    /// primitive implements, catching a bare first-party package-name
    /// import that climbed a staged `node_modules` symlink to a live
    /// workspace sibling.
    ///
    /// `None` (the default) skips both graph emission and auditing for
    /// every job type that does not already write its graph. Mock bundles
    /// never emit or require a graph regardless of this setting.
    pub stage_audit: Option<StageAuditPolicy>,
}

/// Per-job stage-escape audit policy consumed by
/// [`IslandsBundlerConfig::stage_audit`]. Mirrors the parameter shape of
/// [`zfb_build::metafile_deps::audit_metafile_stage_escape`] directly — the
/// bundler passes these fields through unchanged for every real job it
/// audits.
#[derive(Debug, Clone)]
pub struct StageAuditPolicy {
    /// Roots a resolved input may legitimately live under. A build can
    /// stage into more than one root (e.g. a shadow root plus a
    /// separately wholesale-mirrored sibling root).
    pub stage_roots: Vec<PathBuf>,
    /// Outer boundary of "this build's own source" — the project root, or
    /// the widened workspace root for a sibling-mirrored build.
    pub first_party_root: PathBuf,
}

impl Default for IslandsBundlerConfig {
    fn default() -> Self {
        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            working_dir,
            tsconfig_search_boundary: None,
            externals: Vec::new(),
            conditions: None,
            mock_bundle: false,
            mock_output: String::new(),
            node_path: None,
            alias_entries: Vec::new(),
            virtual_modules: Vec::new(),
            original_virtual_modules: BTreeMap::new(),
            stage_audit: None,
        }
    }
}

impl IslandsBundlerConfig {
    /// Override the working directory (chainable).
    pub fn with_working_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.working_dir = dir.into();
        self
    }

    /// Bound closest-parent tsconfig/jsconfig discovery for physical entries
    /// staged inside `root` (chainable).
    pub fn with_tsconfig_search_boundary(mut self, root: impl Into<PathBuf>) -> Self {
        self.tsconfig_search_boundary = Some(root.into());
        self
    }

    /// Configure the bundler to skip bundling and return `output`
    /// instead. Used by unit tests.
    pub fn with_mock_output(mut self, output: impl Into<String>) -> Self {
        self.mock_bundle = true;
        self.mock_output = output.into();
        self
    }

    /// Set the module search path consulted after `node_modules`, in
    /// `NODE_PATH` syntax (chainable). See [`Self::node_path`].
    pub fn with_node_path(mut self, value: impl Into<OsString>) -> Self {
        self.node_path = Some(value.into());
        self
    }

    /// Set the alias entries for the islands bundler (chainable).
    ///
    /// Replaces the current `alias_entries` list. Each `(from, to)`
    /// pair becomes an exact-match `compilerOptions.paths` entry in the
    /// synthetic tsconfig the bundler hands to each job (#269); `to`
    /// must be an absolute filesystem path (as produced by
    /// [`zfb_build::AliasMap`]). When the list is empty (the default)
    /// no synthetic tsconfig is generated and the bundle is
    /// byte-identical to a zero-registration build (#261 regression
    /// guard).
    pub fn with_alias_entries(mut self, aliases: Vec<(String, String)>) -> Self {
        self.alias_entries = aliases;
        self
    }

    /// Set the virtual-module source map for the islands bundler (chainable).
    ///
    /// Replaces the current `virtual_modules` list. Each
    /// `(specifier, source)` pair causes `bundle_one_entry` to
    /// materialize the source to a `.zfb-virtual-*.mjs` temp file (via
    /// `zfb_plugin_resolver::build_resolver_inputs`) and to add an
    /// exact-match `compilerOptions.paths` entry under the bare
    /// specifier in the synthetic tsconfig (#269). The source must be
    /// an ESM module string (the text returned by
    /// [`zfb_build::PluginHost::invoke_virtual_loader`]).
    pub fn with_virtual_modules(mut self, vms: Vec<(String, String)>) -> Self {
        self.virtual_modules = vms;
        self
    }

    /// Set the stage-escape audit policy (chainable). See
    /// [`StageAuditPolicy`]. When set, every real job this bundler runs
    /// writes its module graph and is audited immediately after it
    /// succeeds; mock bundles are unaffected.
    pub fn with_stage_audit(mut self, policy: StageAuditPolicy) -> Self {
        self.stage_audit = Some(policy);
        self
    }

    /// The extra module directories every job searches after `node_modules`:
    /// [`Self::node_path`] when set, otherwise the inherited `NODE_PATH`.
    fn extra_module_dirs(&self) -> Vec<PathBuf> {
        self.node_path
            .clone()
            .or_else(|| std::env::var_os("NODE_PATH"))
            .filter(|value| !value.is_empty())
            .map(|value| std::env::split_paths(&value).collect())
            .unwrap_or_default()
    }
}

/// The default [`ClientBundler`]: zfb's in-process native bundler.
///
/// The shared pass bundles one synthesized entry source as ESM with code
/// splitting; minification and linked sourcemaps follow [`BundleConfig`].
/// Output goes to a staging directory: the entry is emitted under the stable
/// name `islands.js` and any code-split chunks under self-hashed flat names
/// (`islands-chunk-[hash]`). All emitted files are read back into memory and
/// returned via [`BundleOutput`] — the bundler does **NOT** write to
/// `{outdir}/assets/islands.js`. Downstream consumers (prod pipeline, dev
/// server) own all disk writes; chunks are returned via
/// [`BundleOutput::chunks`] (see that field's contract).
/// `ProductionAssetPipeline` performs the content-hash + rename pass on the
/// *entry* at deploy time; chunks already carry the bundler's own content
/// hash and must never be renamed.
///
/// ### Example
///
/// ```no_run
/// use zfb_islands::{
///     BundleConfig, ClientBundler, IslandsBundler,
///     IslandsBundlerConfig, Island,
/// };
/// use std::path::PathBuf;
///
/// let bundler = IslandsBundler::new(IslandsBundlerConfig::default());
/// let islands = vec![Island::new("Counter", PathBuf::from("components/counter.tsx"))];
/// let config = BundleConfig::production()
///     .with_zudo_react_build(Some("0123456789abcdef".to_string()));
/// let out = bundler.bundle(&islands, &config).unwrap();
/// assert_eq!(out.asset_url, "/assets/islands.js");
/// ```
#[derive(Debug, Clone)]
pub struct IslandsBundler {
    config: IslandsBundlerConfig,
}

/// Build the synthetic `tsconfig.json` esbuild reads via `--tsconfig=<file>` when a plugin
/// has registered alias / virtual-module path entries.
///
/// Returns `Ok(None)` when there are **no** plugin entries — in that case no synthetic
/// tsconfig is written and esbuild walks up from the entry file to discover the project's
/// real `tsconfig.json`, exactly as it does with no plugin. The no-plugin code path is thus
/// byte-identical to its pre-fix behaviour. The `paths_entries.is_empty()` early return MUST
/// stay ahead of the user-tsconfig read and the temp-file write for that guarantee to hold.
///
/// When entries exist, `paths_map` is **seeded from the caller-supplied `user_tsconfig_paths`**
/// (the user's `compilerOptions.paths`, read once by the caller via
/// `zfb_plugin_resolver::read_tsconfig_paths_into_map` — absolutised targets, wildcards
/// preserved) **before** the plugin entries are merged on top. This is the #1238 fix: an
/// explicit `--tsconfig=` makes esbuild stop walking up to the real tsconfig, so without
/// seeding the user's own aliases (e.g. `@/*`) would silently vanish the moment a plugin
/// registered anything. `merge_into_tsconfig_paths` applies a "user wins" policy on a key
/// collision, so plugin entries are additive on top of the user's hand-written contract. The
/// caller passes the SAME map to `build_resolver_inputs` so the virtual-module `--alias` set
/// honors the same user-wins policy (#1267).
///
/// `label` distinguishes call sites in temp-file names / error context (today only
/// `"islands"` is ever passed — see the call site below); `comment` is the `//`
/// provenance string embedded in the emitted JSON.
fn build_plugin_tsconfig(
    working_dir: &Path,
    resolver_inputs: &zfb_plugin_resolver::ResolverInputs,
    user_tsconfig_paths: &BTreeMap<String, Vec<String>>,
    label: &str,
    comment: &str,
) -> Result<Option<tempfile::NamedTempFile>> {
    // INVARIANT: early-return BEFORE consuming the user tsconfig or writing any synthetic
    // tsconfig, so the no-plugin path reads nothing and emits no `--tsconfig` (byte-identical).
    // The caller only reads `user_tsconfig_paths` from disk when there is at least one plugin
    // entry (see the `read_tsconfig_paths_into_map` call sites), so the no-plugin path still
    // touches no tsconfig.
    if resolver_inputs.paths_entries.is_empty() {
        return Ok(None);
    }

    let mut paths_map = user_tsconfig_paths.clone();
    zfb_plugin_resolver::merge_into_tsconfig_paths(&mut paths_map, &resolver_inputs.paths_entries);

    let json = serde_json::json!({
        "//": comment,
        "compilerOptions": {
            "baseUrl": ".",
            "paths": paths_map,
        },
    });

    let tmp = if working_dir.is_dir() {
        let prefix = format!(".zfb-{label}-tsconfig-");
        tempfile::Builder::new()
            .prefix(prefix.as_str())
            .suffix(".json")
            .tempfile_in(working_dir)
            .with_context(|| {
                format!("failed to allocate {label} tsconfig temp file inside working_dir")
            })?
    } else {
        let prefix = format!("zfb-{label}-tsconfig-");
        tempfile::Builder::new()
            .prefix(prefix.as_str())
            .suffix(".json")
            .tempfile()
            .with_context(|| format!("failed to allocate {label} tsconfig temp file"))?
    };
    std::fs::write(tmp.path(), serde_json::to_vec_pretty(&json)?)
        .with_context(|| format!("failed to write {label} synthetic tsconfig"))?;
    // Claim a shared lock so a concurrent sweep of this crate's temp classes (mirroring
    // `allocate_locked_entry_tmp`'s contract) can tell "still in use" from "orphaned" rather
    // than inferring it from age alone.
    let _ = tmp.as_file().lock_shared();
    Ok(Some(tmp))
}

fn validate_client_script_worker_entries(
    entry_name: &str,
    workers: &[ClientScriptWorkerEntry],
) -> Result<BTreeMap<String, PathBuf>> {
    let entry_filename = format!("{entry_name}.js");
    let mut validated: BTreeMap<String, PathBuf> = BTreeMap::new();
    for worker in workers {
        let filename = worker.filename.as_str();
        if filename.is_empty()
            || filename.contains('/')
            || filename.contains('\\')
            || filename.contains("..")
            || !filename.starts_with(zfb_types::MODULE_WORKER_FILENAME_PREFIX)
            || !filename.ends_with(".js")
        {
            return Err(anyhow!(
                "client-script module-worker filename {filename:?} must be a flat contract name matching `{}*.js`",
                zfb_types::MODULE_WORKER_FILENAME_PREFIX
            ));
        }
        if filename == entry_filename {
            return Err(anyhow!(
                "client-script entry `{entry_name}` and module worker {} collide on output filename {filename:?}",
                worker.source_path.display()
            ));
        }
        if let Some(existing) = validated.get(filename) {
            if existing != &worker.source_path {
                return Err(anyhow!(
                    "client-script module-worker filename collision for {filename:?}: {} vs {}",
                    existing.display(),
                    worker.source_path.display()
                ));
            }
            continue;
        }
        validated.insert(filename.to_string(), worker.source_path.clone());
    }
    Ok(validated)
}

/// Strict client-script read-back trust boundary.
///
/// Exactly one `<entry>.js` and every expected contract-named worker must be
/// present. Linked sourcemaps for those known outputs are ignored, preserving
/// the existing in-memory JS-only behavior. Any other file or directory is a
/// hard error.
fn read_back_client_script_outdir(
    out_dir: &Path,
    entry_name: &str,
    expected_workers: &BTreeSet<String>,
) -> Result<ClientScriptBundleOutput> {
    let entry_filename = format!("{entry_name}.js");
    let mut entry_js = None;
    let mut companions = BTreeMap::new();
    let mut expected_js = expected_workers.clone();
    expected_js.insert(entry_filename.clone());

    for dirent in std::fs::read_dir(out_dir).with_context(|| {
        format!(
            "failed to read client-script staging outdir {}",
            out_dir.display()
        )
    })? {
        let dirent = dirent.context("failed to read client-script staging outdir entry")?;
        let file_type = dirent
            .file_type()
            .context("failed to inspect client-script output")?;
        let os_name = dirent.file_name();
        let name = os_name.to_str().ok_or_else(|| {
            anyhow!(
                "the bundler emitted a non-UTF-8 client-script filename in {}",
                out_dir.display()
            )
        })?;
        if !file_type.is_file() {
            return Err(anyhow!(
                "the bundler emitted an unexpected non-file client-script output {name:?}"
            ));
        }
        if let Some(js_name) = name.strip_suffix(".map") {
            if expected_js.contains(js_name) {
                continue;
            }
            return Err(anyhow!(
                "the bundler emitted an unexpected client-script sourcemap {name:?}"
            ));
        }
        if name == entry_filename {
            entry_js = Some(
                std::fs::read_to_string(dirent.path())
                    .context("failed to read client-script entry output")?,
            );
            continue;
        }
        if expected_workers.contains(name) {
            companions.insert(
                name.to_string(),
                std::fs::read(dirent.path())
                    .with_context(|| format!("failed to read module worker output {name}"))?,
            );
            continue;
        }
        return Err(anyhow!(
            "the bundler emitted an unexpected client-script output file {name:?}; expected entry {entry_filename:?} or one of {expected_workers:?}"
        ));
    }

    let js = entry_js.ok_or_else(|| {
        anyhow!("the bundler produced no `{entry_filename}` for client-script entry `{entry_name}`")
    })?;
    let missing: Vec<_> = expected_workers
        .iter()
        .filter(|filename| !companions.contains_key(*filename))
        .cloned()
        .collect();
    if !missing.is_empty() {
        return Err(anyhow!(
            "the bundler produced no output for client-script module worker(s): {}",
            missing.join(", ")
        ));
    }
    Ok(ClientScriptBundleOutput {
        js,
        companions: companions
            .into_iter()
            .map(|(filename, bytes)| BundleChunk { filename, bytes })
            .collect(),
    })
}

/// Resolver inputs scoped to one physical esbuild entry.
///
/// Preprocessing shadows mirror the user's config hierarchy, so the closest
/// `tsconfig.json` / `jsconfig.json` must be selected from the physical entry
/// rather than assuming `<working_dir>/tsconfig.json`. When plugin paths are
/// present, `tsconfig` extends that closest config and overlays its
/// shadow-resolved user paths with plugin entries. With no plugin paths the
/// field stays `None` and esbuild retains its native closest-parent discovery.
struct JobResolverInputs {
    resolver_inputs: zfb_plugin_resolver::ResolverInputs,
    tsconfig: Option<tempfile::NamedTempFile>,
}

fn nearest_typescript_config(source_path: &Path, boundary: Option<&Path>) -> Option<PathBuf> {
    let mut dir = source_path.parent()?;
    let boundary = boundary.filter(|boundary| source_path.starts_with(boundary));
    loop {
        let tsconfig = dir.join("tsconfig.json");
        if tsconfig.is_file() {
            return Some(tsconfig);
        }
        let jsconfig = dir.join("jsconfig.json");
        if jsconfig.is_file() {
            return Some(jsconfig);
        }
        if boundary.is_some_and(|boundary| dir == boundary) {
            return None;
        }
        let parent = dir.parent()?;
        if parent == dir {
            return None;
        }
        dir = parent;
    }
}

/// The pinned native resolver discovers only tsconfig.json and does not honor
/// package.json#tsconfig in extends. Materialize the already selected per-entry
/// policy (the request's explicit tsconfig, else the closest config) without
/// writing into packages.
fn native_entry_config(
    request: &BundleRequest,
    source_path: &Path,
    boundary: Option<&Path>,
) -> Result<Option<tempfile::NamedTempFile>> {
    let selected = request
        .tsconfig
        .as_ref()
        .map(|path| request.cwd.join(path))
        .or_else(|| nearest_typescript_config(source_path, boundary));
    let Some(selected) = selected else {
        return Ok(None);
    };
    let selected = std::path::absolute(selected).context("absolute native config selection")?;
    let effective = native_effective_config(&selected, request.resolve.preserve_symlinks)?;
    let file = tempfile::Builder::new()
        .prefix("zfb-native-config-")
        .suffix(".json")
        .tempfile()
        .context("allocate native per-entry config")?;
    std::fs::write(file.path(), serde_json::to_vec(&effective)?)
        .context("write native per-entry config")?;
    Ok(Some(file))
}

fn native_effective_config(path: &Path, preserve_symlinks: bool) -> Result<serde_json::Value> {
    let (mut effective, paths_dir) =
        native_config_chain(path, preserve_symlinks, 0, &mut BTreeSet::new())?;
    let options = effective
        .get_mut("compilerOptions")
        .unwrap()
        .as_object_mut()
        .unwrap();
    let base = options
        .get("baseUrl")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .or(paths_dir);
    if let Some(paths) = options.get_mut("paths") {
        let base = base.context("native config paths declaring directory")?;
        for targets in paths
            .as_object_mut()
            .context("native config paths must be an object")?
            .values_mut()
        {
            native_config_paths(targets, &base)?;
        }
    }
    Ok(serde_json::Value::Object(effective))
}

fn native_config_chain(
    path: &Path,
    preserve_symlinks: bool,
    depth: usize,
    visiting: &mut BTreeSet<PathBuf>,
) -> Result<(serde_json::Map<String, serde_json::Value>, Option<PathBuf>)> {
    anyhow::ensure!(
        depth <= 8,
        "native config extends exceeded 8 levels: {}",
        path.display()
    );
    let identity = std::fs::canonicalize(path)
        .with_context(|| format!("resolve native config {}", path.display()))?;
    anyhow::ensure!(
        visiting.insert(identity.clone()),
        "cyclic native config extends: {}",
        path.display()
    );
    let path = if preserve_symlinks {
        path.to_path_buf()
    } else {
        identity.clone()
    };
    let dir = path.parent().context("native config directory")?;
    let raw = std::fs::read_to_string(&path)?;
    let value: serde_json::Value =
        serde_json::from_str(&zfb_plugin_resolver::strip_tsconfig_jsonc(&raw))
            .with_context(|| format!("parse native config {}", path.display()))?;
    let object = value
        .as_object()
        .context("native config must be an object")?;
    // Relocating these would require project ownership/template evaluation,
    // which this bounded compatibility layer does not implement.
    anyhow::ensure!(
        !value.to_string().contains("${configDir}"),
        "native configDir templates are unsupported: {}",
        path.display()
    );
    if let Some(references) = object.get("references") {
        anyhow::ensure!(
            references.as_array().is_some_and(Vec::is_empty),
            "native config project references are unsupported: {}",
            path.display()
        );
    }
    let parents = match object.get("extends") {
        None => Vec::new(),
        Some(serde_json::Value::String(parent)) => vec![parent.as_str()],
        Some(serde_json::Value::Array(parents)) => parents
            .iter()
            .map(|parent| {
                parent
                    .as_str()
                    .context("native config extends entries must be strings")
            })
            .collect::<Result<Vec<_>>>()?,
        Some(_) => anyhow::bail!(
            "native config extends must be a string or array: {}",
            path.display()
        ),
    };
    let mut effective = serde_json::Map::new();
    let mut compiler = serde_json::Map::new();
    let mut paths_dir = None;
    for parent in parents {
        let resolved = zfb_plugin_resolver::resolve_tsconfig_extends_file(dir, parent)
            .with_context(|| {
                format!(
                    "native config {} extends unresolved target {parent:?}",
                    path.display()
                )
            })?;
        let (mut parent, parent_paths_dir) =
            native_config_chain(&resolved, preserve_symlinks, depth + 1, visiting)?;
        if let Some(options) = parent.remove("compilerOptions") {
            if options.get("paths").is_some() {
                paths_dir = parent_paths_dir;
            }
            compiler.extend(
                options
                    .as_object()
                    .expect("normalized compiler options")
                    .clone(),
            );
        }
        // Resolve each parent independently before overlaying it. Flattening a
        // deduplicated dependency list would break diamond-extends precedence.
        effective.extend(parent);
    }
    let mut local = object.clone();
    local.remove("extends");
    local.remove("references");
    if let Some(options) = local.remove("compilerOptions") {
        let mut options = options
            .as_object()
            .context("native compilerOptions must be an object")?
            .clone();
        for key in [
            "baseUrl",
            "rootDir",
            "outDir",
            "declarationDir",
            "outFile",
            "tsBuildInfoFile",
        ] {
            if let Some(value) = options.get_mut(key) {
                native_config_path(value, dir)?;
            }
        }
        for key in ["rootDirs", "typeRoots"] {
            if let Some(value) = options.get_mut(key) {
                native_config_paths(value, dir)?;
            }
        }
        if let Some(paths) = options.get("paths") {
            // Keep targets relative until the entire chain selects its final
            // baseUrl, which may be declared by a different parent or the leaf.
            for targets in paths
                .as_object()
                .context("native config paths must be an object")?
                .values()
            {
                anyhow::ensure!(
                    targets
                        .as_array()
                        .is_some_and(|items| items.iter().all(serde_json::Value::is_string)),
                    "native config paths targets must be arrays of strings"
                );
            }
            paths_dir = Some(dir.to_path_buf());
        }
        compiler.extend(options);
    }
    for key in ["files", "include", "exclude"] {
        if let Some(value) = local.get_mut(key) {
            native_config_paths(value, dir)?;
        }
    }
    effective.extend(local);
    effective.insert(
        "compilerOptions".into(),
        serde_json::Value::Object(compiler),
    );
    visiting.remove(&identity);
    Ok((effective, paths_dir))
}

fn native_config_path(value: &mut serde_json::Value, base: &Path) -> Result<()> {
    let path = value
        .as_str()
        .context("native config path must be a string")?;
    *value = serde_json::Value::String(base.join(path).to_string_lossy().into_owned());
    Ok(())
}

fn native_config_paths(value: &mut serde_json::Value, base: &Path) -> Result<()> {
    for path in value
        .as_array_mut()
        .context("native config paths must be an array")?
    {
        native_config_path(path, base)?;
    }
    Ok(())
}

/// Runs one typed job: selects its per-entry config, then generates and
/// publishes every output into the job's staging directory.
fn run_native_entry(
    mut request: BundleRequest,
    source_path: &Path,
    boundary: Option<&Path>,
) -> Result<()> {
    let config = native_entry_config(&request, source_path, boundary)?;
    if let Some(config) = &config {
        request.tsconfig = Some(config.path().to_path_buf());
    }
    // Keep the config owner alive through native generation and runtime teardown.
    zfb_rolldown_prototype::bundle(&request)?;
    drop(config);
    Ok(())
}

fn build_job_resolver_inputs(
    working_dir: &Path,
    source_path: &Path,
    tsconfig_search_boundary: Option<&Path>,
    alias_entries: &[(String, String)],
    virtual_modules: &[(String, String)],
) -> Result<JobResolverInputs> {
    let nearest_config = nearest_typescript_config(source_path, tsconfig_search_boundary);
    let has_plugin_entries = !alias_entries.is_empty() || !virtual_modules.is_empty();
    let user_tsconfig_paths = if has_plugin_entries {
        nearest_config
            .as_deref()
            .map(zfb_plugin_resolver::read_tsconfig_paths_file_into_map)
            .unwrap_or_default()
    } else {
        BTreeMap::new()
    };
    let resolver_inputs = zfb_plugin_resolver::build_resolver_inputs(
        alias_entries,
        virtual_modules,
        working_dir,
        &user_tsconfig_paths,
        &["zfb"],
    )
    .context("zfb-islands: failed materializing per-entry plugin resolver inputs")?;

    if resolver_inputs.paths_entries.is_empty() {
        return Ok(JobResolverInputs {
            resolver_inputs,
            tsconfig: None,
        });
    }

    let mut paths_map = user_tsconfig_paths.clone();
    zfb_plugin_resolver::merge_into_tsconfig_paths(&mut paths_map, &resolver_inputs.paths_entries);

    let mut root = serde_json::Map::new();
    root.insert(
        "//".to_string(),
        serde_json::Value::String(
            "Synthetic tsconfig generated by zfb-islands for one physical bundle entry."
                .to_string(),
        ),
    );
    if let Some(config_path) = nearest_config {
        root.insert(
            "extends".to_string(),
            serde_json::Value::String(config_path.to_string_lossy().into_owned()),
        );
    }
    let mut compiler_options = serde_json::Map::new();
    if !paths_map.is_empty() {
        // Every user target is absolute relative to the selected physical
        // config (therefore inside the preprocessing shadow when applicable),
        // and plugin resolver targets are absolute too.
        compiler_options.insert("paths".to_string(), serde_json::to_value(paths_map)?);
        if !root.contains_key("extends") {
            // With no project config to extend, anchor plugin-only path
            // entries at the configured working directory.
            compiler_options.insert(
                "baseUrl".to_string(),
                serde_json::Value::String(working_dir.to_string_lossy().into_owned()),
            );
        }
    }
    root.insert(
        "compilerOptions".to_string(),
        serde_json::Value::Object(compiler_options),
    );

    // This one `.zfb-worker-tsconfig-*.json` class serves BOTH module-worker jobs and
    // client-script entry jobs — `build_job_resolver_inputs` is called once per physical
    // esbuild entry regardless of which job shape owns that entry, so there is no separate
    // `.zfb-client-script-tsconfig-*.json` class to look for.
    let tmp = if working_dir.is_dir() {
        tempfile::Builder::new()
            .prefix(WORKER_TSCONFIG_PREFIX)
            .suffix(TSCONFIG_TEMP_SUFFIX)
            .tempfile_in(working_dir)
            .context("failed to allocate module-worker tsconfig inside working_dir")?
    } else {
        tempfile::Builder::new()
            .prefix("zfb-worker-tsconfig-")
            .suffix(TSCONFIG_TEMP_SUFFIX)
            .tempfile()
            .context("failed to allocate module-worker tsconfig")?
    };
    std::fs::write(tmp.path(), serde_json::to_vec_pretty(&root)?)
        .context("failed to write per-entry synthetic tsconfig")?;
    // Claim a shared lock so a concurrent sweep of this crate's temp classes (mirroring
    // `allocate_locked_entry_tmp`'s contract) can tell "still in use" from "orphaned" rather
    // than inferring it from age alone.
    let _ = tmp.as_file().lock_shared();
    Ok(JobResolverInputs {
        resolver_inputs,
        tsconfig: Some(tmp),
    })
}

/// Allocate a scratch `--metafile` location for one real-subprocess job, or
/// `None` when no stage-audit policy is configured.
///
/// **Must NOT live inside the subprocess's `--outdir` (`out_dir`)**: the
/// strict output read-back (`read_back_outdir_with_resource_oracle`,
/// `read_back_client_script_outdir`) treats `out_dir` as esbuild's exact
/// promised output set and only special-cases the resource-contract pass's
/// own `meta.json` — any other stray file left inside `out_dir` (this one
/// included) is rejected as an unrecognised output. A dedicated system-temp
/// file sidesteps that trust boundary entirely; dropping the returned
/// `NamedTempFile` deletes it once the caller is done auditing.
fn stage_audit_metafile(
    policy: Option<&StageAuditPolicy>,
) -> Result<Option<tempfile::NamedTempFile>> {
    if policy.is_none() {
        return Ok(None);
    }
    tempfile::Builder::new()
        .prefix(".zfb-stage-audit-")
        .suffix(".json")
        .tempfile()
        .context("failed to allocate stage-audit metafile")
        .map(Some)
}

/// Diagnostic opt-in (#4499), off unless `ZFB_DEBUG_ISLANDS_METAFILE_DIR`
/// names a directory: copy the shared islands `meta.json` there before the
/// staging dir is dropped, so a test can map bundled code back to module ids.
/// A copy failure is reported on stderr and never fails the build.
fn keep_debug_islands_metafile(metafile: &Path) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEQ: AtomicUsize = AtomicUsize::new(0);

    let Some(dir) = std::env::var_os("ZFB_DEBUG_ISLANDS_METAFILE_DIR").filter(|d| !d.is_empty())
    else {
        return;
    };
    let dir = PathBuf::from(dir);
    let target = dir.join(format!(
        "islands-meta-{}-{}.json",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    if let Err(error) =
        std::fs::create_dir_all(&dir).and_then(|()| std::fs::copy(metafile, &target))
    {
        eprintln!(
            "[zfb] ZFB_DEBUG_ISLANDS_METAFILE_DIR: could not keep {} at {}: {error}",
            metafile.display(),
            target.display()
        );
    }
}

/// Run the guard (b) stage-escape audit against a just-emitted metafile and
/// attach the owning job's identity to any failure. Called immediately after
/// each real esbuild subprocess succeeds (never for mock subprocesses, and
/// only when [`IslandsBundlerConfig::stage_audit`] is `Some`).
fn run_stage_escape_audit(
    policy: &StageAuditPolicy,
    metafile_path: &Path,
    metafile_cwd: &Path,
    job_label: &str,
) -> Result<()> {
    let stage_roots: Vec<&Path> = policy.stage_roots.iter().map(PathBuf::as_path).collect();
    audit_metafile_stage_escape_at_path(
        metafile_path,
        metafile_cwd,
        &stage_roots,
        &policy.first_party_root,
    )
    .with_context(|| format!("zfb-islands: stage-escape audit failed for {job_label}"))
}

impl IslandsBundler {
    /// Construct a new bundler with the given config.
    pub fn new(config: IslandsBundlerConfig) -> Self {
        Self { config }
    }

    /// Construct a new bundler with the default config.
    pub fn with_default_config() -> Self {
        Self::new(IslandsBundlerConfig::default())
    }

    /// Borrow the underlying config.
    pub fn config(&self) -> &IslandsBundlerConfig {
        &self.config
    }

    /// Join staging provenance to paths returned by the resolver's own
    /// materialization. This never predicts import resolution winners.
    fn identity_source_overrides(
        &self,
        virtual_paths: &[(String, String)],
    ) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut sources = BTreeMap::new();
        for (specifier, path) in virtual_paths {
            if let Some(source) = self.config.original_virtual_modules.get(specifier) {
                sources.insert(PathBuf::from(path), source.clone());
            }
        }
        sources
    }

    /// Reap every recognized in-project `.zfb-*` temp-file class
    /// ([`IN_PROJECT_TEMP_CLASSES`]) stranded in this project by an earlier
    /// zfb process that died without unwinding (issue #1970 / #1976, later
    /// generalized to the tsconfig + virtual-module classes by issue #2109)
    /// — see [`sweep_orphaned_in_project_temp_files`].
    ///
    /// Called once at the [`ClientBundler::bundle`] or
    /// [`Self::bundle_client_script_file_with_workers`]
    /// lifecycle boundary and strictly BEFORE the first entry is allocated. It
    /// deliberately does not live in [`Self::bundle_one_entry`], where a
    /// sweep would repeat for unrelated client-script and worker jobs.
    fn sweep_stranded_entries(&self) {
        if self.config.mock_bundle || !self.config.working_dir.is_dir() {
            return;
        }
        sweep_orphaned_in_project_temp_files(&self.config.working_dir);
    }

    /// Internal: produce the JS payload for the given islands. Split out so
    /// tests can drive it directly without going through `bundle`'s
    /// [`BundleOutput`] assembly step.
    ///
    /// Implementation: synthesizes a single-entry source that imports
    /// every island module by absolute path, then routes through
    /// [`Self::bundle_one_entry`] (single input + `--outdir`). This
    /// sidesteps esbuild's "Must use \"outdir\" when there are multiple
    /// input files" rule (issue #138): passing N island `source_path`s
    /// as N separate inputs trips it for any N >= 2.
    ///
    /// Returns the entry JS plus any code-split chunks esbuild emitted
    /// (empty for a zero-dynamic-import islands set).
    fn produce_bundle_js(
        &self,
        islands: &[Island],
        config: &BundleConfig,
    ) -> Result<OneEntryOutput> {
        validate_locked_resource_loader_overrides(&config.loaders)?;
        let entry_source = render_shared_bundle_entry_source_with_build(
            islands,
            config.client_router,
            config.zudo_react_build.as_deref(),
            &config.module_labels,
        );
        // Shared-bundle path: splitting ON. This is the ONLY path that ships
        // and serves chunks (#806/#808/#809) — `OneEntryOutput::chunks` is
        // threaded into `BundleOutput` here.
        self.bundle_one_entry(&entry_source, config, true, "shared entry", false)
    }

    /// Derive compatibility identity from the bundler's resolved executable closure.
    /// The probe does not ship: the normal pass still owns splitting, filenames
    /// and sourcemaps. No result is cached across builds or watcher generations.
    pub fn derive_build_token(
        &self,
        islands: &[Island],
        config: &BundleConfig,
        compiler_version: &str,
    ) -> Result<String> {
        validate_locked_resource_loader_overrides(&config.loaders)?;
        let mut probe = config.clone();
        probe.zudo_react_build = Some("0000000000000000".to_string());
        probe.minify = true;
        probe.sourcemap = false;
        let entry = render_shared_bundle_entry_source_with_build(
            islands,
            probe.client_router,
            probe.zudo_react_build.as_deref(),
            &probe.module_labels,
        );
        let output = self.bundle_one_entry(&entry, &probe, true, "identity probe", true)?;
        let mut digest = Sha256::new();
        digest.update(b"zfb-islands-resolved-executable/1\n");
        hash_identity_part(
            &mut digest,
            zfb_rolldown_prototype::BACKEND_IDENTITY.as_bytes(),
        );
        hash_identity_part(&mut digest, compiler_version.as_bytes());
        digest.update([
            config.mode.is_prod() as u8,
            config.minify as u8,
            config.sourcemap as u8,
        ]);
        // A minified probe can erase transform differences observable through
        // Function.prototype.toString in the unminified client. Keep explicit
        // transform inputs as well as raw resolved source and graph identity.
        // BTreeMap serialization is deterministic; frames keep fields distinct.
        hash_identity_part(&mut digest, &serde_json::to_vec(&config.define)?);
        hash_identity_part(&mut digest, &serde_json::to_vec(&config.loaders)?);
        // Operator resolution overrides change what the probe can reach.
        hash_identity_part(&mut digest, &serde_json::to_vec(&self.config.externals)?);
        hash_identity_part(&mut digest, &serde_json::to_vec(&self.config.conditions)?);
        for graph in &output.graph_digests {
            digest.update(graph);
        }
        hash_identity_part(&mut digest, output.js.as_bytes());
        for chunk in output.chunks.iter().chain(&output.workers) {
            hash_identity_part(&mut digest, chunk.filename.as_bytes());
            hash_identity_part(&mut digest, &chunk.bytes);
        }
        for resource in &output.resources {
            hash_identity_part(&mut digest, resource.filename.as_bytes());
            hash_identity_part(&mut digest, &resource.bytes);
        }
        Ok(hex::encode(digest.finalize())[..16].to_string())
    }

    /// Internal: bundle a single in-memory entry source. Writes the
    /// source to a temp file (because entry points resolve from disk),
    /// runs the job, and returns its JS and companion outputs.
    ///
    /// `job_label` identifies this invocation in
    /// [`IslandsBundlerConfig::stage_audit`] and splitting-pass
    /// entry-import audit failure messages (e.g. `"shared entry"`).
    fn bundle_one_entry(
        &self,
        entry_source: &str,
        config: &BundleConfig,
        splitting: bool,
        job_label: &str,
        identity_probe: bool,
    ) -> Result<OneEntryOutput> {
        if splitting {
            validate_module_worker_entries(&config.module_workers)?;
        }
        if self.config.mock_bundle {
            // Mock mode: return either the configured mock_output (when
            // set) or the entry source itself so tests can assert the
            // generated entry shape end-to-end. Mock mode never produces
            // chunks — the real bundler is the only chunk source, so the
            // splitting path is exercised by the integration tests.
            let js = if !self.config.mock_output.is_empty() {
                self.config.mock_output.clone()
            } else {
                entry_source.to_string()
            };
            let mut output = OneEntryOutput::js_only(js);
            if splitting {
                output.workers = config
                    .module_workers
                    .iter()
                    .map(|worker| {
                        validate_chunk_filename(worker.filename())?;
                        let bytes = std::fs::read(worker.source_path()).with_context(|| {
                            format!(
                                "mock: failed to read module-worker entry {}",
                                worker.source_path().display()
                            )
                        })?;
                        Ok(BundleChunk {
                            filename: worker.filename().to_string(),
                            bytes,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
            }
            return Ok(output);
        }

        // Entry temp file (.tsx so loader inference picks up JSX
        // automatically — the entry imports component .tsx files).
        //
        // **Why we allocate inside `working_dir` rather than `$TMPDIR`**
        //
        // The synthesized entry's imports (`@takazudo/zfb/runtime` and
        // `@takazudo/zfb/zudo-react/client`) must resolve from the project's
        // `node_modules/`. Resolution walks UP from the entry file's directory
        // looking for `node_modules`, **not** from process cwd. If the entry
        // lived at `/tmp/zfb-esbuild-entry-XXXX.tsx`, the walk would go
        // `/tmp -> /` and never reach the project (issue #147 /
        // zudolab/zudo-doc#1355 Wave 6 follow-up). Allocating inside
        // `working_dir` (= the consumer project root, set by
        // `IslandsBundlerConfig::with_working_dir`) puts the temp file next to
        // the project's `node_modules/` so the upward walk finds both packages
        // on the first hop.
        //
        // We fall back to the system tempdir when `working_dir` does not exist
        // on disk (real builds always have an existing project root).
        let entry_tmp = allocate_locked_entry_tmp(&self.config.working_dir, entry_source)?;

        // Staging output **directory**. Splitting needs a directory, and the
        // bundler emits the entry plus any code-split chunks here. We stage to
        // a throwaway temp dir and read every emitted file back into memory —
        // the bundler never writes into `dist/`; the prod pipeline / dev
        // server own all `dist` writes (see the `BundleOutput::chunks`
        // contract). A system tempdir is fine for the *output* (only the
        // *entry* temp file must sit beside the project's `node_modules`).
        let out_dir = tempfile::Builder::new()
            .prefix("zfb-esbuild-out-")
            .tempdir()
            .context("failed to allocate out temp dir")?;

        // Plugin-registered aliases + virtual modules (#269). Both surface
        // through a synthetic `compilerOptions.paths` map handed to the job
        // as its tsconfig. A plugin alias (`@/foo`) is tsconfig-paths-ONLY
        // when its target is a *directory* (or missing): a request alias is
        // prefix-with-slash, so registering `@/foo` there would silently
        // rewrite `@/foo/bar` (which can be a real file under that
        // directory), contradicting the embedded V8 host's exact-match
        // contract (`zfb-render::BundleModuleLoader::resolve_alias`). A
        // wildcard-free `compilerOptions.paths` entry is a literal exact
        // match.
        //
        // A single-FILE plugin alias target ALSO rides the request aliases
        // (#3002) — a file has no children, so prefix-with-slash matching
        // cannot capture a longer import and the exact-match contract holds —
        // subject to the gates documented on
        // `zfb_plugin_resolver::ResolverInputs::plugin_alias_args`
        // (is_file, user-wins, no slash-prefix collision). Plugin virtual
        // modules always ride them too (#1263): tsconfig `paths` do not apply
        // to a source file under `node_modules` (e.g. a route entrypoint
        // installed by a published preset), while request aliases do.
        //
        // `zfb_plugin_resolver::build_resolver_inputs` materializes each
        // virtual module's source to a `.zfb-virtual-*.mjs` temp file inside
        // `working_dir` (same node_modules-walk rationale as the entry temp
        // file above) and returns POSIX-normalized `(specifier,
        // absolute-path)` pairs. The held-alive temp files live in
        // `resolver_inputs._temp_files` and are dropped after every job.
        // Read the user's own `compilerOptions.paths` ONCE — used both to
        // filter the virtual-module alias set (user-wins, #1267) and to seed
        // the synthetic tsconfig (#1238). Read only when a plugin registered
        // something, preserving the no-plugin path's "reads nothing /
        // byte-identical" guarantee (see `build_plugin_tsconfig`).
        let user_tsconfig_paths =
            if self.config.alias_entries.is_empty() && self.config.virtual_modules.is_empty() {
                BTreeMap::new()
            } else {
                zfb_plugin_resolver::read_tsconfig_paths_into_map(&self.config.working_dir)
            };

        let resolver_inputs = zfb_plugin_resolver::build_resolver_inputs(
            &self.config.alias_entries,
            &self.config.virtual_modules,
            &self.config.working_dir,
            &user_tsconfig_paths,
            &["zfb"],
        )
        .context("zfb-islands: failed materializing plugin resolver inputs")?;

        // Synthetic tsconfig. Allocated only when there is at least one
        // plugin-registered entry; otherwise the job keeps its native
        // closest-config discovery. When allocated, it seeds the user's own
        // `compilerOptions.paths` first, then merges the plugin entries on top
        // (see `build_plugin_tsconfig`) — an explicit tsconfig would otherwise
        // drop the user's `@/`-style aliases (#1238).
        let plugin_tsconfig = build_plugin_tsconfig(
            &self.config.working_dir,
            &resolver_inputs,
            &user_tsconfig_paths,
            "islands",
            "Synthetic tsconfig generated by zfb-islands::bundle_job. \
             Drives plugin-registered alias / virtual-module \
            exact-match resolution through compilerOptions.paths.",
        )?;
        // Scratch graph file for THIS job's stage-escape audit. The resource
        // contract (`== splitting`) already writes its own `meta.json` when
        // `splitting` is true, so this one is only actually requested in the
        // `splitting == false` case — see `JobShape::stage_audit_metafile`.
        let entry_stage_audit_metafile = stage_audit_metafile(self.config.stage_audit.as_ref())?;
        let mut request = build_bundle_request(
            &self.config,
            config,
            Some(&resolver_inputs),
            plugin_tsconfig.as_ref().map(|file| file.path()),
            JobShape::shared_entry(
                out_dir.path(),
                entry_tmp.path(),
                splitting,
                entry_stage_audit_metafile.as_ref().map(|f| f.path()),
            ),
        )?;
        if identity_probe {
            // A single executable closure avoids chunk hashes derived from
            // random staging paths. Graph edges retain module identity.
            request.splitting = false;
        }
        run_native_entry(
            request,
            entry_tmp.path(),
            self.config.tsconfig_search_boundary.as_deref(),
        )?;

        // Kept before the audits so a failing build still leaves its graph.
        if splitting && !identity_probe {
            keep_debug_islands_metafile(&out_dir.path().join(RESOURCE_METAFILE_FILENAME));
        }

        // Guard (b) wiring (#1705): audit immediately after this job
        // succeeds, before touching its output. The resource-contract pass
        // (`splitting == true`) already wrote `meta.json` above regardless
        // of this policy; every other pass wrote the dedicated
        // `stage_audit_metafile` requested above.
        if let Some(policy) = &self.config.stage_audit {
            let metafile_path = if splitting {
                out_dir.path().join(RESOURCE_METAFILE_FILENAME)
            } else {
                entry_stage_audit_metafile
                    .as_ref()
                    .expect(
                        "entry_stage_audit_metafile is Some whenever config.stage_audit is Some",
                    )
                    .path()
                    .to_path_buf()
            };
            run_stage_escape_audit(policy, &metafile_path, &self.config.working_dir, job_label)?;
        }

        // Entry-import guard (#4464): the splitting pass's `meta.json` must
        // show no output importing the islands entry. The identity probe runs
        // unsplit and never ships, so it is skipped.
        if splitting && !identity_probe {
            crate::entry_import_guard::audit_entry_imports_at_path(
                &out_dir.path().join(RESOURCE_METAFILE_FILENAME),
                &self.config.working_dir,
                entry_tmp.path(),
            )
            .with_context(|| format!("zfb-islands: entry-import audit failed for {job_label}"))?;
        }

        let mut graph_digests = Vec::new();
        if identity_probe {
            graph_digests.push(crate::graph_identity::resolved_graph_digest(
                &out_dir.path().join(RESOURCE_METAFILE_FILENAME),
                &self.config.working_dir,
                entry_tmp.path(),
                &resolver_inputs.paths_entries,
                true,
                &self.identity_source_overrides(&resolver_inputs.paths_entries),
            )?);
        }

        // Issue #1501: `new Worker(new URL(...))` is not discovered as another
        // entry. Run one explicit browser bundle per locked #1500 worker entry
        // into the SAME staging outdir as `islands.js`. Each pass disables
        // splitting so the stable `worker-*.js` companion is self-contained;
        // the shared loaders, defines, mode, JSX policy, plugin aliases, and
        // virtual modules all flow through the same request builder and
        // resolver inputs as the islands entry.
        if splitting && !config.module_workers.is_empty() {
            for worker in &config.module_workers {
                let worker_resolver = build_job_resolver_inputs(
                    &self.config.working_dir,
                    worker.source_path(),
                    self.config.tsconfig_search_boundary.as_deref(),
                    &self.config.alias_entries,
                    &self.config.virtual_modules,
                )?;
                let worker_stage_audit_metafile = if identity_probe {
                    Some(
                        tempfile::NamedTempFile::new()
                            .context("allocate worker identity metafile")?,
                    )
                } else {
                    stage_audit_metafile(self.config.stage_audit.as_ref())?
                };
                let worker_request = build_bundle_request(
                    &self.config,
                    config,
                    Some(&worker_resolver.resolver_inputs),
                    worker_resolver.tsconfig.as_ref().map(|file| file.path()),
                    JobShape::named(
                        out_dir.path(),
                        worker.source_path(),
                        worker.entry_name_template(),
                        worker_stage_audit_metafile.as_ref().map(|f| f.path()),
                    ),
                )?;
                run_native_entry(
                    worker_request,
                    worker.source_path(),
                    self.config.tsconfig_search_boundary.as_deref(),
                )?;

                if identity_probe {
                    graph_digests.push(crate::graph_identity::resolved_graph_digest(
                        worker_stage_audit_metafile
                            .as_ref()
                            .expect("probe metafile")
                            .path(),
                        &self.config.working_dir,
                        worker.source_path(),
                        &worker_resolver.resolver_inputs.paths_entries,
                        false,
                        &self.identity_source_overrides(
                            &worker_resolver.resolver_inputs.paths_entries,
                        ),
                    )?);
                }

                if let Some(policy) = &self.config.stage_audit {
                    let metafile_path = worker_stage_audit_metafile.as_ref().expect(
                        "worker_stage_audit_metafile is Some whenever config.stage_audit is Some",
                    );
                    run_stage_escape_audit(
                        policy,
                        metafile_path.path(),
                        &self.config.working_dir,
                        &format!("shared worker {}", worker.filename()),
                    )?;
                }
            }
        }

        // All jobs are complete. Dropping these deletes the temporary
        // plugin resolver modules/tsconfig only after every worker pass has
        // had a chance to resolve through them.
        drop(resolver_inputs);
        drop(plugin_tsconfig);

        let mut output = if splitting {
            read_back_shared_outdir(
                out_dir.path(),
                &out_dir.path().join(RESOURCE_METAFILE_FILENAME),
                &self.config.working_dir,
            )?
        } else {
            read_back_outdir(out_dir.path())?
        };
        output.graph_digests = graph_digests;
        Ok(output)
    }
}

fn hash_identity_part(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
}

/// Validate caller-supplied worker entries before either the real or mock
/// bundle path consumes them. This keeps both modes aligned and prevents two
/// passes from overwriting the same stable companion inside the shared
/// staging outdir.
fn validate_module_worker_entries(entries: &[ModuleWorkerBundleEntry]) -> Result<()> {
    let mut seen_worker_names = std::collections::BTreeSet::new();
    for worker in entries {
        match validate_chunk_filename(worker.filename())? {
            OutputFilenameClass::Worker => {}
            OutputFilenameClass::Chunk | OutputFilenameClass::Resource => {
                return Err(anyhow!(
                    "module-worker entry unexpectedly used a non-worker filename {:?}",
                    worker.filename()
                ));
            }
        }
        if !seen_worker_names.insert(worker.filename()) {
            return Err(anyhow!(
                "duplicate module-worker output filename {:?}",
                worker.filename()
            ));
        }
        if !worker.source_path().is_file() {
            return Err(anyhow!(
                "module-worker entry source does not exist or is not a file: {}",
                worker.source_path().display()
            ));
        }
    }
    Ok(())
}

/// In-memory result of the shared `bundle_one_entry` pass: the entry JS plus
/// any code-split chunks and independently bundled module workers emitted
/// beside it.
///
/// `chunks` is empty for a zero-dynamic-import entry, always in mock mode,
/// and on non-splitting client-script jobs. The shared-bundle path
/// uses `splitting = true` and threads `chunks`/`workers` into
/// [`BundleOutput`].
#[derive(Debug)]
struct OneEntryOutput {
    graph_digests: Vec<[u8; 32]>,
    js: String,
    chunks: Vec<BundleChunk>,
    workers: Vec<BundleChunk>,
    resources: Vec<BundleResource>,
}

impl OneEntryOutput {
    /// Entry-only output with no chunks (mock mode + the read-back path's
    /// zero-dynamic-import case).
    fn js_only(js: String) -> Self {
        Self {
            graph_digests: Vec::new(),
            js,
            chunks: Vec::new(),
            workers: Vec::new(),
            resources: Vec::new(),
        }
    }
}

/// Filename prefix of the synthesized entry temp file when it is allocated
/// **inside `working_dir`** (the production placement — see
/// [`IslandsBundler::bundle_one_entry`] for why it cannot live in
/// `$TMPDIR`). Leading dot so the project's own tooling treats it as hidden.
const IN_PROJECT_ENTRY_PREFIX: &str = ".zfb-esbuild-entry-";

/// Suffix of the synthesized entry temp file. `.tsx` so esbuild's loader
/// inference picks up JSX automatically.
const IN_PROJECT_ENTRY_SUFFIX: &str = ".tsx";

/// How long an in-project entry file must have been untouched before the
/// sweep is willing to treat it as abandoned.
///
/// The entry is written once and then only read (by the esbuild subprocess),
/// so a *live* entry's mtime is at most the duration of one
/// `bundle_one_entry` call — seconds for a normal islands set, and still well
/// under a minute for the largest real projects (esbuild itself is the fast
/// part of a build). Thirty minutes is therefore a ~100x margin against
/// deleting an entry that a **concurrent** zfb process in the same project
/// root is still feeding to esbuild — the case that matters, because running
/// `zfb dev` and `zfb build` against one project at the same time is normal.
/// Erring long is nearly free: an orphan lingers for one extra build cycle
/// instead of forever.
const ORPHANED_ENTRY_GRACE: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Filename prefix of the synthetic per-job islands tsconfig
/// [`build_plugin_tsconfig`] writes when called with `label = "islands"` —
/// the crate's only call site for that function. Mirrors that function's own
/// `format!(".zfb-{label}-tsconfig-")`; kept as a literal constant (rather
/// than derived from the label at compile time) because that call site is
/// the sole producer, and the pairing is pinned by
/// `islands_tsconfig_is_locked_so_a_concurrent_exclusive_lock_attempt_fails_while_owned`,
/// which asserts the produced filename actually starts with this prefix.
const ISLANDS_TSCONFIG_PREFIX: &str = ".zfb-islands-tsconfig-";

/// Filename prefix of the synthetic per-job tsconfig
/// [`build_job_resolver_inputs`] writes for both module-worker and
/// client-script entries (there is no separate client-script class — see
/// that function's own comment).
const WORKER_TSCONFIG_PREFIX: &str = ".zfb-worker-tsconfig-";

/// Suffix shared by both synthetic tsconfig classes above.
const TSCONFIG_TEMP_SUFFIX: &str = ".json";

/// Every in-project `.zfb-*` temp-file class the sweep recognizes, as
/// `(prefix, suffix)` pairs. Each class is allocated **inside `working_dir`**
/// (the real project root — `zfb-islands` never shadow-copies the project)
/// and locked with a *shared* `flock`
/// by its producer, so the age+lock contract documented on
/// [`sweep_orphaned_in_project_temp_files`] applies uniformly to all four:
///
/// - the synthesized esbuild entry ([`allocate_locked_entry_tmp`])
/// - the islands-entry synthetic tsconfig ([`build_plugin_tsconfig`])
/// - the worker/client-script synthetic tsconfig ([`build_job_resolver_inputs`])
/// - the virtual-module materialization (`zfb_plugin_resolver::build_resolver_inputs`,
///   locked by that crate — its shadow-side counterpart in `zfb-build`'s SSR
///   bundler is a different producer writing into a different, already
///   separately-swept `TempDir`, and stays out of this table)
const IN_PROJECT_TEMP_CLASSES: &[(&str, &str)] = &[
    (IN_PROJECT_ENTRY_PREFIX, IN_PROJECT_ENTRY_SUFFIX),
    (ISLANDS_TSCONFIG_PREFIX, TSCONFIG_TEMP_SUFFIX),
    (WORKER_TSCONFIG_PREFIX, TSCONFIG_TEMP_SUFFIX),
    (
        zfb_plugin_resolver::VIRTUAL_MODULE_TEMP_PREFIX,
        zfb_plugin_resolver::VIRTUAL_MODULE_TEMP_SUFFIX,
    ),
];

/// Delete every recognized in-project `.zfb-*` temp-file class
/// ([`IN_PROJECT_TEMP_CLASSES`]) left in `working_dir` by an earlier zfb
/// process that terminated without unwinding (issue #1970 / #1976 for the
/// original entry-only class; generalized to the other three classes by
/// issue #2109).
///
/// **Why this is needed at all.** Every class here is a `tempfile::NamedTempFile`
/// whose `Drop` removes it, and every producer is synchronous with a
/// locally-owned handle, so the success path *and* every `?` error path
/// already clean up (proved by `working_dir_is_clean_after_multi_entry_bundle`
/// in `tests/integration.rs`). What `Drop` cannot cover is termination that
/// never unwinds the stack: a `SIGTERM`/`SIGKILL`ed `zfb dev` session or an
/// interrupted `zfb build` strands the file beside the project, and until
/// this sweep nothing ever removed it. Because none of these files is covered
/// by any `.gitignore`, one then surfaces as an untracked file in `git status`
/// after the *next*, entirely successful build — which is exactly how #1970
/// was reported (two entries present, only the live build's own one removed).
///
/// **A candidate must clear two independent liveness checks**, and either one
/// alone is enough to spare it:
///
/// 1. Its mtime must be older than [`ORPHANED_ENTRY_GRACE`]. Every class here
///    is written once and then only read, so a live file's mtime is at most
///    the duration of one esbuild invocation.
/// 2. An **exclusive** lock on it must be available. Every producer holds a
///    *shared* lock on its own file until it returns, so a candidate we can
///    lock exclusively is provably unowned. This covers what age cannot: a
///    bundling process suspended or stuck inside a single esbuild invocation
///    for longer than the grace period still holds its lock and is left
///    alone.
///
/// The lock is shared rather than exclusive on the owner's side because
/// esbuild has to read the file, and on Windows an exclusive lock would deny
/// it that read. Locks also survive nothing: the OS releases them when the
/// owning process dies, which is precisely the case this sweep handles.
///
/// Best-effort throughout: this is opportunistic hygiene, never a correctness
/// step, so every I/O failure — including a filesystem with no lock support —
/// simply leaves the candidate alone.
/// Allocate the esbuild entry temp file for one
/// [`IslandsBundler::bundle_one_entry`] call, write `entry_source`
/// into it, and claim it with a **shared** lock.
///
/// Split out of `bundle_one_entry` so the ownership claim — the production
/// half of the sweep's safety contract — is reachable from a unit test
/// without spawning esbuild (see
/// `allocated_entry_is_locked_so_a_concurrent_sweep_spares_it`).
///
/// The entry is allocated inside `working_dir` whenever that exists, so
/// esbuild's upward `node_modules` walk starts beside the project (see the
/// long note at the call site); otherwise it falls back to the system
/// tempdir, which only the `zfb-islands` unit tests take.
///
/// The write happens **before** the lock, and must stay that way: on Windows
/// a shared `LockFileEx` byte-range lock denies writes to the locked range,
/// so locking first would make the `std::fs::write` below fail with a lock
/// violation on every real Windows bundle.
///
/// The lock lets a concurrent process's sweep prove the entry is still live
/// rather than inferring it from age alone (see
/// [`sweep_orphaned_in_project_temp_files`]). It is *shared*, not exclusive:
/// esbuild only reads the entry, and on Windows an exclusive lock would deny
/// it that read. It is released when the returned handle drops — including
/// when the OS closes the handle for a process that never unwinds, which is
/// exactly the case the sweep exists for. Best-effort: on a filesystem
/// without lock support the sweep simply falls back to its age gate.
fn allocate_locked_entry_tmp(
    working_dir: &Path,
    entry_source: &str,
) -> Result<tempfile::NamedTempFile> {
    let entry_tmp = if working_dir.is_dir() {
        tempfile::Builder::new()
            .prefix(IN_PROJECT_ENTRY_PREFIX)
            .suffix(IN_PROJECT_ENTRY_SUFFIX)
            .tempfile_in(working_dir)
            .context("failed to allocate entry temp file inside working_dir")?
    } else {
        tempfile::Builder::new()
            .prefix("zfb-esbuild-entry-")
            .suffix(IN_PROJECT_ENTRY_SUFFIX)
            .tempfile()
            .context("failed to allocate entry temp file")?
    };
    std::fs::write(entry_tmp.path(), entry_source.as_bytes())
        .context("failed to write entry temp file")?;
    let _ = entry_tmp.as_file().lock_shared();
    Ok(entry_tmp)
}

/// Does `name` (a bare filename, no directory components) match one of the
/// recognized in-project `.zfb-*` temp-file classes ([`IN_PROJECT_TEMP_CLASSES`])?
/// Shared by the orphan sweep above and [`is_zfb_islands_temp_file`] below so
/// the two can never drift apart.
fn matches_in_project_temp_class(name: &str) -> bool {
    IN_PROJECT_TEMP_CLASSES
        .iter()
        .any(|(prefix, suffix)| name.starts_with(prefix) && name.ends_with(suffix))
}

/// Does `path` name one of zfb-islands' own in-project temp files
/// ([`IN_PROJECT_TEMP_CLASSES`]) — the synthesized esbuild entry, either
/// synthetic tsconfig class, or the virtual-module materialization?
///
/// The dev watcher's intake suppression consumes this (issue #3215): every
/// recognized class is allocated inside `working_dir`, which is the watched
/// project root (see [`allocate_locked_entry_tmp`]'s doc comment for why),
/// so without suppression the `Created`/`Removed` events re-trigger a second
/// full watcher tick after every islands rebuild. Basename-only matching uses
/// the same prefix/suffix shape as the sweep's own predicate
/// (`matches_in_project_temp_class`). These classes are dot-prefixed and
/// zfb-namespaced, so a user file colliding with one is not a realistic
/// concern.
pub fn is_zfb_islands_temp_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    matches_in_project_temp_class(name)
}

fn sweep_orphaned_in_project_temp_files(working_dir: &Path) {
    let Ok(dir) = std::fs::read_dir(working_dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for dirent in dir.flatten() {
        let name = dirent.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !matches_in_project_temp_class(name) {
            continue;
        }
        let Ok(metadata) = dirent.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let stale = metadata
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= ORPHANED_ENTRY_GRACE);
        if !stale {
            continue;
        }
        let path = dirent.path();
        // Rust's `OpenOptions` shares delete access, so holding this handle
        // does not stop the `remove_file` below on Windows.
        let Ok(candidate) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
        else {
            continue;
        };
        if zfb_plugin_resolver::exclusive_lock_on_file_at_path(&candidate, &path) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Read every file esbuild staged into `out_dir` back into memory without a
/// resource oracle. This is retained for the per-island/runtime path, whose
/// output policy deliberately remains entry/chunk/worker-only.
fn read_back_outdir(out_dir: &Path) -> Result<OneEntryOutput> {
    read_back_outdir_with_resource_oracle(out_dir, None, None, None)
}

/// Read the shared browser-islands staging directory with its mandatory
/// esbuild metafile. The metafile is an oracle for every accepted resource;
/// it is not a published output and is skipped during output classification.
fn read_back_shared_outdir(
    out_dir: &Path,
    metafile_path: &Path,
    metafile_working_dir: &Path,
) -> Result<OneEntryOutput> {
    let metafile = read_resource_metafile(metafile_path)?;
    read_back_outdir_with_resource_oracle(
        out_dir,
        Some(&metafile),
        Some(metafile_path),
        Some(metafile_working_dir),
    )
}

/// Minimal strict view of the esbuild metafile needed to prove file-loader
/// outputs. `outputs[output-path].inputs` maps source input paths to metadata;
/// only its keys are material here, but requiring this structured shape makes
/// malformed or missing metafiles fail closed.
#[derive(Debug, Deserialize)]
struct ResourceMetafile {
    outputs: BTreeMap<String, ResourceMetafileOutput>,
}

#[derive(Debug, Deserialize)]
struct ResourceMetafileOutput {
    #[serde(default)]
    inputs: BTreeMap<String, serde_json::Value>,
}

fn read_resource_metafile(metafile_path: &Path) -> Result<ResourceMetafile> {
    let bytes = std::fs::read(metafile_path).with_context(|| {
        format!(
            "resource discovery requires metafile {}",
            metafile_path.display()
        )
    })?;
    serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "resource discovery could not parse metafile {}",
            metafile_path.display()
        )
    })
}

/// Read every file esbuild staged into `out_dir` back into memory, split into
/// the stable entry (`islands.js`), self-hashed chunks, stable module workers,
/// and (for the shared pass only) metafile-backed resources.
///
/// Contract enforced here (the read-back is the trust boundary for the
/// `BundleOutput` companion shape downstream consumers rely on):
///
/// - Exactly one entry named [`STABLE_ISLANDS_FILENAME`] must exist.
/// - Every other output is a regular, flat file and belongs to exactly one
///   reserved companion class: chunk, worker, or locked resource. Resources
///   further require an exact metafile output key and a matching source suffix
///   edge. Anything else is rejected rather than silently shipped.
/// - Recognised sourcemap siblings (`*.js.map`) are ignored on read-back,
///   matching the pre-splitting behaviour (the old single-file path requested
///   `--sourcemap=linked` but only ever read the `.js` back). Keeping that
///   exact behaviour means enabling splitting introduces zero new bytes for
///   the non-splitting case.
fn read_back_outdir_with_resource_oracle(
    out_dir: &Path,
    metafile: Option<&ResourceMetafile>,
    metafile_path: Option<&Path>,
    metafile_working_dir: Option<&Path>,
) -> Result<OneEntryOutput> {
    let mut entry_js: Option<String> = None;
    let mut chunks: Vec<BundleChunk> = Vec::new();
    let mut workers: Vec<BundleChunk> = Vec::new();
    let mut resources: Vec<BundleResource> = Vec::new();
    let mut output_names = BTreeSet::new();

    for dirent in std::fs::read_dir(out_dir)
        .with_context(|| format!("failed to read staging outdir {}", out_dir.display()))?
    {
        let dirent = dirent.context("failed to read staging outdir entry")?;
        if metafile_path
            .map(|path| path == dirent.path().as_path())
            .unwrap_or(false)
        {
            // `--metafile` writes this private read-back oracle beside the
            // actual staging outputs. It must never enter the resource list.
            continue;
        }
        if !dirent.file_type().map(|t| t.is_file()).unwrap_or(false) {
            return Err(anyhow!(
                "the bundler emitted an unexpected non-file output entry {:?} in {}",
                dirent.file_name(),
                out_dir.display()
            ));
        }
        let os_name = dirent.file_name();
        let name = os_name.to_str().ok_or_else(|| {
            anyhow!(
                "the bundler emitted a non-UTF-8 filename in {}",
                out_dir.display()
            )
        })?;

        // Ignore only sourcemap siblings of a recognised JS output (see fn
        // doc) — never shipped today. An arbitrary `sneaky.js.map` must not
        // bypass the same filename trust boundary its hypothetical JS peer
        // would fail.
        if let Some(mapped_js) = name.strip_suffix(".map") {
            if !mapped_js.ends_with(".js") {
                return Err(anyhow!(
                    "esbuild emitted an unexpected sourcemap file {name:?}; \
                     expected a recognised `.js.map` sibling"
                ));
            }
            if mapped_js != zfb_types::STABLE_ISLANDS_FILENAME {
                validate_chunk_filename(mapped_js)?;
            }
            continue;
        }

        if name == zfb_types::STABLE_ISLANDS_FILENAME {
            register_output_name(&mut output_names, name)?;
            let js =
                std::fs::read_to_string(dirent.path()).context("failed to read entry output")?;
            if entry_js.is_some() {
                return Err(anyhow!(
                    "the bundler emitted duplicate entry output {name:?} in {}",
                    out_dir.display()
                ));
            }
            entry_js = Some(js);
            continue;
        }

        // Every companion is validated before its bytes are read or exposed to
        // a downstream verbatim writer. `register_output_name` keeps all
        // classes in one namespace even though the filesystem normally cannot
        // present duplicate directory entries.
        let class = validate_chunk_filename(name)?;
        register_output_name(&mut output_names, name)?;
        let bytes = std::fs::read(dirent.path())
            .with_context(|| format!("failed to read companion {name}"))?;
        match class {
            OutputFilenameClass::Chunk => chunks.push(BundleChunk {
                filename: name.to_string(),
                bytes,
            }),
            OutputFilenameClass::Worker => workers.push(BundleChunk {
                filename: name.to_string(),
                bytes,
            }),
            OutputFilenameClass::Resource => {
                let metafile = metafile.ok_or_else(|| {
                    anyhow!(
                        "the bundler emitted resource {name:?} without the shared-islands metafile oracle"
                    )
                })?;
                let metafile_working_dir = metafile_working_dir.ok_or_else(|| {
                    anyhow!(
                        "the bundler emitted resource {name:?} without the shared-islands metafile working-directory context"
                    )
                })?;
                validate_resource_metafile_edge(
                    metafile,
                    &dirent.path(),
                    name,
                    metafile_working_dir,
                )?;
                resources.push(BundleResource {
                    filename: name.to_string(),
                    bytes,
                });
            }
        }
    }

    let js = entry_js.ok_or_else(|| {
        anyhow!(
            "the bundler produced no `{}` entry in {}",
            zfb_types::STABLE_ISLANDS_FILENAME,
            out_dir.display()
        )
    })?;

    // Sort chunks by filename for a deterministic `BundleOutput.chunks`
    // order (directory-iteration order is filesystem-dependent). The chunk
    // *names* are already content-stable across rebuilds; sorting makes the
    // Vec order stable too.
    chunks.sort_by(|a, b| a.filename.cmp(&b.filename));
    workers.sort_by(|a, b| a.filename.cmp(&b.filename));
    resources.sort_by(|a, b| a.filename.cmp(&b.filename));

    Ok(OneEntryOutput {
        graph_digests: Vec::new(),
        js,
        chunks,
        workers,
        resources,
    })
}

fn register_output_name(names: &mut BTreeSet<String>, name: &str) -> Result<()> {
    if !names.insert(name.to_string()) {
        return Err(anyhow!(
            "the bundler emitted duplicate or cross-class-colliding output filename {name:?}"
        ));
    }
    Ok(())
}

/// Prove that a resource read from the staging directory is the precise
/// file-loader output esbuild recorded for a matching marked glue or wasm
/// source. Filename validation establishes the expected final extension;
/// this oracle then ties it to the matching source edge rather than accepting
/// an arbitrary flat file merely because it has the reserved prefix.
fn validate_resource_metafile_edge(
    metafile: &ResourceMetafile,
    resource_path: &Path,
    resource_filename: &str,
    metafile_working_dir: &Path,
) -> Result<()> {
    let expected_source_suffix = if resource_filename.ends_with(".mjs") {
        ZFB_RESOURCE_MJS_EXTENSION
    } else if resource_filename.ends_with(WASM_EXTENSION) {
        WASM_EXTENSION
    } else {
        return Err(anyhow!(
            "resource filename {resource_filename:?} has no supported final extension"
        ));
    };

    let canonical_resource_path = std::fs::canonicalize(resource_path).with_context(|| {
        format!(
            "could not canonicalize staged resource output {}",
            resource_path.display()
        )
    })?;
    let output = metafile
        .outputs
        .iter()
        .find_map(|(output_path, output)| {
            metafile_output_path(output_path, metafile_working_dir)
                .as_ref()
                .is_ok_and(|path| path == &canonical_resource_path)
                .then_some(output)
        })
        .ok_or_else(|| {
            anyhow!(
                "resource output {resource_filename:?} is not backed by an exact metafile output path {}",
                resource_path.display()
            )
        })?;

    if !output
        .inputs
        .keys()
        .any(|input_path| metafile_input_matches_resource(input_path, expected_source_suffix))
    {
        return Err(anyhow!(
            "resource output {resource_filename:?} has no metafile file-loader edge from a source ending in {expected_source_suffix:?}"
        ));
    }
    Ok(())
}

/// esbuild preserves an import's resource query in metafile input keys. The
/// browser package uses the explicit `?url` contract shared with Vite, so
/// recognize that one exact query in addition to the legacy bare file-loader
/// edge. Do not generally strip queries: an unrelated loader/plugin input must
/// not satisfy the fail-closed resource oracle.
fn metafile_input_matches_resource(input_path: &str, expected_source_suffix: &str) -> bool {
    input_path.ends_with(expected_source_suffix)
        || input_path
            .strip_suffix("?url")
            .is_some_and(|path| path.ends_with(expected_source_suffix))
}

/// Metafile paths are relative to esbuild's configured working directory by
/// default. Resolve and canonicalize each key before comparing it with the
/// regular staging output we just read, so the comparison stays exact without
/// relying on a non-contract `--abs-paths=metafile` flag.
pub(crate) fn metafile_output_path(output_path: &str, working_dir: &Path) -> Result<PathBuf> {
    let path = Path::new(output_path);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        working_dir.join(path)
    };
    std::fs::canonicalize(&resolved).with_context(|| {
        format!(
            "could not canonicalize module-graph output path {output_path:?} resolved from {}",
            working_dir.display()
        )
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFilenameClass {
    Chunk,
    Worker,
    Resource,
}

/// Validate a discovered companion filename against the strict output
/// contract: it must be a flat basename (no path separators, no `..`
/// traversal segment) and belong to one reserved companion class. Returns the
/// recognised class, or an error describing the rejected name.
fn validate_chunk_filename(name: &str) -> Result<OutputFilenameClass> {
    if name.contains('/') || name.contains('\\') || name.contains(std::path::MAIN_SEPARATOR) {
        return Err(anyhow!(
            "companion filename must be flat (no path separator): {name:?}"
        ));
    }
    // A legitimate self-hashed chunk basename never contains `..`; reject the
    // substring outright (defence-in-depth against traversal, belt-and-braces
    // with the separator check above).
    if name.contains("..") {
        return Err(anyhow!(
            "companion filename must not contain `..`: {name:?}"
        ));
    }
    // Issue #1395: a residual `.css` output here means the `--loader:.css=empty`
    // policy in `build_esbuild_args_with_entry_name` was bypassed or an
    // unexpected esbuild version emitted a sibling CSS file anyway. Give a
    // targeted message pointing at the zfb CSS pipeline instead of the
    // generic "unexpected output file" wording, which reads as an esbuild
    // internals bug rather than a documented policy.
    if name.ends_with(".css") {
        return Err(anyhow!(
            "esbuild emitted a CSS output file {name:?} from an island bundle; \
             CSS imports in islands are handled by the zfb CSS pipeline \
             (`styles-<hash>.css`), not shipped as a separate islands build \
             artifact — see the `--loader:.css=empty` policy in \
             build_esbuild_args_with_entry_name"
        ));
    }
    if let Some(hash) = name
        .strip_prefix(CHUNK_FILENAME_PREFIX)
        .and_then(|rest| rest.strip_suffix(".js"))
    {
        if !hash.is_empty()
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Ok(OutputFilenameClass::Chunk);
        }
    }
    if let Some(encoded_path) = name
        .strip_prefix(WORKER_ENTRY_NAME_PREFIX)
        .and_then(|rest| rest.strip_suffix(".js"))
    {
        if !encoded_path.is_empty()
            && encoded_path
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Ok(OutputFilenameClass::Worker);
        }
    }
    if let Some((resource_name, hash)) = resource_name_and_hash(name) {
        if !resource_name.is_empty()
            && resource_name
                .chars()
                .all(|ch| !ch.is_control() && ch != '/' && ch != '\\')
            && !hash.is_empty()
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Ok(OutputFilenameClass::Resource);
        }
    }
    Err(anyhow!(
        "esbuild emitted an unexpected output file {name:?}; expected the entry `{}`, \
         a `{CHUNK_FILENAME_PREFIX}*` chunk, or a \
         `{WORKER_ENTRY_NAME_PREFIX}*.js` module worker, or a \
         `{RESOURCE_FILENAME_PREFIX}*-<hash>.mjs|.wasm` resource",
        zfb_types::STABLE_ISLANDS_FILENAME
    ))
}

/// Return the source-derived `[name]` and esbuild `[hash]` portions of a
/// locked resource output, provided it has exactly one supported final
/// extension and the reserved prefix. The caller performs character policy
/// checks, so this helper stays narrowly about the naming template shape.
fn resource_name_and_hash(name: &str) -> Option<(&str, &str)> {
    let stem = name
        .strip_prefix(RESOURCE_FILENAME_PREFIX)?
        .strip_suffix(".mjs")
        .or_else(|| {
            name.strip_prefix(RESOURCE_FILENAME_PREFIX)?
                .strip_suffix(WASM_EXTENSION)
        })?;
    stem.rsplit_once('-')
}

/// Reject attempts to override the two loader keys that make shared browser
/// resources safely discoverable. The command layer normally rejects these
/// too, but this crate owns the final `BundleConfig` boundary and therefore
/// cannot rely on every direct Rust caller having passed through that layer.
fn validate_locked_resource_loader_overrides(loaders: &BTreeMap<String, String>) -> Result<()> {
    for extension in [ZFB_RESOURCE_MJS_EXTENSION, WASM_EXTENSION] {
        if let Some(loader) = loaders.get(extension) {
            return Err(anyhow!(
                "shared browser-islands loader key {extension:?} is reserved by zfb-islands for the locked file resource contract and cannot be overridden (got {loader:?})"
            ));
        }
    }
    Ok(())
}

/// The per-job half of a bundle request: where the job reads and writes, and
/// which output contract it owns.
#[derive(Debug, Clone, Copy)]
pub(crate) struct JobShape<'a> {
    /// Private staging directory every output is written into.
    pub(crate) out_dir: &'a Path,
    pub(crate) entry_path: &'a Path,
    /// Code splitting; only the shared islands pass ships chunks.
    pub(crate) splitting: bool,
    /// Entry file pattern without extension (`islands`, a worker name, a
    /// client-script name).
    pub(crate) entry_name_template: &'a str,
    /// The locked file-resource contract plus its `meta.json` graph oracle;
    /// owned by the shared splitting islands pass only.
    pub(crate) resource_contract: bool,
    /// Graph file requested for the stage-escape audit. Honoured only
    /// without the resource contract, whose own `meta.json` already is the
    /// graph the audit reads.
    pub(crate) stage_audit_metafile: Option<&'a Path>,
}

impl<'a> JobShape<'a> {
    /// The shared islands entry: stable `islands.js`, with the resource
    /// contract exactly when splitting.
    pub(crate) fn shared_entry(
        out_dir: &'a Path,
        entry_path: &'a Path,
        splitting: bool,
        stage_audit_metafile: Option<&'a Path>,
    ) -> Self {
        Self {
            out_dir,
            entry_path,
            splitting,
            entry_name_template: ENTRY_NAME_TEMPLATE,
            resource_contract: splitting,
            stage_audit_metafile,
        }
    }

    /// A self-contained module-worker or client-script job named
    /// `<entry_name_template>.js`.
    pub(crate) fn named(
        out_dir: &'a Path,
        entry_path: &'a Path,
        entry_name_template: &'a str,
        stage_audit_metafile: Option<&'a Path>,
    ) -> Self {
        Self {
            out_dir,
            entry_path,
            splitting: false,
            entry_name_template,
            resource_contract: false,
            stage_audit_metafile,
        }
    }
}

/// Compose the typed bundle request for one job. Pure: it reads only its
/// arguments (and the inherited `NODE_PATH` when the config sets none), so
/// tests can assert the request without bundling.
///
/// Every job is a browser ESM bundle with automatic JSX on the single owned
/// import source; without that, the classic transform emits unresolved React
/// references (#151). `resolver` supplies the plugin aliases (virtual modules
/// first, then single-file plugin aliases) placed ahead of the built-in `zfb`
/// SDK alias; `tsconfig` is the job's explicit (synthetic) tsconfig, if any.
pub(crate) fn build_bundle_request(
    bundler: &IslandsBundlerConfig,
    config: &BundleConfig,
    resolver: Option<&zfb_plugin_resolver::ResolverInputs>,
    tsconfig: Option<&Path>,
    job: JobShape<'_>,
) -> Result<BundleRequest> {
    // The entry-name template must produce exactly `STABLE_ISLANDS_FILENAME`
    // (the bundler appends `.js`), or the read-back won't recognise the entry.
    debug_assert_eq!(
        format!("{ENTRY_NAME_TEMPLATE}.js"),
        zfb_types::STABLE_ISLANDS_FILENAME,
        "entry-name template must match STABLE_ISLANDS_FILENAME stem"
    );
    // Directory-mode output into the *staging* `out_dir` (a throwaway
    // tempdir — NOT `dist/assets/`). The entry pattern pins the entry's
    // basename so the read-back can identify it by name. The chunk pattern
    // (splitting only) self-content-hashes each chunk FLAT in the same dir
    // (`islands-chunk-<hash>.js`); the bundler bakes relative `import("./...")`
    // references between them, so names must never be renamed downstream. See
    // `ENTRY_NAME_TEMPLATE` / `CHUNK_NAME_TEMPLATE` and the
    // `BundleOutput::chunks` contract for the full rationale.
    let output = OutputLayout::Dir {
        dir: job.out_dir.to_path_buf(),
        entry_names: Some(job.entry_name_template.to_string()),
        chunk_names: job.splitting.then(|| CHUNK_NAME_TEMPLATE.to_string()),
        asset_names: job
            .resource_contract
            .then(|| RESOURCE_NAME_TEMPLATE.to_string()),
    };
    let mut request = BundleRequest::new(
        &bundler.working_dir,
        job.entry_path,
        output,
        Platform::Browser,
    );
    // Code-splitting is enabled ONLY for the shared-bundle path. With
    // splitting on, the bundler emits the entry plus one chunk per shared /
    // dynamically-imported module graph, so a consumer's `import("shiki")`
    // lands in its own chunk instead of being inlined into the multi-MB entry
    // every page loads (issue #800). Client-script and worker jobs inline
    // their dynamic imports so the single output file remains self-contained.
    request.splitting = job.splitting;
    request
        .externals
        .push(External::Prefix("node:".to_string()));
    request.externals.extend(bundler.externals.iter().cloned());

    // Issue #1395 + #1410: an island importing `.css` / `.module.css` must
    // not hard-fail the islands bundle and must not emit a sibling CSS output
    // the read-back would reject. Both map to the empty loader (mirroring the
    // SSR bundler); `.module.css` needs its own rule so no scoped-CSS loader
    // claims it. Authored CSS is shipped by the zfb CSS pipeline instead.
    let mut loaders = BTreeMap::from([
        (".css".to_string(), Loader::Empty),
        (".module.css".to_string(), Loader::Empty),
    ]);
    for (extension, loader) in &config.loaders {
        let parsed =
            Loader::parse(loader).with_context(|| format!("Unknown module type: {loader}"))?;
        loaders.insert(extension.clone(), parsed);
    }
    if job.resource_contract {
        // Locked after every configured loader so the shared browser-islands
        // contract cannot be weakened by a later override. The explicit
        // `.zfb-resource.mjs` rule is more specific than `.mjs`, preserving
        // the copied glue as JavaScript while ordinary `.mjs` modules remain
        // bundle inputs.
        loaders.insert(ZFB_RESOURCE_MJS_EXTENSION.to_string(), Loader::File);
        loaders.insert(WASM_EXTENSION.to_string(), Loader::File);
    }
    request.loaders = loaders.into_iter().collect();

    request.jsx.runtime = Some(JsxRuntime::Automatic);
    request.jsx.import_source = Some(zfb_types::owned_runtime::JSX_IMPORT_SOURCE.to_string());
    request.keep_names = true;
    if let Some(resolver) = resolver {
        request
            .resolve
            .aliases
            .extend(resolver.virtual_module_alias_args.iter().cloned());
        request
            .resolve
            .aliases
            .extend(resolver.plugin_alias_args.iter().cloned());
    }
    // Consumer code uses the public SDK through the bare `zfb` namespace
    // (`zfb/content`, `zfb/config`, `zfb/zudo-react`, …), while the published
    // npm package is named `@takazudo/zfb`. The alias applies to exact
    // imports and subpaths, including imports from node_modules entries where
    // the synthetic tsconfig paths are ignored. Both resolver-input call sites
    // reserve `zfb` so a plugin alias cannot shadow this mapping (#3934).
    request
        .resolve
        .aliases
        .push(("zfb".to_string(), "@takazudo/zfb".to_string()));
    request.minify = config.minify;
    if config.sourcemap {
        request.source_map = zfb_rolldown_prototype::SourceMap::Linked;
    }
    // Issue #1404/#1413: keep resolution anchored at each importer's on-disk
    // location when the islands shadow uses symlink mode. Copy-mode shadows
    // leave it off because their source files are real shadow copies.
    request.resolve.preserve_symlinks = config.preserve_symlinks;
    request.resolve.conditions = bundler.conditions.clone();
    request.resolve.extra_module_dirs = bundler.extra_module_dirs();

    // Inline NODE_ENV so framework packages pick their mode-specific build.
    // Mode is intentionally independent from minification. Define values are
    // JS expressions, hence the embedded quotes.
    let prod = config.mode.is_prod();
    let node_env = if prod { "production" } else { "development" };
    request.defines.push((
        "process.env.NODE_ENV".to_string(),
        format!("\"{node_env}\""),
    ));
    // Inline `import.meta.env.{PROD,DEV}` so consumer `'use client'` code
    // that references either expression is folded at bundle time instead of
    // shipping `import.meta.env`, which is `undefined` in the browser and
    // throws before any island can hydrate (issue #287). The SSR bundler
    // defines the same pair.
    request
        .defines
        .push(("import.meta.env.PROD".to_string(), prod.to_string()));
    request
        .defines
        .push(("import.meta.env.DEV".to_string(), (!prod).to_string()));
    for (key, value) in &config.define {
        request.defines.push((key.clone(), value.clone()));
    }

    request.graph = if job.resource_contract {
        Some(job.out_dir.join(RESOURCE_METAFILE_FILENAME))
    } else {
        job.stage_audit_metafile.map(Path::to_path_buf)
    };
    request.tsconfig = tsconfig.map(Path::to_path_buf);
    Ok(request)
}

/// Generate the synthetic single-entry source for the owned shared bundle.
///
/// When `client_router` is `true`, a side-effect
/// `import "@takazudo/zfb-runtime/client-router";` is prepended so the
/// `<ClientRouter />` View Transitions runtime ships in the bundle
/// (issue #289). When `false`, nothing extra is emitted and the output
/// is byte-identical to a pre-#289 build. This is the only knob that
/// makes a zero-island slice produce non-empty JS.
///
/// The shared bundle's contract is "every island module's source code
/// is present in the islands JS asset so the runtime's DOM walk can
/// find each `data-zfb-island="…"` element's component bundled into the
/// page". Producing one synthetic entry sidesteps esbuild's
/// multi-input-with-`--outfile` restriction (issue #138).
///
/// # Why we call `mountIslands` from the entry
///
/// Bundling every island's source is necessary but not sufficient:
/// without a top-level call into the hydration runtime, the SSR'd
/// `data-zfb-island` markers stay un-hydrated and interactivity never
/// activates (issue #146 / zudolab/zudo-doc#1355 Wave 6). The
/// shared-bundle entry calls `mountIslands(MANIFEST)`, so one script supplies
/// the hydration glue and island source code in one HTTP
/// request.
///
/// The synthesised entry imports each island as a namespace and
/// **registers** it into a manifest at top-level using a
/// `__zfb_register(ns, exportName, markerName, moduleLabel)` helper: the
/// helper picks the exact scanner-selected export (`__zfb_pick`) and
/// stashes a mount thunk under `markerName`.
///
/// `markerName` is the scanner-derived SSR marker name baked into the source.
/// Registration checks the component's runtime name against that literal,
/// and uses the literal as the manifest key. The server boundary checks the
/// same name against scanner metadata before writing the wrapper.
///
/// The runtime's `mountIslands` accepts this object shape directly
/// (no second dynamic import) — see `IslandManifestValue` in
/// `packages/zfb/src/runtime.ts`. Each `mount` thunk builds an owned
/// description and returns the RootHandle from `hydrate` or `mount`.
///
/// # Why namespace imports
///
/// A naive `import "<path>";` shape (the original #138 fix) is a
/// **side-effect-only import**. esbuild runs with
/// `--bundle --tree-shaking=true`, which only retains code that
/// produces a top-level side effect or that the entry references by
/// name. Islands authored as a bare
/// `export default function ComponentName(...) {}` with no top-level
/// side-effecting statement get **tree-shaken out** (issue #144 /
/// zudolab/zudo-doc#1355 Wave 5). Namespace imports keep every export
/// reachable by name, AND each namespace is referenced from the
/// `__zfb_register(...)` calls — which are themselves top-level side
/// effects esbuild MUST preserve. So tree-shaking remains defanged in
/// the new shape too.
///
/// Sequential numeric identifiers avoid collision pitfalls when two
/// islands share a base name (e.g. host `theme-toggle.tsx` and v2
/// `theme/theme-toggle.tsx`) — every binding is unique by construction.
///
/// Each physical island import path, module label, and component name is
/// JSON-encoded to defang stray quotes / backslashes from paths on Windows or
/// arbitrary file names. The order follows the input slice so the resulting
/// bytes are deterministic for a given `(islands, config)` pair (downstream
/// tests rely on this for the bundle's content hash to be stable across runs).
///
/// Deterministic source-shape helper for tests and tooling. This uses a fixed
/// synthetic build identity; shipping callers must use [`ClientBundler::bundle`],
/// which validates the real owned identity before invoking esbuild.
pub fn render_shared_bundle_entry_source(islands: &[Island], client_router: bool) -> String {
    render_shared_bundle_entry_source_with_build(
        islands,
        client_router,
        Some("0123456789abcdef"),
        &BTreeMap::new(),
    )
}

fn render_shared_bundle_entry_source_with_build(
    islands: &[Island],
    client_router: bool,
    build: Option<&str>,
    module_labels: &BTreeMap<PathBuf, String>,
) -> String {
    let mut out = String::from("// Generated by zfb-islands::IslandsBundler::produce_bundle_js\n");
    // `<ClientRouter />` auto-include (issue #289).
    //
    // `<ClientRouter />` only renders SSR `<head>` tags; the View
    // Transitions runtime is registered by `init()` as a side effect of
    // importing `@takazudo/zfb-runtime/client-router`. When the islands
    // scanner sees a page reach `<ClientRouter />` the build command sets
    // `BundleConfig::client_router`, and this side-effect import ships the
    // runtime in `assets/islands.js` with no `"use client"` boilerplate on
    // the consumer's side.
    //
    // When `client_router` is `false` this branch emits NOTHING, so a
    // project not using `<ClientRouter />` gets byte-identical output to a
    // pre-#289 build (acceptance criterion: zero new bytes for non-users).
    if client_router {
        out.push_str("import \"@takazudo/zfb-runtime/client-router\";\n");
    }
    if islands.is_empty() {
        // Header-only (plus the optional client-router import above) when
        // there are no islands. `build_production_islands_asset` only
        // reaches the bundler on an empty slice when `client_router` is
        // `true` (#289); a defensive zero-island call without it still
        // produces valid JS rather than a tail comma in a generated
        // array literal.
        return out;
    }
    {
        let build = build.expect("nonempty island entry source requires a build identity");
        out.push_str("import { mountIslands } from \"@takazudo/zfb/runtime\";\n");
        out.push_str("import { h } from \"@takazudo/zfb/zudo-react\";\n");
        out.push_str("import { hydrate, mount } from \"@takazudo/zfb/zudo-react/client\";\n");
        for (i, island) in islands.iter().enumerate() {
            out.push_str(&format!(
                "import * as __zfb_island_{i} from {};\n",
                json_string(&island.source_path.to_string_lossy())
            ));
        }
        out.push_str("const __zfb_manifest = Object.create(null);\n");
        // The scanner has selected a concrete export route. Falling back to
        // `default` can mount a different function under the selected marker.
        out.push_str("function __zfb_pick(ns, exportName) { if (!Object.prototype.hasOwnProperty.call(ns, exportName)) throw new Error(\"[zfb] missing owned island export \" + exportName); return ns[exportName]; }\n");
        out.push_str("function __zfb_register(ns, exportName, markerName, moduleLabel) {\n");
        out.push_str("  const C = __zfb_pick(ns, exportName);\n");
        out.push_str("  if (typeof C !== \"function\") throw new Error(\"[zfb] owned island export \" + exportName + \" from \" + moduleLabel + \" must be a function\");\n");
        out.push_str("  const name = C.name;\n");
        out.push_str("  if (!name || (C.displayName != null && C.displayName !== name) || name !== markerName) throw new Error(\"[zfb] owned island identity mismatch: \" + markerName + \" versus \" + name);\n");
        out.push_str("  if (Object.prototype.hasOwnProperty.call(__zfb_manifest, markerName)) throw new Error(\"[zfb] duplicate owned island marker: \" + markerName);\n");
        out.push_str(&format!("  __zfb_manifest[markerName] = {{ identity: {{ component: markerName, build: {} }}, mount: (props, element, mode) => {{\n", json_string(build)));
        out.push_str(&format!(
            "    const identity = {{ component: markerName, build: {} }};\n",
            json_string(build)
        ));
        out.push_str("    return mode === \"hydrate\" ? hydrate(h(C, props), element, { identity }) : mount(h(C, props), element, { identity });\n");
        out.push_str("  } };\n}\n");
        for (i, island) in islands.iter().enumerate() {
            let module_label = module_labels
                .get(&island.source_path)
                .filter(|label| !is_absolute_module_label(label))
                .cloned()
                .or_else(|| {
                    island
                        .source_path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .unwrap_or_default();
            out.push_str(&format!(
                "__zfb_register(__zfb_island_{i}, {}, {}, {});\n",
                json_string(&island.component_name),
                json_string(&island.marker_name),
                json_string(&module_label)
            ));
        }
        out.push_str("mountIslands(__zfb_manifest);\n");
    }
    out
}

fn is_absolute_module_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    Path::new(label).is_absolute()
        || label.starts_with('/')
        || label.starts_with('\\')
        || (bytes.get(1) == Some(&b':') && bytes.first().is_some_and(u8::is_ascii_alphabetic))
}

impl ClientBundler for IslandsBundler {
    fn bundle(&self, islands: &[Island], config: &BundleConfig) -> Result<BundleOutput> {
        validate_owned_build_identity(islands, config)?;
        let mut markers = std::collections::HashSet::new();
        for island in islands {
            if !markers.insert(&island.marker_name) {
                anyhow::bail!(
                    "duplicate owned island marker {:?} reached browser emission; validate and deduplicate defining bindings before bundling",
                    island.marker_name
                );
            }
        }
        self.sweep_stranded_entries();

        let OneEntryOutput {
            graph_digests: _,
            js,
            chunks,
            workers,
            resources,
        } = self.produce_bundle_js(islands, config)?;

        // Carry the entry JS **in memory** — do NOT write `islands.js` to
        // disk here. Per the Prod Asset Graph epic, the single source of
        // truth for all disk writes is the downstream consumer:
        //
        // - Production: `ProductionAssetPipeline` hashes these bytes,
        //   writes `assets/islands-<hash>.js`, rewrites HTML references in
        //   one pass. Writing a stable `islands.js` here too would produce
        //   two byte-identical files on every production deploy
        //   (zudolab/zzmod#497).
        // - Dev server: the dev path explicitly writes `islands.js` from
        //   `BundleOutput::bytes` (mirroring the CSS pipeline) so
        //   `ServeDir` can serve the stable URL.
        //
        // `asset_path` is the canonical path the caller SHOULD write to
        // (used for URL derivation and caller-side assertions); the bundler
        // itself never touches it.
        let asset_path = config
            .outdir
            .join(zfb_types::DIST_ASSETS_DIR)
            .join(zfb_types::STABLE_ISLANDS_FILENAME);

        // Chunks are returned in-memory only. The prod pipeline (#808)
        // writes them verbatim beside the (later content-hashed) entry
        // under `dist/assets/`, and the dev server (#809) serves them.
        // Chunks already carry esbuild's content hash in their filenames,
        // so the pipeline must NOT re-hash or rename them.

        let asset_url = bundle_link_href(&config.base_url, &asset_path);

        // Default `module_ids` mapping: identity per-island. Sub 1's
        // scanner is the source of truth for the canonical mapping (e.g.
        // when the exported name differs from the component name); until
        // it lands a richer Island shape, `component_name` is the right
        // contract for the hydration runtime to assume.
        let module_ids: Vec<ModuleId> = islands.iter().map(|i| i.component_name.clone()).collect();

        Ok(BundleOutput {
            bytes: js.into_bytes(),
            asset_path,
            asset_url,
            module_ids,
            chunks,
            workers,
            resources,
        })
    }
}

impl IslandsBundler {
    /// Bundle a single client-script file (`.client.{ts,tsx,js,jsx}`) directly.
    ///
    /// Unlike the shared-islands path (which synthesises a virtual entry that
    /// imports all islands), a client-script file IS the entry — no synthetic
    /// wrapper is generated. The file is bundled with `--splitting=false` so any
    /// dynamic `import()` calls are inlined into the single output file (identical
    /// rationale: a single emitted client-script file must contain its imports.
    ///
    /// `entry_name` is the logical name for this bundle — typically the file stem
    /// minus the `.client` suffix (e.g. `"search-widget"` for
    /// `search-widget.client.ts`). It is used as `--entry-names=<entry_name>` so
    /// the read-back function can find the output file by name.
    ///
    /// `entry_path` is the absolute path to the `.client.{ts,tsx,js,jsx}` source
    /// file. Because the file already exists on disk, esbuild's upward
    /// `node_modules` resolution walk starts from its directory — no temp entry
    /// file is needed. This means bare imports in the file resolve against
    /// `node_modules/` in the same directory or any ancestor, including the
    /// project root.
    ///
    /// Returns the bundled JS string.
    pub fn bundle_client_script_file(
        &self,
        entry_name: &str,
        entry_path: &Path,
        config: &BundleConfig,
    ) -> Result<String> {
        self.bundle_client_script_file_with_workers(entry_name, entry_path, &[], config)
            .map(|output| output.js)
    }

    /// Bundle one client-script entry and all of its discovered module-worker
    /// entries into an in-memory entry plus strict contract-named companions.
    ///
    /// Sweeps stranded in-project temp files first (issue #2109), matching
    /// [`ClientBundler::bundle`] — otherwise a
    /// client-script-only project (one that never runs an islands build)
    /// would accumulate its own worker-tsconfig/virtual-module strays
    /// indefinitely, only reaped whenever an unrelated islands build happens
    /// to run.
    pub fn bundle_client_script_file_with_workers(
        &self,
        entry_name: &str,
        entry_path: &Path,
        workers: &[ClientScriptWorkerEntry],
        config: &BundleConfig,
    ) -> Result<ClientScriptBundleOutput> {
        self.sweep_stranded_entries();

        let workers = validate_client_script_worker_entries(entry_name, workers)?;
        if self.config.mock_bundle {
            let js = if !self.config.mock_output.is_empty() {
                self.config.mock_output.clone()
            } else {
                // In mock mode with no canned output, read the file off disk
                // (same convention as `bundle_one_entry`'s mock path).
                std::fs::read_to_string(entry_path).with_context(|| {
                    format!("mock: failed to read entry file {}", entry_path.display())
                })?
            };
            let companions = workers
                .into_iter()
                .map(|(filename, source_path)| {
                    let bytes = std::fs::read(&source_path).with_context(|| {
                        format!("mock: failed to read worker file {}", source_path.display())
                    })?;
                    Ok(BundleChunk { filename, bytes })
                })
                .collect::<Result<Vec<_>>>()?;
            return Ok(ClientScriptBundleOutput { js, companions });
        }

        // Staging output directory — same pattern as `bundle_one_entry`.
        // A system tempdir is fine for the output because the `node_modules`
        // upward-walk starts from the actual entry file's directory (not the
        // outdir), so bare imports resolve against the project's
        // `node_modules/` naturally.
        let out_dir = tempfile::Builder::new()
            .prefix("zfb-esbuild-client-out-")
            .tempdir()
            .context("failed to allocate out temp dir for client script bundling")?;

        let mut jobs = Vec::with_capacity(workers.len() + 1);
        jobs.push((
            entry_name.to_string(),
            entry_path.to_path_buf(),
            "client script",
        ));
        jobs.extend(workers.iter().map(|(filename, path)| {
            (
                filename.trim_end_matches(".js").to_string(),
                path.clone(),
                "client-script module worker",
            )
        }));

        for (output_stem, source_path, label) in jobs {
            let job_resolver = build_job_resolver_inputs(
                &self.config.working_dir,
                &source_path,
                self.config.tsconfig_search_boundary.as_deref(),
                &self.config.alias_entries,
                &self.config.virtual_modules,
            )?;
            let job_stage_audit_metafile = stage_audit_metafile(self.config.stage_audit.as_ref())?;
            // Every entry is self-contained (no splitting). Worker URL
            // edges have already been rewritten to sibling contract names.
            let request = build_bundle_request(
                &self.config,
                config,
                Some(&job_resolver.resolver_inputs),
                job_resolver.tsconfig.as_ref().map(|file| file.path()),
                JobShape::named(
                    out_dir.path(),
                    &source_path,
                    &output_stem,
                    job_stage_audit_metafile.as_ref().map(|f| f.path()),
                ),
            )?;
            run_native_entry(
                request,
                &source_path,
                self.config.tsconfig_search_boundary.as_deref(),
            )?;
            if std::env::var_os("ZFB_DEV_TIMING").is_some() {
                let kind = if label == "client script" {
                    "entry"
                } else {
                    "worker"
                };
                let relative_source = source_path
                    .strip_prefix(&self.config.working_dir)
                    .unwrap_or(&source_path);
                // The line shape is parsed by prototypes/rolldown/client-scripts.mjs
                // and crates/zfb/tests/client_bundling_cross_pipeline.rs.
                eprintln!(
                    "[zfb-timing] rolldown-prototype client-script job={output_stem} kind={kind} source={}",
                    relative_source.display()
                );
            }

            if let Some(policy) = &self.config.stage_audit {
                let metafile_path = job_stage_audit_metafile
                    .as_ref()
                    .expect("job_stage_audit_metafile is Some whenever config.stage_audit is Some");
                run_stage_escape_audit(
                    policy,
                    metafile_path.path(),
                    &self.config.working_dir,
                    &format!("{label} {source_path:?}"),
                )?;
            }
        }

        let expected_workers = workers.into_keys().collect();
        read_back_client_script_outdir(out_dir.path(), entry_name, &expected_workers)
    }
}

/// Compute the 8-char hex hash for the given bundle bytes.
///
/// The hash is the first 8 characters of `sha256(js)` in lowercase hex.
/// Mirrors `zfb-css::pipeline::hash_8` (which folds two halves via a `\n`
/// separator) — for islands there is only one byte stream so no separator
/// is needed.
pub fn hash_8(js: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(js.as_bytes());
    let digest = hasher.finalize();
    let full = hex::encode(digest);
    full[..8].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_BUILD_TOKEN: &str = "0123456789abcdef";

    fn config_request(cwd: &Path) -> BundleRequest {
        BundleRequest::new(cwd, "entry.ts", OutputLayout::dir("out"), Platform::Browser)
    }

    #[test]
    fn native_config_discovers_jsconfig_and_reloads_package_selected_parent() {
        let owner = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(owner.path()).unwrap();
        let package = root.as_path().join("node_modules/@scope/config");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::create_dir_all(root.as_path().join("src")).unwrap();
        std::fs::write(
            root.as_path().join("jsconfig.json"),
            r#"{"extends":"@scope/config"}"#,
        )
        .unwrap();
        std::fs::write(
            package.join("assignment.json"),
            r#"{"compilerOptions":{"useDefineForClassFields":false}}"#,
        )
        .unwrap();
        std::fs::write(
            package.join("define.json"),
            r#"{"compilerOptions":{"useDefineForClassFields":true}}"#,
        )
        .unwrap();
        let request = config_request(root.as_path());
        for (selected, expected) in [("assignment.json", false), ("define.json", true)] {
            std::fs::write(
                package.join("package.json"),
                format!(r#"{{"tsconfig":"{selected}"}}"#),
            )
            .unwrap();
            let file = native_entry_config(&request, &root.as_path().join("src/worker.ts"), None)
                .unwrap()
                .unwrap();
            let value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(file.path()).unwrap()).unwrap();
            assert_eq!(
                value["compilerOptions"]["useDefineForClassFields"],
                expected
            );
            assert!(
                !file.path().starts_with(root.as_path()),
                "dependency directories need not be writable"
            );
            assert_eq!(std::fs::read_dir(&package).unwrap().count(), 3);
            let path = file.path().to_path_buf();
            drop(file);
            assert!(!path.exists(), "materialized config must retain an owner");
        }
    }

    #[test]
    fn native_config_keeps_diamond_precedence_and_original_path_bases() {
        let owner = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(owner.path()).unwrap();
        let base = root.as_path().join("base");
        let leaf = root.as_path().join("leaf");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&leaf).unwrap();
        std::fs::write(base.join("common.json"), r#"{"compilerOptions":{"useDefineForClassFields":false,"baseUrl":"assets","paths":{"shared/*":["shared/*"]},"rootDirs":["src"]},"include":["src/**/*.ts"]}"#).unwrap();
        std::fs::write(
            base.join("a.json"),
            r#"{"extends":"./common.json","compilerOptions":{"useDefineForClassFields":true}}"#,
        )
        .unwrap();
        std::fs::write(
            base.join("b.json"),
            r#"{"extends":"./common.json","compilerOptions":{"paths":{"worker/*":["workers/*"]}}}"#,
        )
        .unwrap();
        std::fs::write(
            leaf.join("jsconfig.json"),
            r#"{"extends":["../base/a.json","../base/b.json"],"compilerOptions":{"strict":true}}"#,
        )
        .unwrap();
        let value = native_effective_config(&leaf.join("jsconfig.json"), false).unwrap();
        let options = &value["compilerOptions"];
        assert_eq!(
            options["useDefineForClassFields"], false,
            "later parent's inherited value overrides earlier parent's own value"
        );
        assert_eq!(options["strict"], true);
        assert_eq!(options["baseUrl"], base.join("assets").to_str().unwrap());
        assert_eq!(
            options["paths"]["worker/*"][0],
            base.join("assets/workers/*").to_str().unwrap()
        );
        assert!(
            options["paths"].get("shared/*").is_none(),
            "paths maps replace rather than deep-merge"
        );
        assert_eq!(options["rootDirs"][0], base.join("src").to_str().unwrap());
        assert_eq!(
            value["include"][0],
            base.join("src/**/*.ts").to_str().unwrap()
        );
        assert!(value.get("extends").is_none());
    }

    #[test]
    fn native_config_explicit_selection_wins_and_boundary_stops_discovery() {
        let owner = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(owner.path()).unwrap();
        let project = root.as_path().join("project");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(
            root.as_path().join("jsconfig.json"),
            r#"{"compilerOptions":{"strict":false}}"#,
        )
        .unwrap();
        std::fs::write(
            project.join("selected.json"),
            r#"{"compilerOptions":{"strict":true}}"#,
        )
        .unwrap();
        let source = project.join("worker.ts");
        let mut request = config_request(&project);
        assert!(native_entry_config(&request, &source, Some(&project))
            .unwrap()
            .is_none());
        request.tsconfig = Some(PathBuf::from("selected.json"));
        let selected = native_entry_config(&request, &source, Some(&project))
            .unwrap()
            .unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(selected.path()).unwrap()).unwrap();
        assert_eq!(value["compilerOptions"]["strict"], true);
        request.tsconfig = Some(PathBuf::from("missing.json"));
        assert!(
            native_entry_config(&request, &source, None).is_err(),
            "never fall back from an invalid explicit selection"
        );
    }

    #[test]
    fn native_config_effective_base_url_anchors_inherited_paths() {
        let owner = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(owner.path()).unwrap();
        let parent = root.as_path().join("parent");
        let leaf = root.as_path().join("leaf");
        std::fs::create_dir(&parent).unwrap();
        std::fs::create_dir(&leaf).unwrap();
        std::fs::write(
            parent.join("paths.json"),
            r#"{"compilerOptions":{"paths":{"chosen":["./chosen.ts"]}}}"#,
        )
        .unwrap();
        std::fs::write(
            parent.join("base.json"),
            r#"{"compilerOptions":{"baseUrl":"sibling"}}"#,
        )
        .unwrap();
        for (config, expected) in [
            (
                r#"{"extends":"../parent/paths.json","compilerOptions":{"baseUrl":"local"}}"#,
                leaf.join("local/./chosen.ts"),
            ),
            (
                r#"{"extends":["../parent/paths.json","../parent/base.json"]}"#,
                parent.join("sibling/./chosen.ts"),
            ),
            (
                r#"{"extends":"../parent/paths.json"}"#,
                parent.join("./chosen.ts"),
            ),
        ] {
            let selected = leaf.join("tsconfig.json");
            std::fs::write(&selected, config).unwrap();
            let value = native_effective_config(&selected, false).unwrap();
            assert_eq!(
                value["compilerOptions"]["paths"]["chosen"][0],
                expected.to_str().unwrap()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn native_config_preserves_requested_symlink_path_semantics() {
        let owner = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(owner.path()).unwrap();
        let real = root.as_path().join("real");
        let lexical = root.as_path().join("project");
        std::fs::create_dir(&real).unwrap();
        std::fs::create_dir(&lexical).unwrap();
        std::fs::write(
            real.join("base.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"chosen":["./chosen.ts"]}}}"#,
        )
        .unwrap();
        std::os::unix::fs::symlink(real.join("base.json"), lexical.join("tsconfig.json")).unwrap();
        for (preserve, base) in [(false, &real), (true, &lexical)] {
            let mut request = config_request(&lexical);
            request.resolve.preserve_symlinks = preserve;
            let file = native_entry_config(&request, &lexical.join("worker.ts"), None)
                .unwrap()
                .unwrap();
            let value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(file.path()).unwrap()).unwrap();
            assert_eq!(
                value["compilerOptions"]["paths"]["chosen"][0],
                base.join("././chosen.ts").to_str().unwrap()
            );
        }
    }

    #[test]
    fn native_config_rejects_invalid_cyclic_and_excessive_extends() {
        let owner = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(owner.path()).unwrap();
        let path = root.as_path().join("tsconfig.json");
        for invalid in [
            r#"{"extends":[7]}"#,
            r#"{"extends":"absent-package"}"#,
            r#"{"extends":"./tsconfig.json"}"#,
            r#"{"compilerOptions":{"paths":{"@/*":"wrong-type"}}}"#,
            r#"{"references":[{"path":"./other"}]}"#,
            r#"{"compilerOptions":{"baseUrl":"${configDir}"}}"#,
        ] {
            std::fs::write(&path, invalid).unwrap();
            assert!(native_effective_config(&path, false).is_err(), "{invalid}");
        }
        for index in 0..10 {
            let value = if index == 9 {
                "{}".to_owned()
            } else {
                format!(r#"{{"extends":"./{}.json"}}"#, index + 1)
            };
            std::fs::write(root.as_path().join(format!("{index}.json")), value).unwrap();
        }
        let error = native_effective_config(&root.as_path().join("0.json"), false).unwrap_err();
        assert!(error.to_string().contains("exceeded 8 levels"));
    }

    /// Write an in-project entry-shaped file and backdate its mtime by `age`.
    fn write_entry_aged(dir: &Path, name: &str, age: std::time::Duration) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, "// stranded entry\n").unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_modified(std::time::SystemTime::now() - age)
            .unwrap();
        path
    }

    #[test]
    fn is_zfb_islands_temp_file_drops_every_in_project_temp_class_but_keeps_real_sources() {
        // Issue #3215: every islands temp class must be dropped by the
        // dev watcher's intake filter so its `Created`/`Removed` events do
        // not trigger a second full tick after a rebuild. Pin one filename
        // per recognized class, plus a normal source file that must NOT match.
        for name in [
            ".zfb-esbuild-entry-APmUkl.tsx",
            ".zfb-islands-tsconfig-abc123.json",
            ".zfb-worker-tsconfig-abc123.json",
        ] {
            assert!(
                is_zfb_islands_temp_file(Path::new(name)),
                "{name} should be recognized as an in-project islands temp file"
            );
        }
        assert!(
            is_zfb_islands_temp_file(Path::new(&format!(
                "{}xyz{}",
                zfb_plugin_resolver::VIRTUAL_MODULE_TEMP_PREFIX,
                zfb_plugin_resolver::VIRTUAL_MODULE_TEMP_SUFFIX
            ))),
            "the virtual-module temp class should be recognized too"
        );
        assert!(
            !is_zfb_islands_temp_file(Path::new("Header.tsx")),
            "an ordinary component source file must never be dropped"
        );
        assert!(
            !is_zfb_islands_temp_file(Path::new("islands-chunk-abc123.js")),
            "an unrelated zfb-generated filename must not accidentally match"
        );
    }

    #[test]
    fn sweep_removes_entry_stranded_by_a_dead_process() {
        // Issue #1970: a `zfb dev`/`zfb build` killed mid-bundle never runs
        // `NamedTempFile`'s Drop, so its entry stays in the project root
        // forever. The next bundle reaps it.
        let dir = tempfile::tempdir().unwrap();
        let orphan = write_entry_aged(
            dir.path(),
            ".zfb-esbuild-entry-plnMVZ.tsx",
            ORPHANED_ENTRY_GRACE + std::time::Duration::from_secs(60),
        );
        sweep_orphaned_in_project_temp_files(dir.path());
        assert!(
            !orphan.exists(),
            "an entry older than the grace period must be swept"
        );
    }

    #[test]
    fn sweep_keeps_a_recent_entry_a_concurrent_process_may_still_own() {
        // Safety property: `zfb dev` and `zfb build` legitimately run against
        // the same project root at once. Deleting the other process's live
        // entry would break its esbuild invocation.
        let dir = tempfile::tempdir().unwrap();
        let live = write_entry_aged(
            dir.path(),
            ".zfb-esbuild-entry-trVVbc.tsx",
            std::time::Duration::from_secs(5),
        );
        sweep_orphaned_in_project_temp_files(dir.path());
        assert!(
            live.exists(),
            "an entry inside the grace period must be left alone"
        );
    }

    #[test]
    fn sweep_keeps_a_stale_entry_whose_owner_still_holds_it() {
        // The age gate alone cannot tell "abandoned" from "owned by a process
        // suspended (or stuck in one esbuild invocation) past the grace
        // period". The shared lock `bundle_one_entry` holds on its own entry
        // does, and it must win over the age gate.
        let dir = tempfile::tempdir().unwrap();
        let owned = write_entry_aged(
            dir.path(),
            ".zfb-esbuild-entry-owned.tsx",
            ORPHANED_ENTRY_GRACE + std::time::Duration::from_secs(60 * 60),
        );
        let owner = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&owned)
            .unwrap();
        owner.lock_shared().expect("take the owner's shared lock");

        sweep_orphaned_in_project_temp_files(dir.path());
        assert!(
            owned.exists(),
            "an entry still locked by its owner must be spared however old it is"
        );

        // Once the owner releases it — as the OS does for a process that dies
        // without unwinding — the next sweep reaps it.
        drop(owner);
        sweep_orphaned_in_project_temp_files(dir.path());
        assert!(
            !owned.exists(),
            "an unlocked, past-grace entry must be swept"
        );
    }

    // --- Per-class tests for the three classes joined by issue #2109 -------
    //
    // Each class below mirrors the three entry-class tests just above, ONE
    // exception: only the stale+unlocked "reaped" case is genuinely
    // RED-before/GREEN-after this sub's generalization (verified by running
    // each on the pre-generalization `sweep_orphaned_in_project_entries`,
    // which only recognized the entry class, and observing the expected
    // failure, then again after widening `IN_PROJECT_TEMP_CLASSES` and
    // observing the pass). The recent-spared and locked-spared cases are NOT
    // naturally red the same way: before generalization an unrecognized class
    // is skipped unconditionally, so those two assertions already "pass" —
    // for the wrong reason (never matched at all, not "matched but
    // protected"). Each is instead verified REVERT-PROOF by hand: temporarily
    // stubbing out the specific safety gate it exercises (the
    // `age >= ORPHANED_ENTRY_GRACE` check for the recent case, the
    // `try_lock()` check for the locked case) — while leaving the class
    // recognized — reproduces an INCORRECT sweep of that file, proving the
    // real gate is what the passing assertion depends on rather than the
    // class simply never matching.

    #[test]
    fn islands_tsconfig_sweep_removes_orphan_stranded_by_a_dead_process() {
        // Genuinely red before #2109's generalization: `.zfb-islands-tsconfig-*.json`
        // was not in `IN_PROJECT_TEMP_CLASSES`, so this file was never matched and
        // survived regardless of age.
        let dir = tempfile::tempdir().unwrap();
        let orphan = write_entry_aged(
            dir.path(),
            ".zfb-islands-tsconfig-plnMVZ.json",
            ORPHANED_ENTRY_GRACE + std::time::Duration::from_secs(60),
        );
        sweep_orphaned_in_project_temp_files(dir.path());
        assert!(
            !orphan.exists(),
            "an islands-tsconfig file older than the grace period must be swept"
        );
    }

    #[test]
    fn islands_tsconfig_sweep_keeps_a_recent_file_a_concurrent_process_may_still_own() {
        // Revert-proof: temporarily short-circuiting the sweep's `age >= ORPHANED_ENTRY_GRACE`
        // check to always report "stale" (while leaving the class recognized) reaps this
        // recent file — proving the age gate, not "class unmatched", is what spares it.
        let dir = tempfile::tempdir().unwrap();
        let live = write_entry_aged(
            dir.path(),
            ".zfb-islands-tsconfig-trVVbc.json",
            std::time::Duration::from_secs(5),
        );
        sweep_orphaned_in_project_temp_files(dir.path());
        assert!(
            live.exists(),
            "an islands-tsconfig file inside the grace period must be left alone"
        );
    }

    #[test]
    fn islands_tsconfig_sweep_keeps_a_stale_file_whose_owner_still_holds_it() {
        // Revert-proof: temporarily short-circuiting the sweep's `try_lock()` check to
        // always report "lockable" (while leaving the class recognized) reaps this
        // still-locked file — proving the lock gate, not "class unmatched", is what
        // spares it.
        let dir = tempfile::tempdir().unwrap();
        let owned = write_entry_aged(
            dir.path(),
            ".zfb-islands-tsconfig-owned.json",
            ORPHANED_ENTRY_GRACE + std::time::Duration::from_secs(60 * 60),
        );
        let owner = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&owned)
            .unwrap();
        owner.lock_shared().expect("take the owner's shared lock");

        sweep_orphaned_in_project_temp_files(dir.path());
        assert!(
            owned.exists(),
            "an islands-tsconfig file still locked by its owner must be spared however old it is"
        );

        drop(owner);
        sweep_orphaned_in_project_temp_files(dir.path());
        assert!(
            !owned.exists(),
            "an unlocked, past-grace islands-tsconfig file must be swept"
        );
    }

    #[test]
    fn worker_tsconfig_sweep_removes_orphan_stranded_by_a_dead_process() {
        // Genuinely red before #2109's generalization: `.zfb-worker-tsconfig-*.json`
        // was not in `IN_PROJECT_TEMP_CLASSES`, so this file was never matched and
        // survived regardless of age.
        let dir = tempfile::tempdir().unwrap();
        let orphan = write_entry_aged(
            dir.path(),
            ".zfb-worker-tsconfig-plnMVZ.json",
            ORPHANED_ENTRY_GRACE + std::time::Duration::from_secs(60),
        );
        sweep_orphaned_in_project_temp_files(dir.path());
        assert!(
            !orphan.exists(),
            "a worker-tsconfig file older than the grace period must be swept"
        );
    }

    #[test]
    fn worker_tsconfig_sweep_keeps_a_recent_file_a_concurrent_process_may_still_own() {
        // Revert-proof: see the islands-tsconfig sibling above — the same age-gate
        // stub reaps this recent file, proving the age gate is load-bearing here too.
        let dir = tempfile::tempdir().unwrap();
        let live = write_entry_aged(
            dir.path(),
            ".zfb-worker-tsconfig-trVVbc.json",
            std::time::Duration::from_secs(5),
        );
        sweep_orphaned_in_project_temp_files(dir.path());
        assert!(
            live.exists(),
            "a worker-tsconfig file inside the grace period must be left alone"
        );
    }

    #[test]
    fn worker_tsconfig_sweep_keeps_a_stale_file_whose_owner_still_holds_it() {
        // Revert-proof: see the islands-tsconfig sibling above — the same lock-gate
        // stub reaps this still-locked file, proving the lock gate is load-bearing here too.
        let dir = tempfile::tempdir().unwrap();
        let owned = write_entry_aged(
            dir.path(),
            ".zfb-worker-tsconfig-owned.json",
            ORPHANED_ENTRY_GRACE + std::time::Duration::from_secs(60 * 60),
        );
        let owner = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&owned)
            .unwrap();
        owner.lock_shared().expect("take the owner's shared lock");

        sweep_orphaned_in_project_temp_files(dir.path());
        assert!(
            owned.exists(),
            "a worker-tsconfig file still locked by its owner must be spared however old it is"
        );

        drop(owner);
        sweep_orphaned_in_project_temp_files(dir.path());
        assert!(
            !owned.exists(),
            "an unlocked, past-grace worker-tsconfig file must be swept"
        );
    }

    #[test]
    fn virtual_module_sweep_removes_orphan_stranded_by_a_dead_process() {
        // Genuinely red before #2109's generalization: `.zfb-virtual-*.mjs`
        // was not in `IN_PROJECT_TEMP_CLASSES`, so this file was never matched and
        // survived regardless of age.
        let dir = tempfile::tempdir().unwrap();
        let orphan = write_entry_aged(
            dir.path(),
            ".zfb-virtual-plnMVZ.mjs",
            ORPHANED_ENTRY_GRACE + std::time::Duration::from_secs(60),
        );
        sweep_orphaned_in_project_temp_files(dir.path());
        assert!(
            !orphan.exists(),
            "a virtual-module file older than the grace period must be swept"
        );
    }

    #[test]
    fn virtual_module_sweep_keeps_a_recent_file_a_concurrent_process_may_still_own() {
        // Revert-proof: see the islands-tsconfig sibling above — the same age-gate
        // stub reaps this recent file, proving the age gate is load-bearing here too.
        let dir = tempfile::tempdir().unwrap();
        let live = write_entry_aged(
            dir.path(),
            ".zfb-virtual-trVVbc.mjs",
            std::time::Duration::from_secs(5),
        );
        sweep_orphaned_in_project_temp_files(dir.path());
        assert!(
            live.exists(),
            "a virtual-module file inside the grace period must be left alone"
        );
    }

    #[test]
    fn virtual_module_sweep_keeps_a_stale_file_whose_owner_still_holds_it() {
        // Revert-proof: see the islands-tsconfig sibling above — the same lock-gate
        // stub reaps this still-locked file, proving the lock gate is load-bearing here too.
        let dir = tempfile::tempdir().unwrap();
        let owned = write_entry_aged(
            dir.path(),
            ".zfb-virtual-owned.mjs",
            ORPHANED_ENTRY_GRACE + std::time::Duration::from_secs(60 * 60),
        );
        let owner = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&owned)
            .unwrap();
        owner.lock_shared().expect("take the owner's shared lock");

        sweep_orphaned_in_project_temp_files(dir.path());
        assert!(
            owned.exists(),
            "a virtual-module file still locked by its owner must be spared however old it is"
        );

        drop(owner);
        sweep_orphaned_in_project_temp_files(dir.path());
        assert!(
            !owned.exists(),
            "an unlocked, past-grace virtual-module file must be swept"
        );
    }

    #[test]
    fn allocated_entry_is_locked_so_a_concurrent_sweep_spares_it() {
        // The test above takes the owner's lock by hand, so it pins the
        // sweep's respect-a-lock behaviour but not the production path that
        // acquires the lock in the first place. This one drives the real
        // allocation (`allocate_locked_entry_tmp`, the step
        // `bundle_one_entry` runs before every esbuild invocation) and then
        // backdates the entry past the grace period, leaving the ownership
        // lock as the ONLY thing that can spare it. Drop
        // `allocate_locked_entry_tmp`'s `lock_shared` call and this fails.
        let dir = tempfile::tempdir().unwrap();
        let entry_tmp =
            allocate_locked_entry_tmp(dir.path(), "// entry\n").expect("allocate the entry");
        let path = entry_tmp.path().to_path_buf();
        assert_eq!(
            path.parent(),
            Some(dir.path()),
            "an existing working_dir must host the entry"
        );
        entry_tmp
            .as_file()
            .set_modified(
                std::time::SystemTime::now()
                    - (ORPHANED_ENTRY_GRACE + std::time::Duration::from_secs(60)),
            )
            .unwrap();

        sweep_orphaned_in_project_temp_files(dir.path());

        assert!(
            path.exists(),
            "an entry still owned by an in-flight bundle must be spared however old it is"
        );
    }

    #[test]
    fn islands_tsconfig_is_locked_so_a_concurrent_exclusive_lock_attempt_fails_while_owned() {
        // Same contract as `allocated_entry_is_locked_so_a_concurrent_sweep_spares_it` above,
        // applied to `build_plugin_tsconfig`'s `.zfb-islands-tsconfig-*.json` allocation: drive
        // the REAL production helper (not a hand-written file), then prove a second handle's
        // exclusive `try_lock()` fails while the production handle is alive and succeeds once
        // it drops. Drop `build_plugin_tsconfig`'s `lock_shared` call and this fails.
        let dir = tempfile::tempdir().unwrap();
        let plugin_target = dir.path().join("plugin-entry.ts");
        std::fs::write(&plugin_target, "export const plugin = true;\n").unwrap();
        let aliases = vec![(
            "@plugin/entry".to_string(),
            plugin_target.to_string_lossy().into_owned(),
        )];
        let resolver_inputs = zfb_plugin_resolver::build_resolver_inputs(
            &aliases,
            &[],
            dir.path(),
            &BTreeMap::new(),
            &[],
        )
        .expect("build resolver inputs with at least one plugin entry");
        assert!(
            !resolver_inputs.paths_entries.is_empty(),
            "the fixture must actually register a plugin path entry, or build_plugin_tsconfig \
             takes its no-plugin early return and allocates nothing"
        );

        let tmp = build_plugin_tsconfig(
            dir.path(),
            &resolver_inputs,
            &BTreeMap::new(),
            "islands",
            "test tsconfig",
        )
        .expect("build the islands tsconfig")
        .expect("a plugin entry must produce a synthetic tsconfig");
        let path = tmp.path().to_path_buf();

        let second_handle = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("open a second handle to the same tsconfig");
        assert!(
            second_handle.try_lock().is_err(),
            "a concurrent exclusive try_lock must fail while build_plugin_tsconfig's own \
             shared-lock-holding handle is still alive"
        );

        drop(tmp);

        assert!(
            second_handle.try_lock().is_ok(),
            "once the production handle drops, the exclusive try_lock must succeed"
        );
    }

    #[test]
    fn sweep_reaps_all_four_recognized_classes_but_spares_the_shadow_and_unrelated_files() {
        // Issue #2109 (Temp Sweeper epic #2106): generalizes what used to be
        // `sweep_touches_nothing_but_stale_entry_files`, which asserted the
        // three sibling `.zfb-*` classes were "deliberately out of scope" —
        // that was true only because the sweeper didn't recognize them yet.
        // Now that the class table covers all four, a stale+unlocked file of
        // ANY recognized class must be reaped.
        let dir = tempfile::tempdir().unwrap();
        let old = ORPHANED_ENTRY_GRACE + std::time::Duration::from_secs(60);

        let reaped = [
            write_entry_aged(dir.path(), ".zfb-esbuild-entry-abc.tsx", old),
            write_entry_aged(dir.path(), ".zfb-islands-tsconfig-abc.json", old),
            write_entry_aged(dir.path(), ".zfb-worker-tsconfig-abc.json", old),
            write_entry_aged(dir.path(), ".zfb-virtual-abc.mjs", old),
        ];

        // Genuinely out of scope, unchanged by the generalization: a name
        // whose suffix doesn't match any recognized class, an unrelated
        // project file, and a directory that merely happens to share the
        // entry class's name shape.
        let kept = [
            write_entry_aged(dir.path(), ".zfb-esbuild-entry-abc.json", old), // wrong suffix
            write_entry_aged(dir.path(), "page.tsx", old),
        ];
        let dir_shaped_like_an_entry = dir.path().join(".zfb-esbuild-entry-dir.tsx");
        std::fs::create_dir(&dir_shaped_like_an_entry).unwrap();

        // The shadow-side virtual-module leak (zfb-build's SSR bundler,
        // `bundler.rs`, passes a `shadow` dir inside its own
        // `zfb-shadow-session-*` `TempDir`) is a DIFFERENT producer and stays
        // out of scope per #2108's Part A — it is already covered by a
        // separate stale-shadow sweep. This sweep is a non-recursive
        // `read_dir` of `working_dir` alone, so a virtual-module-shaped file
        // one level down (standing in for that nested shadow root) can never
        // be reached regardless of naming.
        let shadow_root = dir.path().join("zfb-shadow-session-stand-in");
        std::fs::create_dir(&shadow_root).unwrap();
        let shadow_virtual = write_entry_aged(&shadow_root, ".zfb-virtual-nested.mjs", old);

        sweep_orphaned_in_project_temp_files(dir.path());

        for path in &reaped {
            assert!(
                !path.exists(),
                "a stale, unlocked file of a recognized class must be swept: {}",
                path.display()
            );
        }
        for path in &kept {
            assert!(path.exists(), "must not sweep {}", path.display());
        }
        assert!(
            dir_shaped_like_an_entry.is_dir(),
            "a directory matching the entry name shape must not be touched"
        );
        assert!(
            shadow_virtual.exists(),
            "the shadow-side virtual-module class (a separate producer/TempDir, #2108 Part A) \
             must stay out of this project-root-only sweep's scope"
        );
    }

    #[test]
    fn client_scripts_read_back_collects_only_contract_workers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("widget.js"), "entry\n").unwrap();
        std::fs::write(dir.path().join("worker-src-s-search-d-ts.js"), b"worker\n").unwrap();
        std::fs::write(dir.path().join("widget.js.map"), b"{}").unwrap();
        std::fs::write(dir.path().join("worker-src-s-search-d-ts.js.map"), b"{}").unwrap();
        let expected = BTreeSet::from(["worker-src-s-search-d-ts.js".to_string()]);

        let output = read_back_client_script_outdir(dir.path(), "widget", &expected).unwrap();
        assert_eq!(output.js, "entry\n");
        assert_eq!(output.companions.len(), 1);
        assert_eq!(output.companions[0].filename, "worker-src-s-search-d-ts.js");
        assert_eq!(output.companions[0].bytes, b"worker\n");
    }

    #[test]
    fn client_scripts_read_back_rejects_unknown_outputs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("widget.js"), "entry\n").unwrap();
        std::fs::write(dir.path().join("surprise.js"), "unknown\n").unwrap();

        let error = read_back_client_script_outdir(dir.path(), "widget", &BTreeSet::new())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("unexpected client-script output file"),
            "{error}"
        );
    }

    #[test]
    fn client_scripts_read_back_requires_every_worker() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("widget.js"), "entry\n").unwrap();
        let expected = BTreeSet::from(["worker-src-s-search-d-ts.js".to_string()]);

        let error = read_back_client_script_outdir(dir.path(), "widget", &expected)
            .unwrap_err()
            .to_string();
        assert!(error.contains("produced no output"), "{error}");
        assert!(error.contains("worker-src-s-search-d-ts.js"), "{error}");
    }

    #[test]
    fn client_scripts_worker_filename_collisions_are_hard_errors() {
        let workers = vec![
            ClientScriptWorkerEntry {
                filename: "worker-same.js".into(),
                source_path: PathBuf::from("/project/a.ts"),
            },
            ClientScriptWorkerEntry {
                filename: "worker-same.js".into(),
                source_path: PathBuf::from("/project/b.ts"),
            },
        ];
        let error = validate_client_script_worker_entries("widget", &workers)
            .unwrap_err()
            .to_string();
        assert!(error.contains("filename collision"), "{error}");

        let entry_collision = vec![ClientScriptWorkerEntry {
            filename: "worker-same.js".into(),
            source_path: PathBuf::from("/project/worker.ts"),
        }];
        let error = validate_client_script_worker_entries("worker-same", &entry_collision)
            .unwrap_err()
            .to_string();
        assert!(error.contains("collide on output filename"), "{error}");
    }

    #[test]
    fn hash_is_stable_for_identical_inputs() {
        let a = hash_8("export const x = 1;");
        let b = hash_8("export const x = 1;");
        assert_eq!(a, b);
        assert_eq!(a.len(), 8);
    }

    #[test]
    fn hash_changes_when_payload_changes() {
        let before = hash_8("export const x = 1;");
        let after = hash_8("export const x = 2;");
        assert_ne!(before, after);
    }

    #[test]
    fn job_resolver_leaves_native_nearest_config_discovery_without_plugins() {
        let project = tempfile::tempdir().unwrap();
        let project_tsconfig = project.path().join("tsconfig.json");
        std::fs::write(
            &project_tsconfig,
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@worker/*":["lib/*"]}}}"#,
        )
        .unwrap();
        let source = project.path().join("src/worker.ts");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "export {};\n").unwrap();
        let effective = build_job_resolver_inputs(project.path(), &source, None, &[], &[]).unwrap();
        assert!(
            effective.tsconfig.is_none(),
            "no-plugin jobs must retain esbuild's native closest-parent discovery"
        );
        assert_eq!(
            nearest_typescript_config(&source, None),
            Some(project_tsconfig)
        );
    }

    #[test]
    fn job_resolver_extends_nearest_config_and_merges_shadow_user_and_plugin_paths() {
        let project = tempfile::tempdir().unwrap();
        let nested = project.path().join("packages/widget");
        std::fs::create_dir_all(nested.join("src")).unwrap();
        let project_tsconfig = nested.join("jsconfig.json");
        std::fs::write(
            &project_tsconfig,
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@user/*":["src/*"]}}}"#,
        )
        .unwrap();
        let source = nested.join("src/worker.ts");
        std::fs::write(&source, "export {};\n").unwrap();
        let plugin_target = project.path().join("plugin-entry.ts");
        std::fs::write(&plugin_target, "export const plugin = true;\n").unwrap();
        let aliases = vec![(
            "@plugin/entry".to_string(),
            plugin_target.to_string_lossy().into_owned(),
        )];
        let effective =
            build_job_resolver_inputs(project.path(), &source, None, &aliases, &[]).unwrap();
        let file = effective
            .tsconfig
            .expect("plugin paths require a merged synthetic worker config");
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(file.path()).unwrap()).unwrap();
        assert_eq!(
            json["extends"].as_str(),
            Some(project_tsconfig.to_string_lossy().as_ref())
        );
        assert_eq!(
            json["compilerOptions"]["paths"]["@user/*"][0].as_str(),
            Some(nested.join("src/*").to_string_lossy().as_ref())
        );
        assert_eq!(
            json["compilerOptions"]["paths"]["@plugin/entry"][0].as_str(),
            Some(plugin_target.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn worker_tsconfig_is_locked_so_a_concurrent_exclusive_lock_attempt_fails_while_owned() {
        // Same contract as `allocated_entry_is_locked_so_a_concurrent_sweep_spares_it` and
        // `islands_tsconfig_is_locked_so_a_concurrent_exclusive_lock_attempt_fails_while_owned`
        // above, applied to `build_job_resolver_inputs`'s `.zfb-worker-tsconfig-*.json`
        // allocation: drive the REAL production helper, then prove a second handle's exclusive
        // `try_lock()` fails while the production handle is alive and succeeds once it drops.
        // Drop `build_job_resolver_inputs`'s `lock_shared` call and this fails.
        let project = tempfile::tempdir().unwrap();
        let source = project.path().join("src/worker.ts");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "export {};\n").unwrap();
        let plugin_target = project.path().join("plugin-entry.ts");
        std::fs::write(&plugin_target, "export const plugin = true;\n").unwrap();
        let aliases = vec![(
            "@plugin/entry".to_string(),
            plugin_target.to_string_lossy().into_owned(),
        )];

        let effective =
            build_job_resolver_inputs(project.path(), &source, None, &aliases, &[]).unwrap();
        let tmp = effective
            .tsconfig
            .expect("a plugin entry must produce a synthetic worker tsconfig");
        let path = tmp.path().to_path_buf();

        let second_handle = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("open a second handle to the same tsconfig");
        assert!(
            second_handle.try_lock().is_err(),
            "a concurrent exclusive try_lock must fail while build_job_resolver_inputs's own \
             shared-lock-holding handle is still alive"
        );

        drop(tmp);

        assert!(
            second_handle.try_lock().is_ok(),
            "once the production handle drops, the exclusive try_lock must succeed"
        );
    }

    #[test]
    fn nearest_config_prefers_nested_tsconfig_then_jsconfig() {
        let project = tempfile::tempdir().unwrap();
        let nested = project.path().join("pages");
        std::fs::create_dir_all(&nested).unwrap();
        let source = nested.join("widget.worker.ts");
        std::fs::write(&source, "export {};\n").unwrap();
        let root_config = project.path().join("tsconfig.json");
        let nested_jsconfig = nested.join("jsconfig.json");
        std::fs::write(&root_config, "{}").unwrap();
        std::fs::write(&nested_jsconfig, "{}").unwrap();

        assert_eq!(
            nearest_typescript_config(&source, None),
            Some(nested_jsconfig)
        );
        let nested_tsconfig = nested.join("tsconfig.json");
        std::fs::write(&nested_tsconfig, "{}").unwrap();
        assert_eq!(
            nearest_typescript_config(&source, None),
            Some(nested_tsconfig)
        );
    }

    #[test]
    fn nearest_config_honors_shadow_boundary_without_changing_physical_discovery() {
        let temp = tempfile::tempdir().unwrap();
        let ambient_config = temp.path().join("tsconfig.json");
        let shadow = temp.path().join("preprocess-shadow");
        let source = shadow.join("src/worker.ts");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&ambient_config, "{}").unwrap();
        std::fs::write(&source, "export {};\n").unwrap();

        assert_eq!(
            nearest_typescript_config(&source, None),
            Some(ambient_config),
            "physical/no-shadow jobs retain native ancestor discovery"
        );
        assert_eq!(
            nearest_typescript_config(&source, Some(&shadow)),
            None,
            "shadow jobs must not leak into unrelated tempdir ancestor configs"
        );

        let boundary_config = shadow.join("tsconfig.json");
        std::fs::write(&boundary_config, "{}").unwrap();
        assert_eq!(
            nearest_typescript_config(&source, Some(&shadow)),
            Some(boundary_config),
            "a synthetic boundary config remains discoverable at the boundary itself"
        );
    }

    #[test]
    fn json_string_escapes_specials() {
        assert_eq!(json_string("a/b"), "\"a/b\"");
        assert_eq!(json_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(json_string("a\\b"), "\"a\\\\b\"");
        assert_eq!(json_string("a\nb"), "\"a\\nb\"");
    }

    #[test]
    fn render_shared_bundle_entry_source_imports_each_island_in_order() {
        // The shared-bundle entry source must reference every island's
        // resolved `source_path` as a namespace import so esbuild's
        // single-input + `--outfile` invocation pulls them all into one
        // bundled output (issue #138 fix) AND keeps their exports alive
        // through tree-shaking even when the imported module has no
        // top-level side effect (issue #144 fix). Order follows the
        // input slice so the resulting bytes are deterministic across
        // runs.
        let islands = vec![
            Island::new("Counter", "/abs/components/Counter.tsx"),
            Island::new("Modal", "/abs/components/Modal.tsx"),
        ];
        let src = render_shared_bundle_entry_source(&islands, false);
        assert!(
            src.contains(r#"import * as __zfb_island_0 from "/abs/components/Counter.tsx";"#),
            "missing namespace-import for Counter: {src}"
        );
        assert!(
            src.contains(r#"import * as __zfb_island_1 from "/abs/components/Modal.tsx";"#),
            "missing namespace-import for Modal: {src}"
        );
        // Order check — Counter must appear before Modal in the
        // generated source so the bundle's content hash is stable for
        // identical inputs.
        let i_counter = src.find("Counter.tsx").expect("counter present");
        let i_modal = src.find("Modal.tsx").expect("modal present");
        assert!(i_counter < i_modal, "expected Counter before Modal");
        // The top-level `mountIslands(...)` invocation must reference
        // every namespace (via `__zfb_register(...)` calls), otherwise
        // esbuild's tree-shaker would drop the namespace imports as
        // unused (the bug #144 fixes) AND the page would never call
        // into the hydration runtime (the bug #146 fixes).
        assert!(
            src.contains(r#"import { mountIslands } from "@takazudo/zfb/runtime""#),
            "missing mountIslands import: {src}"
        );
        assert!(
            src.contains("mountIslands(__zfb_manifest);"),
            "missing mountIslands call: {src}"
        );
        assert!(
            src.contains("__zfb_register(__zfb_island_0, \"Counter\", \"Counter\","),
            "expected register call for Counter: {src}"
        );
        assert!(
            src.contains("__zfb_register(__zfb_island_1, \"Modal\", \"Modal\","),
            "expected register call for Modal: {src}"
        );
    }

    #[test]
    fn render_shared_bundle_entry_source_escapes_quotes_in_paths() {
        // Path containing a literal double-quote (rare but legal on
        // macOS / Linux) must not break the synthesized JS — it gets
        // JSON-escaped just like component names.
        let islands = vec![Island::new("Weird", "/abs/components/has\"quote/Weird.tsx")];
        let src = render_shared_bundle_entry_source(&islands, false);
        assert!(
            src.contains(
                r#"import * as __zfb_island_0 from "/abs/components/has\"quote/Weird.tsx";"#
            ),
            "expected escaped quote in path: {src}"
        );
    }

    #[test]
    fn render_shared_bundle_entry_source_handles_empty_islands_slice() {
        // Empty input emits a header-only entry (no `import` lines, no
        // mountIslands call) — `build_production_islands_asset` already
        // short-circuits on an empty islands slice, so this code path
        // is mostly defensive, but we still want it to produce valid JS
        // rather than panic or emit a `mountIslands({})` call that
        // would query the DOM for islands the page doesn't have.
        let src = render_shared_bundle_entry_source(&[], false);
        assert!(!src.contains("import"));
        assert!(!src.contains("mountIslands"));
        assert!(src.starts_with("// Generated"));
    }

    #[test]
    fn render_shared_bundle_entry_source_client_router_false_adds_zero_bytes() {
        // Acceptance criterion 2 (#289): a project NOT using
        // `<ClientRouter />` sees zero new bytes in its islands bundle.
        // The `client_router=true` output must equal the `false` output
        // with EXACTLY one extra line — the side-effect import inserted
        // right after the header — and nothing else. This is the test
        // that proves the flag is byte-neutral when off.
        let islands = vec![
            Island::new("Counter", "/abs/components/Counter.tsx"),
            Island::new("Modal", "/abs/components/Modal.tsx"),
        ];
        let without = render_shared_bundle_entry_source(&islands, false);
        let with = render_shared_bundle_entry_source(&islands, true);

        assert!(
            !without.contains("@takazudo/zfb-runtime/client-router"),
            "client_router=false must not mention the client-router subpath: {without}"
        );

        let header = "// Generated by zfb-islands::IslandsBundler::produce_bundle_js\n";
        let import_line = "import \"@takazudo/zfb-runtime/client-router\";\n";
        let body_without = without
            .strip_prefix(header)
            .expect("output starts with the generated-by header");
        let expected_with = format!("{header}{import_line}{body_without}");
        assert_eq!(
            with, expected_with,
            "client_router=true output must be the false output with exactly one inserted import line"
        );
    }

    #[test]
    fn render_shared_bundle_entry_source_client_router_true_with_no_islands() {
        // Issue #289: a `<ClientRouter />`-only project (no `"use client"`
        // islands) still gets the side-effect import — and nothing else
        // (the legacy Preact import stays absent, no mountIslands call) so
        // the entry stays minimal.
        let src = render_shared_bundle_entry_source(&[], true);
        assert!(src.contains("import \"@takazudo/zfb-runtime/client-router\";"));
        assert!(!src.contains("mountIslands"));
        assert!(!src.contains("from \"preact\""));
        assert!(src.starts_with("// Generated"));
    }

    #[test]
    fn render_shared_bundle_entry_source_client_router_true_with_islands() {
        // With both islands and client-router, the side-effect import and
        // the normal island registration shape coexist.
        let islands = vec![Island::new("Counter", "/abs/components/Counter.tsx")];
        let src = render_shared_bundle_entry_source(&islands, true);
        assert!(src.contains("import \"@takazudo/zfb-runtime/client-router\";"));
        assert!(src.contains(r#"import * as __zfb_island_0 from "/abs/components/Counter.tsx";"#));
        assert!(src.contains("mountIslands(__zfb_manifest);"));
    }

    #[test]
    fn render_shared_bundle_entry_source_namespace_identifiers_are_sequential_and_unique() {
        // Two islands whose base names collide (host `theme-toggle.tsx`
        // and v2 `theme-toggle.tsx`, the real-world case from
        // zudolab/zudo-doc#1355 Wave 5) must end up under DIFFERENT
        // namespace identifiers so neither namespace shadows the other
        // — that's why we use sequential numeric suffixes rather than a
        // base-name-derived identifier.
        let islands = vec![
            Island::new("ThemeToggle", "/host/src/components/theme-toggle.tsx"),
            Island::new(
                "ThemeToggle",
                "/v2/packages/zudo-doc-v2/src/theme/theme-toggle.tsx",
            ),
            Island::new(
                "Sidebar",
                "/v2/packages/zudo-doc-v2/src/sidebar/sidebar.tsx",
            ),
        ];
        let src = render_shared_bundle_entry_source(&islands, false);
        assert!(src.contains("import * as __zfb_island_0 from "));
        assert!(src.contains("import * as __zfb_island_1 from "));
        assert!(src.contains("import * as __zfb_island_2 from "));
        // Every namespace identifier must appear in a `__zfb_register`
        // call so esbuild keeps each module's exports alive.
        assert!(src.contains("__zfb_register(__zfb_island_0, "));
        assert!(src.contains("__zfb_register(__zfb_island_1, "));
        assert!(src.contains("__zfb_register(__zfb_island_2, "));
    }

    #[test]
    fn render_shared_bundle_entry_source_calls_mount_islands() {
        // Regression for issue #146 / zudolab/zudo-doc#1355 Wave 6:
        // the shared-bundle production path must call `mountIslands` at
        // top level so the SSR'd `data-zfb-island` markers actually get
        // hydrated. Before the fix, the synthesised entry only anchored
        // namespaces against tree-shaking via
        // `(globalThis).__zfb_islands ??= [...]` — every island's
        // source code shipped, but no code ran the hydration glue, so
        // interactivity never activated.
        let islands = vec![
            Island::new("Counter", "/abs/components/Counter.tsx"),
            Island::new("Modal", "/abs/components/Modal.tsx"),
        ];
        let src = render_shared_bundle_entry_source(&islands, false);

        assert!(
            src.contains(r#"import { mountIslands } from "@takazudo/zfb/runtime""#),
            "missing mountIslands import: {src}"
        );
        assert!(
            src.contains(r#"import { hydrate, mount } from "@takazudo/zfb/zudo-react/client""#),
            "missing owned glue imports: {src}"
        );

        // Helper functions present, plus an `__zfb_register` call per
        // island. Issue #149: the manifest key (third arg) is now a
        // **static literal** — the scanner-derived `marker_name`. The owned
        // glue checks the function name against that marker before mounting.
        assert!(src.contains("function __zfb_pick("));
        assert!(src.contains("Object.prototype.hasOwnProperty.call(ns, exportName)"));
        assert!(!src.contains("return ns.default"));
        assert!(src.contains("duplicate owned island marker"));
        assert!(src.contains("C.displayName !== name"));
        assert!(src.contains("if (!name"));
        assert!(src.contains("function __zfb_register("));
        assert!(
            !src.contains("function __zfb_keyFor("),
            "issue #149: runtime introspection helper must be gone:\n{src}"
        );
        assert!(src.contains("__zfb_register(__zfb_island_0, \"Counter\", \"Counter\","));
        assert!(src.contains("__zfb_register(__zfb_island_1, \"Modal\", \"Modal\","));

        // hydrate vs render branching follows the runtime contract.
        assert!(src.contains("mode === \"hydrate\" ? hydrate(h(C, props)"));
        assert!(src.contains("mount(h(C, props)"));

        // The owned runtime receives RootHandles from the mount thunk.
        assert!(
            !src.contains("unmount: (element)"),
            "legacy unmount thunk must be absent: {src}"
        );

        // Final invocation hands the populated manifest to the runtime.
        assert!(src.contains("mountIslands(__zfb_manifest);"));
    }

    #[test]
    fn render_shared_bundle_entry_source_uses_marker_name_for_default_export_islands() {
        // Regression for issue #149 (zudolab/zudo-doc#1355 Wave 7):
        // host-shape islands authored as
        // `export default function FooBar(...)` are recorded by the
        // scanner with `component_name = "default"` AND
        // `marker_name = "FooBar"` (the function identifier name, which
        // matches what `function.name` produces at SSR time).
        //
        // The synthesised bundle entry must register the manifest entry
        // under `marker_name`, NOT `component_name`. Before this fix
        // (issue #149 Gap B), every host-shape default-export island
        // collided on the literal `"default"` slot because esbuild
        // minification renamed the actual functions and broke runtime
        // `displayName ?? name` introspection.
        let islands = vec![
            Island::with_marker_name(
                "default",
                "/abs/components/sidebar-toggle.tsx",
                "SidebarToggle",
            ),
            Island::with_marker_name("default", "/abs/components/theme-toggle.tsx", "ThemeToggle"),
            Island::with_marker_name(
                "default",
                "/abs/components/ai-chat-modal.tsx",
                "AiChatModal",
            ),
        ];
        let src = render_shared_bundle_entry_source(&islands, false);

        // The third argument is the SSR-marker name, distinct from the
        // export-side `component_name = "default"`. Static literal —
        // not derived at runtime.
        assert!(
            src.contains("__zfb_register(__zfb_island_0, \"default\", \"SidebarToggle\","),
            "expected SidebarToggle marker:\n{src}"
        );
        assert!(
            src.contains("__zfb_register(__zfb_island_1, \"default\", \"ThemeToggle\","),
            "expected ThemeToggle marker:\n{src}"
        );
        assert!(
            src.contains("__zfb_register(__zfb_island_2, \"default\", \"AiChatModal\","),
            "expected AiChatModal marker:\n{src}"
        );

        // Marker names must be all distinct so no two islands collide
        // on the same manifest slot.
        assert!(src.contains("\"SidebarToggle\""));
        assert!(src.contains("\"ThemeToggle\""));
        assert!(src.contains("\"AiChatModal\""));
    }

    #[test]
    fn render_shared_bundle_entry_source_uses_marker_name_for_ssr_skip_wrappers() {
        // Regression for issue #149 Gap A: SSR-skip wrappers like
        // `AiChatModalIsland` are exported under their wrapper name but
        // emit `data-zfb-island-skip-ssr="AiChatModal"` (no "Island"
        // suffix) via `renderSsrSkipPlaceholder("AiChatModal", …)`. The
        // bundle's manifest must therefore key on "AiChatModal", not
        // "AiChatModalIsland".
        //
        // The scanner extracts the literal first argument from the
        // helper call and stores it as `marker_name`. The bundler then
        // bakes that as the static third arg of `__zfb_register`.
        let islands = vec![
            Island::with_marker_name(
                "AiChatModalIsland",
                "/abs/components/ai-chat-modal-island.tsx",
                "AiChatModal",
            ),
            Island::with_marker_name(
                "ImageEnlargeIsland",
                "/abs/components/image-enlarge-island.tsx",
                "ImageEnlarge",
            ),
        ];
        let src = render_shared_bundle_entry_source(&islands, false);

        // Lookup uses the wrapper export name (so the import * as ns
        // round-trip lands on the wrapper component). The manifest key
        // is the SSR marker name.
        assert!(
            src.contains("__zfb_register(__zfb_island_0, \"AiChatModalIsland\", \"AiChatModal\",")
        );
        assert!(src
            .contains("__zfb_register(__zfb_island_1, \"ImageEnlargeIsland\", \"ImageEnlarge\","));
    }

    /// Regression for issue #138.
    ///
    /// Before the fix, `produce_bundle_js` passed N island
    /// `source_path`s as N separate inputs to esbuild plus a single
    /// `--outfile`, and esbuild bailed with "Must use \"outdir\" when
    /// there are multiple input files" for any N >= 2. The fix
    /// synthesizes a single-entry source via
    /// [`render_shared_bundle_entry_source`] and routes through
    /// [`IslandsBundler::bundle_one_entry`] so esbuild always
    /// sees exactly one input.
    ///
    /// We exercise the mock path with an empty `mock_output` so
    /// `bundle_one_entry` echoes the synthesized entry source back; the
    /// test then asserts the echoed string contains the multi-island
    /// import shape (which is what would have made esbuild fail with
    /// the old multi-input code path).
    #[test]
    fn bundle_handles_multiple_islands_via_synthetic_entry() {
        let dir = tempfile::tempdir().unwrap();
        let bundler = IslandsBundler::new(IslandsBundlerConfig {
            mock_bundle: true,
            // Empty mock_output → bundle_one_entry returns the entry
            // source verbatim. That lets us assert against the
            // synthesized shape end-to-end.
            ..IslandsBundlerConfig::default()
        });
        let islands = vec![
            Island::new("Counter", "/abs/components/Counter.tsx"),
            Island::new("Modal", "/abs/components/Modal.tsx"),
            Island::new("Sidebar", "/abs/components/Sidebar.tsx"),
        ];
        let cfg = BundleConfig::default()
            .with_zudo_react_build(Some(TEST_BUILD_TOKEN.to_string()))
            .with_outdir(dir.path().to_path_buf());

        let out = bundler.bundle(&islands, &cfg).expect("multi-island bundle");

        // Stable filename + URL — the multi-input fix must NOT change
        // the asset_path / asset_url shape.
        assert_eq!(out.asset_url, "/assets/islands.js");
        assert_eq!(out.asset_path, dir.path().join("assets").join("islands.js"));
        // Bundler carries bytes in memory — no disk write.
        assert!(!out.asset_path.exists(), "bundler must not write to disk");

        // Module IDs preserve input order.
        assert_eq!(out.module_ids, vec!["Counter", "Modal", "Sidebar"]);

        // The in-memory bytes are the synthesized entry source (echoed by
        // mock mode) — verify each island path appears in it under the
        // namespace-import shape (issue #144 fix). The previous shape
        // (`import "<path>";` side-effect import) tree-shook every
        // island whose body had no top-level effect.
        //
        // The bytes also carry the `mountIslands(...)` invocation that
        // the issue #146 fix added so the SSR'd markers hydrate.
        let in_memory = String::from_utf8(out.bytes).expect("bytes are valid UTF-8");
        assert!(
            in_memory.contains(r#"import * as __zfb_island_0 from "/abs/components/Counter.tsx";"#)
        );
        assert!(
            in_memory.contains(r#"import * as __zfb_island_1 from "/abs/components/Modal.tsx";"#)
        );
        assert!(
            in_memory.contains(r#"import * as __zfb_island_2 from "/abs/components/Sidebar.tsx";"#)
        );
        assert!(in_memory.contains(r#"import { mountIslands } from "@takazudo/zfb/runtime""#));
        assert!(in_memory.contains("mountIslands(__zfb_manifest);"));
        assert!(in_memory.contains("__zfb_register(__zfb_island_0, \"Counter\", \"Counter\","));
        assert!(in_memory.contains("__zfb_register(__zfb_island_1, \"Modal\", \"Modal\","));
        assert!(in_memory.contains("__zfb_register(__zfb_island_2, \"Sidebar\", \"Sidebar\","));
    }

    /// The shared-entry request `bundle_one_entry` builds. `splitting` selects
    /// the shared-bundle path (`true`) vs the per-island/runtime path (`false`).
    fn shared_request(config: &BundleConfig, splitting: bool) -> BundleRequest {
        shared_request_with_graph(config, splitting, None)
    }

    fn shared_request_with_graph(
        config: &BundleConfig,
        splitting: bool,
        stage_audit_metafile: Option<&Path>,
    ) -> BundleRequest {
        build_bundle_request(
            &IslandsBundlerConfig::default().with_working_dir("/tmp/zfb-proj"),
            config,
            None,
            None,
            JobShape::shared_entry(
                Path::new("/tmp/zfb-out"),
                Path::new("/tmp/entry.tsx"),
                splitting,
                stage_audit_metafile,
            ),
        )
        .expect("bundle request")
    }

    fn named_request(
        config: &BundleConfig,
        entry: &str,
        entry_name: &str,
        stage_audit_metafile: Option<&Path>,
    ) -> BundleRequest {
        build_bundle_request(
            &IslandsBundlerConfig::default().with_working_dir("/tmp/zfb-proj"),
            config,
            None,
            None,
            JobShape::named(
                Path::new("/tmp/zfb-out"),
                Path::new(entry),
                entry_name,
                stage_audit_metafile,
            ),
        )
        .expect("bundle request")
    }

    /// `(entry_names, chunk_names, asset_names)` of a staging-directory job.
    fn layout_names(request: &BundleRequest) -> (Option<&str>, Option<&str>, Option<&str>) {
        match &request.output {
            OutputLayout::Dir {
                dir,
                entry_names,
                chunk_names,
                asset_names,
            } => {
                assert_eq!(dir, Path::new("/tmp/zfb-out"));
                (
                    entry_names.as_deref(),
                    chunk_names.as_deref(),
                    asset_names.as_deref(),
                )
            }
            other => panic!("every job writes a staging directory: {other:?}"),
        }
    }

    fn loader_of(request: &BundleRequest, extension: &str) -> Option<Loader> {
        request
            .loaders
            .iter()
            .find(|(key, _)| key == extension)
            .map(|(_, loader)| *loader)
    }

    fn define_of<'r>(request: &'r BundleRequest, key: &str) -> Option<&'r str> {
        request
            .defines
            .iter()
            .rev()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value.as_str())
    }

    // -----------------------------------------------------------------------
    // Stage-escape audit policy (#1705): argv gating, metafile allocation,
    // and job-identity propagation. These are pure/no-subprocess tests —
    // a real-esbuild end-to-end proof is left for a follow-up confirm pass
    // (no pinned esbuild binary was available to author/verify one here).
    // -----------------------------------------------------------------------

    /// `splitting == false` (per-island / runtime / worker / client-script
    /// passes) is the ONLY case that honours `stage_audit_metafile` — no
    /// `--metafile` appears at all without a policy.
    #[test]
    fn bundle_request_omits_graph_without_a_stage_audit_policy() {
        let request = shared_request(&BundleConfig::production(), false);
        assert_eq!(
            request.graph, None,
            "no policy configured, no graph file expected: {request:?}"
        );
    }

    #[test]
    fn bundle_request_writes_requested_graph_when_stage_audit_metafile_is_some() {
        let metafile = PathBuf::from("/tmp/zfb-out/.zfb-stage-audit-entry.json");
        let request =
            shared_request_with_graph(&BundleConfig::production(), false, Some(&metafile));
        assert_eq!(request.graph, Some(metafile));
    }

    /// The resource-contract pass (`splitting == true`) already emits its
    /// own mandatory `--metafile=meta.json`; a caller-supplied
    /// `stage_audit_metafile` must be silently ignored there so the
    /// invocation never carries two `--metafile` flags (esbuild accepts
    /// only one).
    #[test]
    fn bundle_request_resource_contract_pass_reuses_its_own_graph() {
        let would_be_stage_audit_metafile =
            PathBuf::from("/tmp/zfb-out/.zfb-stage-audit-entry.json");
        let request = shared_request_with_graph(
            &BundleConfig::production(),
            true,
            Some(&would_be_stage_audit_metafile),
        );
        assert_eq!(
            request.graph,
            Some(Path::new("/tmp/zfb-out").join(RESOURCE_METAFILE_FILENAME)),
            "the resource-contract graph is the one the audit reads: {request:?}"
        );
    }

    /// `build_esbuild_args_with_entry_name` is the shared builder for the
    /// worker loop and the client-script jobs loop — same gating contract
    /// as `build_esbuild_args`.
    #[test]
    fn named_bundle_request_writes_graph_when_stage_audit_metafile_is_some() {
        let metafile =
            PathBuf::from("/tmp/zfb-out/.zfb-stage-audit-worker-src-s-search-d-worker-d-ts.json");
        let request = named_request(
            &BundleConfig::production(),
            "/tmp/worker.ts",
            "worker-src-s-search-d-worker-d-ts",
            Some(&metafile),
        );
        assert_eq!(request.graph, Some(metafile));
    }

    #[test]
    fn stage_audit_metafile_is_none_without_a_policy() {
        assert!(stage_audit_metafile(None).unwrap().is_none());
    }

    /// `stage_audit_metafile` deliberately takes no `out_dir` parameter — a
    /// codex review finding on the first version of this wiring flagged that
    /// nesting the scratch metafile inside an esbuild `--outdir` makes the
    /// strict output read-back (`read_back_outdir_with_resource_oracle`,
    /// `read_back_client_script_outdir`) reject it as an unrecognised
    /// output on every job type except the resource-contract pass. Dropping
    /// the parameter makes that bug class structurally unreachable; this
    /// test pins the remaining per-job contract: every call allocates a
    /// distinct, real, on-disk file.
    #[test]
    fn stage_audit_metafile_allocates_a_distinct_real_file_per_call() {
        let policy = StageAuditPolicy {
            stage_roots: vec![PathBuf::from("/stage")],
            first_party_root: PathBuf::from("/workspace"),
        };
        let entry_metafile = stage_audit_metafile(Some(&policy)).unwrap().unwrap();
        let worker_metafile = stage_audit_metafile(Some(&policy)).unwrap().unwrap();
        assert_ne!(entry_metafile.path(), worker_metafile.path());
        assert!(entry_metafile.path().is_file());
        assert!(worker_metafile.path().is_file());
    }

    /// Shared entry, worker, client script, and client-script worker jobs
    /// share one audit helper. Failures must carry the caller's job label.
    #[test]
    fn run_stage_escape_audit_failure_surfaces_owning_job_identity_for_every_job_type() {
        let policy = StageAuditPolicy {
            stage_roots: vec![PathBuf::from("/nonexistent/stage")],
            first_party_root: PathBuf::from("/nonexistent/workspace"),
        };
        // A metafile that does not exist deterministically fails the audit
        // (via `audit_metafile_stage_escape_at_path`'s own read step) without
        // needing a real esbuild subprocess or a crafted offending metafile.
        let missing_metafile = PathBuf::from("/nonexistent/.zfb-stage-audit-missing.json");
        let job_labels = [
            "shared entry".to_string(),
            format!("shared worker {}", "worker-src-s-search-d-worker-d-ts.js"),
            format!(
                "client script {:?}",
                PathBuf::from("/proj/pages/search-widget.client.ts")
            ),
            format!(
                "client-script module worker {:?}",
                PathBuf::from("/proj/pages/search.worker.ts")
            ),
        ];
        for job_label in job_labels {
            let err = run_stage_escape_audit(
                &policy,
                &missing_metafile,
                Path::new("/nonexistent"),
                &job_label,
            )
            .expect_err("a missing metafile must fail the audit");
            let msg = err.to_string();
            assert!(
                msg.contains(&job_label),
                "expected job identity {job_label:?} in error, got: {msg}"
            );
        }
    }

    /// Mock subprocesses never emit or require a metafile, even when a
    /// stage-audit policy is configured — covers the shared entry, per-island,
    /// and runtime job types (all routed through `bundle_one_entry`'s mock
    /// early-return). Pointing the policy at nonexistent paths means any
    /// accidental real audit attempt would fail loudly instead of silently
    /// passing.
    #[test]
    fn mock_subprocess_bundle_skips_stage_audit_entirely() {
        let policy = StageAuditPolicy {
            stage_roots: vec![PathBuf::from("/nonexistent/stage")],
            first_party_root: PathBuf::from("/nonexistent/workspace"),
        };
        let cfg = IslandsBundlerConfig::default()
            .with_mock_output("export const Counter = () => null;\n")
            .with_stage_audit(policy);
        let bundler = IslandsBundler::new(cfg);
        let tmp = tempfile::tempdir().unwrap();
        let bundle_cfg = BundleConfig::production()
            .with_zudo_react_build(Some(TEST_BUILD_TOKEN.to_string()))
            .with_outdir(tmp.path());
        let result = bundler.bundle(
            &[Island::new(
                "Counter",
                PathBuf::from("components/counter.tsx"),
            )],
            &bundle_cfg,
        );
        assert!(
            result.is_ok(),
            "mock mode must skip the stage-escape audit entirely: {result:?}"
        );
    }

    #[test]
    fn browser_bundler_rejects_duplicate_marker_before_emitting() {
        let bundler = IslandsBundler::new(IslandsBundlerConfig::default().with_mock_output(""));
        let config =
            BundleConfig::production().with_zudo_react_build(Some(TEST_BUILD_TOKEN.to_string()));
        let error = bundler
            .bundle(
                &[
                    Island::with_marker_name("A", "/components/a.tsx", "Same"),
                    Island::with_marker_name("B", "/components/b.tsx", "Same"),
                ],
                &config,
            )
            .unwrap_err();
        assert!(error.to_string().contains("duplicate owned island marker"));
    }

    /// Mirrors the mock-mode guarantee above for the client-script path
    /// (covers the client entry and client-script module worker job types).
    #[test]
    fn mock_subprocess_client_script_skips_stage_audit_entirely() {
        let policy = StageAuditPolicy {
            stage_roots: vec![PathBuf::from("/nonexistent/stage")],
            first_party_root: PathBuf::from("/nonexistent/workspace"),
        };
        let cfg = IslandsBundlerConfig::default()
            .with_mock_output("// bundled")
            .with_stage_audit(policy);
        let bundler = IslandsBundler::new(cfg);
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("widget.client.ts");
        let worker = dir.path().join("worker.ts");
        std::fs::write(&entry, "console.log('entry');\n").unwrap();
        std::fs::write(&worker, "self.postMessage('worker');\n").unwrap();
        let workers = vec![ClientScriptWorkerEntry {
            filename: "worker-worker-d-ts.js".into(),
            source_path: worker,
        }];
        let result = bundler.bundle_client_script_file_with_workers(
            "widget",
            &entry,
            &workers,
            &BundleConfig::production(),
        );
        assert!(
            result.is_ok(),
            "mock mode must skip the stage-escape audit entirely: {result:?}"
        );
    }

    fn write_resource_metafile(metafile_path: &Path, outputs: &[(PathBuf, &str)]) -> Result<()> {
        let mut metafile_outputs = serde_json::Map::new();
        for (output_path, source_path) in outputs {
            let mut inputs = serde_json::Map::new();
            inputs.insert(
                (*source_path).to_string(),
                serde_json::json!({ "bytesInOutput": 1 }),
            );
            metafile_outputs.insert(
                output_path.to_string_lossy().into_owned(),
                serde_json::json!({ "inputs": inputs }),
            );
        }
        std::fs::write(
            metafile_path,
            serde_json::to_vec(&serde_json::json!({ "outputs": metafile_outputs }))?,
        )?;
        Ok(())
    }

    /// Code-splitting contract (#806): the shared-bundle esbuild invocation
    /// must enable `--splitting=true`, write to a directory via `--outdir`
    /// (not `--outfile`), and pin the stable entry name + self-hashed flat
    /// chunk-name template. These flags are what let a consumer's dynamic
    /// `import()` land in its own chunk instead of inlining into the
    /// multi-MB entry every page loads.
    #[test]
    fn bundle_request_enables_splitting_with_outdir_and_naming() {
        let request = shared_request(&BundleConfig::default(), true);
        assert!(request.splitting, "shared pass must split: {request:?}");
        assert_eq!(
            layout_names(&request),
            (
                Some("islands"),
                Some("islands-chunk-[hash]"),
                Some("islands-resource-[name]-[hash]")
            )
        );
    }

    /// The shared browser-islands pass alone owns the locked file-loader
    /// contract. Its flags come after user loader arguments and include the
    /// private metafile read-back oracle; non-splitting client-script jobs
    /// retain their separate argument contract.
    #[test]
    fn shared_bundle_request_includes_locked_resource_contract_only_when_splitting() {
        let cfg = BundleConfig::default()
            .with_loaders(BTreeMap::from([(".txt".to_string(), "text".to_string())]));
        let shared = shared_request(&cfg, true);
        assert_eq!(loader_of(&shared, ".txt"), Some(Loader::Text));
        assert_eq!(loader_of(&shared, ".zfb-resource.mjs"), Some(Loader::File));
        assert_eq!(loader_of(&shared, ".wasm"), Some(Loader::File));
        assert_eq!(
            layout_names(&shared).2,
            Some("islands-resource-[name]-[hash]"),
            "missing locked resource naming: {shared:?}"
        );
        assert_eq!(
            shared.graph,
            Some(PathBuf::from("/tmp/zfb-out/meta.json")),
            "missing graph oracle: {shared:?}"
        );

        // The locked loaders win over a configured override that reached the
        // builder directly.
        let overridden = shared_request(
            &BundleConfig::default()
                .with_loaders(BTreeMap::from([(".wasm".to_string(), "empty".to_string())])),
            true,
        );
        assert_eq!(loader_of(&overridden, ".wasm"), Some(Loader::File));

        let per_island = shared_request(&cfg, false);
        assert_eq!(
            loader_of(&per_island, ".zfb-resource.mjs"),
            None,
            "per-island request must not acquire the resource contract: {per_island:?}"
        );
        assert_eq!(loader_of(&per_island, ".wasm"), None);
        assert_eq!(
            layout_names(&per_island).2,
            None,
            "per-island request must not acquire resource output naming: {per_island:?}"
        );
        assert_eq!(per_island.graph, None);
    }

    #[test]
    fn shared_bundle_rejects_reserved_resource_loader_overrides() {
        for extension in [ZFB_RESOURCE_MJS_EXTENSION, WASM_EXTENSION] {
            let bundler =
                IslandsBundler::new(IslandsBundlerConfig::default().with_mock_output("export {};"));
            let cfg = BundleConfig::default()
                .with_zudo_react_build(Some(TEST_BUILD_TOKEN.to_string()))
                .with_loaders(BTreeMap::from([(
                    extension.to_string(),
                    "empty".to_string(),
                )]));
            let err = bundler
                .bundle(&[Island::new("ResourceIsland", "/tmp/resource.tsx")], &cfg)
                .expect_err("reserved shared resource loader must be rejected");
            let message = err.to_string();
            assert!(message.contains(extension), "{message}");
            assert!(message.contains("reserved"), "{message}");
        }
    }

    /// Non-splitting jobs omit `--splitting` and `--chunk-names`; esbuild
    /// inlines dynamic imports into their single output file.
    #[test]
    fn bundle_request_non_splitting_path_disables_splitting() {
        let request = shared_request(&BundleConfig::default(), false);
        assert!(
            !request.splitting,
            "non-splitting path must not split: {request:?}"
        );
        // The stable-entry read-back contract still holds without splitting.
        assert_eq!(layout_names(&request), (Some("islands"), None, None));
    }

    /// `validate_chunk_filename` accepts exactly the two reserved companion
    /// classes and rejects pathological names (path separators, traversal,
    /// malformed workers, unexpected prefix) so read-back can never smuggle
    /// an arbitrary file into `BundleOutput`.
    #[test]
    fn validate_chunk_filename_accepts_and_rejects() {
        assert_eq!(
            validate_chunk_filename("islands-chunk-WOEGGERP.js").unwrap(),
            OutputFilenameClass::Chunk
        );
        assert_eq!(
            validate_chunk_filename("worker-src-s-search-d-worker-d-ts.js").unwrap(),
            OutputFilenameClass::Worker
        );
        assert_eq!(
            validate_chunk_filename("islands-resource-glue.zfb-resource-ABCDE.mjs").unwrap(),
            OutputFilenameClass::Resource
        );
        assert_eq!(
            validate_chunk_filename("islands-resource-payload-ABCDE.wasm").unwrap(),
            OutputFilenameClass::Resource
        );

        assert!(validate_chunk_filename("../islands-chunk-X.js").is_err());
        assert!(validate_chunk_filename("nested/islands-chunk-X.js").is_err());
        assert!(validate_chunk_filename("islands-chunk-..").is_err());
        assert!(validate_chunk_filename("islands-chunk-.js").is_err());
        assert!(validate_chunk_filename("islands-chunk-AAAA.css").is_err());
        assert!(validate_chunk_filename("worker-.js").is_err());
        assert!(validate_chunk_filename("worker-bad.name.js").is_err());
        assert!(validate_chunk_filename("worker-valid-name.css").is_err());
        assert!(validate_chunk_filename("islands-resource-nohash.wasm").is_err());
        assert!(validate_chunk_filename("islands-resource-name-ABCDE.txt").is_err());
        assert!(validate_chunk_filename("islands-resource-..-ABCDE.wasm").is_err());
        assert!(validate_chunk_filename("evil.js").is_err());
        assert!(validate_chunk_filename("islands.js").is_err());
    }

    /// Worker passes inherit the shared browser mode/loaders/defines while
    /// deliberately disabling splitting and pinning the full contract name.
    #[test]
    fn module_worker_request_is_browser_non_splitting_and_inherits_config() {
        let cfg = BundleConfig::dev()
            .with_loaders(BTreeMap::from([(".txt".to_string(), "text".to_string())]))
            .with_define(BTreeMap::from([(
                "__WORKER_FLAG__".to_string(),
                "true".to_string(),
            )]));
        let request = named_request(
            &cfg,
            "/app/src/search.worker.ts",
            "worker-src-s-search-d-worker-d-ts",
            None,
        );

        assert_eq!(request.platform, Platform::Browser);
        assert!(
            !request.splitting,
            "worker bundles must be self-contained: {request:?}"
        );
        assert_eq!(
            layout_names(&request),
            (Some("worker-src-s-search-d-worker-d-ts"), None, None)
        );
        assert_eq!(loader_of(&request, ".txt"), Some(Loader::Text));
        assert_eq!(define_of(&request, "__WORKER_FLAG__"), Some("true"));
        assert_eq!(define_of(&request, "import.meta.env.DEV"), Some("true"));
    }

    #[test]
    fn duplicate_module_worker_outputs_are_rejected_in_mock_mode_too() {
        let project = tempfile::tempdir().unwrap();
        let source = project.path().join("worker.ts");
        std::fs::write(&source, "self.postMessage('ok');").unwrap();
        let worker = ModuleWorkerBundleEntry::new(project.path(), &source, &source).unwrap();
        let config = BundleConfig::dev()
            .with_zudo_react_build(Some(TEST_BUILD_TOKEN.to_string()))
            .with_module_workers(vec![worker.clone(), worker]);
        let bundler =
            IslandsBundler::new(IslandsBundlerConfig::default().with_mock_output("export {};"));

        let error = bundler
            .bundle(&[Island::new("Island", &source)], &config)
            .expect_err("duplicate stable worker filenames must not overwrite each other");
        assert!(
            format!("{error}").contains("duplicate module-worker output filename"),
            "{error}"
        );
    }

    /// Issue #1395: a residual `.css` output must fail with the targeted
    /// "handled by the zfb CSS pipeline" message rather than the generic
    /// "unexpected output file" wording — the CSS-specific branch must be
    /// checked before the generic prefix check below it.
    #[test]
    fn validate_chunk_filename_rejects_css_with_targeted_message() {
        let err = validate_chunk_filename("styles.css")
            .expect_err("a residual .css output must be rejected");
        let msg = format!("{err}");
        assert!(
            msg.contains("zfb CSS pipeline"),
            "expected the targeted CSS-pipeline message, got: {msg}"
        );
        assert!(
            !msg.contains("unexpected output file"),
            "CSS rejection must not fall through to the generic message: {msg}"
        );

        let err2 = validate_chunk_filename("islands-chunk-WOEGGERP.css")
            .expect_err("a chunk-prefixed .css output must still be rejected as CSS");
        assert!(
            format!("{err2}").contains("zfb CSS pipeline"),
            "expected the targeted CSS-pipeline message even with the chunk prefix: {err2}"
        );
    }

    /// Issue #1395 + #1410: the islands esbuild arg set must carry BOTH
    /// `--loader:.css=empty` and `--loader:.module.css=empty` (mirroring the
    /// SSR bundler's `ESBUILD_LOADER_ARGS`) so an island's `import "./x.css"`
    /// / `import styles from "./x.module.css"` neutralises at compile time
    /// instead of esbuild writing a sibling CSS output file that
    /// `read_back_outdir` would reject. The explicit `.module.css` rule is
    /// load-bearing on esbuild 0.25: its native `local-css` loader claims
    /// `.module.css` unless a more specific rule overrides it (#1410).
    /// Checked on both the shared-bundle (`splitting = true`) and per-island
    /// (`splitting = false`) arg shapes since both call the same builder.
    #[test]
    fn bundle_request_includes_css_empty_loader() {
        let cfg = BundleConfig::default();
        for splitting in [true, false] {
            let request = shared_request(&cfg, splitting);
            assert_eq!(
                loader_of(&request, ".css"),
                Some(Loader::Empty),
                "missing empty .css loader (splitting={splitting}): {request:?}"
            );
            assert_eq!(
                loader_of(&request, ".module.css"),
                Some(Loader::Empty),
                "missing empty .module.css loader (splitting={splitting}): {request:?}"
            );
        }
    }

    /// Issue #1404: `--preserve-symlinks` is emitted iff
    /// `BundleConfig::preserve_symlinks` is set, and is absent by default so
    /// the no-shadow argv stays byte-identical to a pre-#1404 build.
    #[test]
    fn bundle_request_preserves_symlinks_only_when_configured() {
        for splitting in [true, false] {
            let off = shared_request(&BundleConfig::default(), splitting);
            assert!(
                !off.resolve.preserve_symlinks,
                "default config must NOT preserve symlinks (splitting={splitting}): {off:?}"
            );

            let on = shared_request(
                &BundleConfig::default().with_preserve_symlinks(true),
                splitting,
            );
            assert!(
                on.resolve.preserve_symlinks,
                "preserve_symlinks=true must preserve symlinks (splitting={splitting}): {on:?}"
            );
        }
    }

    /// `read_back_outdir`: a directory holding only the entry (the
    /// zero-dynamic-import case) yields the entry JS and ZERO chunks — the
    /// non-splitting path stays single-file, identical to pre-#806.
    #[test]
    fn read_back_outdir_entry_only_yields_no_chunks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("islands.js"), b"export const x = 1;\n").unwrap();

        let out = read_back_outdir(dir.path()).expect("read back");
        assert_eq!(out.js, "export const x = 1;\n");
        assert!(
            out.chunks.is_empty(),
            "no chunks expected: {:?}",
            out.chunks
        );
        assert!(out.workers.is_empty(), "no workers expected");
        assert!(out.resources.is_empty(), "no resources expected");
    }

    #[test]
    fn read_back_shared_outdir_keeps_zero_resource_output_compatible() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("islands.js"), b"export const x = 1;\n").unwrap();
        write_resource_metafile(&dir.path().join(RESOURCE_METAFILE_FILENAME), &[]).unwrap();

        let out = read_back_shared_outdir(
            dir.path(),
            &dir.path().join(RESOURCE_METAFILE_FILENAME),
            dir.path(),
        )
        .expect("zero resources remain valid");
        assert!(out.chunks.is_empty());
        assert!(out.workers.is_empty());
        assert!(out.resources.is_empty());
    }

    #[test]
    fn read_back_shared_outdir_collects_metafile_backed_resources_sorted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("islands.js"), b"entry\n").unwrap();
        let wasm = dir.path().join("islands-resource-payload-ABCDE.wasm");
        let glue = dir
            .path()
            .join("islands-resource-glue.zfb-resource-ABCDE.mjs");
        // Write in reverse filename order to prove output ordering is not
        // inherited from filesystem iteration order.
        std::fs::write(&wasm, [0_u8, 97, 115, 109]).unwrap();
        std::fs::write(&glue, b"export const glue = true;\n").unwrap();
        let metafile = dir.path().join(RESOURCE_METAFILE_FILENAME);
        write_resource_metafile(
            &metafile,
            &[
                (wasm.clone(), "/fixture/payload.wasm?url"),
                (glue.clone(), "/fixture/glue.zfb-resource.mjs?url"),
            ],
        )
        .unwrap();

        let out =
            read_back_shared_outdir(dir.path(), &metafile, dir.path()).expect("read resources");
        assert_eq!(
            out.resources
                .iter()
                .map(|resource| resource.filename.as_str())
                .collect::<Vec<_>>(),
            vec![
                "islands-resource-glue.zfb-resource-ABCDE.mjs",
                "islands-resource-payload-ABCDE.wasm",
            ]
        );
        assert_eq!(out.resources[0].bytes, b"export const glue = true;\n");
        assert_eq!(out.resources[1].bytes, [0_u8, 97, 115, 109]);
    }

    #[test]
    fn resource_metafile_edge_only_accepts_the_exact_url_query() {
        assert!(metafile_input_matches_resource(
            "/fixture/payload.wasm",
            WASM_EXTENSION
        ));
        assert!(metafile_input_matches_resource(
            "/fixture/payload.wasm?url",
            WASM_EXTENSION
        ));
        assert!(metafile_input_matches_resource(
            "/fixture/glue.zfb-resource.mjs?url",
            ZFB_RESOURCE_MJS_EXTENSION
        ));
        assert!(!metafile_input_matches_resource(
            "/fixture/payload.wasm?raw",
            WASM_EXTENSION
        ));
        assert!(!metafile_input_matches_resource(
            "/fixture/payload.wasm?url&raw",
            WASM_EXTENSION
        ));
    }

    #[test]
    fn read_back_shared_outdir_resolves_relative_metafile_output_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("islands.js"), b"entry\n").unwrap();
        let resource = dir
            .path()
            .join("islands-resource-glue.zfb-resource-ABCDE.mjs");
        std::fs::write(&resource, b"export {}\n").unwrap();
        let metafile = dir.path().join(RESOURCE_METAFILE_FILENAME);
        std::fs::write(
            &metafile,
            serde_json::to_vec(&serde_json::json!({
                "outputs": {
                    resource.file_name().unwrap().to_string_lossy().into_owned(): {
                        "inputs": { "fixture-glue.zfb-resource.mjs": { "bytesInOutput": 1 } }
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let out = read_back_shared_outdir(dir.path(), &metafile, dir.path())
            .expect("metafile output paths are relative to esbuild cwd by default");
        assert_eq!(out.resources.len(), 1);
        assert_eq!(
            out.resources[0].filename,
            resource.file_name().unwrap().to_string_lossy()
        );
    }

    #[test]
    fn read_back_shared_outdir_fails_closed_for_resource_oracle_and_name_violations() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("islands.js"), b"entry\n").unwrap();
        let resource = dir.path().join("islands-resource-payload-ABCDE.wasm");
        std::fs::write(&resource, b"wasm").unwrap();
        let metafile = dir.path().join(RESOURCE_METAFILE_FILENAME);

        write_resource_metafile(
            &metafile,
            &[(resource.clone(), "/fixture/not-a-resource.txt")],
        )
        .unwrap();
        let err = read_back_shared_outdir(dir.path(), &metafile, dir.path())
            .expect_err("wrong metafile source suffix must fail")
            .to_string();
        assert!(err.contains("file-loader edge"), "{err}");

        write_resource_metafile(
            &metafile,
            &[(
                dir.path()
                    .join("other")
                    .join("islands-resource-payload-ABCDE.wasm"),
                "/fixture/payload.wasm",
            )],
        )
        .unwrap();
        let err = read_back_shared_outdir(dir.path(), &metafile, dir.path())
            .expect_err("different output path with the same basename must fail")
            .to_string();
        assert!(err.contains("exact metafile output path"), "{err}");

        std::fs::write(&metafile, b"{not json").unwrap();
        let err = read_back_shared_outdir(dir.path(), &metafile, dir.path())
            .expect_err("malformed metafile must fail")
            .to_string();
        assert!(err.contains("could not parse metafile"), "{err}");

        std::fs::remove_file(&resource).unwrap();
        std::fs::create_dir(&resource).unwrap();
        std::fs::write(&metafile, b"{\"outputs\":{}}").unwrap();
        let err = read_back_shared_outdir(dir.path(), &metafile, dir.path())
            .expect_err("directory resource output must fail")
            .to_string();
        assert!(err.contains("unexpected non-file"), "{err}");
    }

    #[test]
    fn output_names_reject_cross_class_collisions() {
        let mut names = BTreeSet::new();
        register_output_name(&mut names, "islands.js").unwrap();
        let err = register_output_name(&mut names, "islands.js")
            .expect_err("all output classes share one filename namespace")
            .to_string();
        assert!(err.contains("cross-class-colliding"), "{err}");
    }

    /// `read_back_outdir`: entry + chunks are split correctly, sourcemap
    /// siblings are ignored (matching pre-#806 behaviour), and chunks come
    /// back in a deterministic filename-sorted order.
    #[test]
    fn read_back_outdir_collects_chunks_and_ignores_maps() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("islands.js"), b"entry\n").unwrap();
        // Write out of sorted order to prove the sort.
        std::fs::write(dir.path().join("islands-chunk-ZZZ.js"), b"z\n").unwrap();
        std::fs::write(dir.path().join("islands-chunk-AAA.js"), b"a\n").unwrap();
        // Sourcemap siblings must be ignored, not shipped as chunks.
        std::fs::write(dir.path().join("islands.js.map"), b"{}").unwrap();
        std::fs::write(dir.path().join("islands-chunk-AAA.js.map"), b"{}").unwrap();

        let out = read_back_outdir(dir.path()).expect("read back");
        assert_eq!(out.js, "entry\n");
        let names: Vec<&str> = out.chunks.iter().map(|c| c.filename.as_str()).collect();
        assert_eq!(names, vec!["islands-chunk-AAA.js", "islands-chunk-ZZZ.js"]);
        assert_eq!(out.chunks[0].bytes, b"a\n");
        assert_eq!(out.chunks[1].bytes, b"z\n");
        assert!(out.workers.is_empty());
    }

    /// Worker outputs share the staging directory but remain separate from
    /// self-hashed chunks in the public result.
    #[test]
    fn read_back_outdir_collects_workers_separately_and_sorts_them() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("islands.js"), b"entry\n").unwrap();
        std::fs::write(dir.path().join("islands-chunk-AAA.js"), b"chunk\n").unwrap();
        std::fs::write(dir.path().join("worker-src-s-z-d-ts.js"), b"worker-z\n").unwrap();
        std::fs::write(dir.path().join("worker-src-s-a-d-ts.js"), b"worker-a\n").unwrap();
        std::fs::write(dir.path().join("worker-src-s-a-d-ts.js.map"), b"{}").unwrap();

        let out = read_back_outdir(dir.path()).expect("read back");
        assert_eq!(out.chunks.len(), 1);
        assert_eq!(
            out.workers
                .iter()
                .map(|worker| worker.filename.as_str())
                .collect::<Vec<_>>(),
            vec!["worker-src-s-a-d-ts.js", "worker-src-s-z-d-ts.js"]
        );
        assert_eq!(out.workers[0].bytes, b"worker-a\n");
    }

    /// `read_back_outdir`: a pathologically-named output file that is not
    /// the entry and not `islands-chunk-*` is rejected rather than silently
    /// shipped into `BundleOutput.chunks`.
    #[test]
    fn read_back_outdir_rejects_unexpected_output_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("islands.js"), b"entry\n").unwrap();
        std::fs::write(dir.path().join("sneaky.js"), b"nope\n").unwrap();

        let err = read_back_outdir(dir.path()).expect_err("must reject unexpected file");
        assert!(
            format!("{err}").contains("unexpected output file"),
            "got: {err}"
        );

        let map_only = tempfile::tempdir().unwrap();
        std::fs::write(map_only.path().join("islands.js"), b"entry\n").unwrap();
        std::fs::write(map_only.path().join("sneaky.js.map"), b"{}").unwrap();
        let err = read_back_outdir(map_only.path())
            .expect_err("an unknown sourcemap must not bypass filename validation");
        assert!(format!("{err}").contains("unexpected output file"));

        let malformed_map = tempfile::tempdir().unwrap();
        std::fs::write(malformed_map.path().join("islands.js"), b"entry\n").unwrap();
        std::fs::write(malformed_map.path().join("islands-chunk-AAAA.map"), b"{}").unwrap();
        let err = read_back_outdir(malformed_map.path())
            .expect_err("only .js.map sourcemap siblings are recognised");
        assert!(format!("{err}").contains("unexpected sourcemap"));
    }

    /// `read_back_outdir`: a directory with no `islands.js` entry is an
    /// error — the stable-name contract must hold.
    #[test]
    fn read_back_outdir_requires_entry() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("islands-chunk-AAA.js"), b"a\n").unwrap();

        let err = read_back_outdir(dir.path()).expect_err("must require entry");
        assert!(
            format!("{err}").contains("no `islands.js` entry"),
            "got: {err}"
        );
    }

    /// Regression for issue #151 (zudolab/zudo-doc#1355 Wave 8).
    ///
    /// The esbuild subprocess argument list MUST include
    /// `--jsx=automatic` and the owned JSX import source.
    #[test]
    fn bundle_request_uses_automatic_owned_jsx() {
        let request = shared_request(&BundleConfig::default(), true);
        assert_eq!(request.jsx.runtime, Some(JsxRuntime::Automatic));
        assert_eq!(
            request.jsx.import_source.as_deref(),
            Some("@takazudo/zfb/zudo-react")
        );
    }

    #[test]
    fn every_client_pass_has_the_sdk_alias_exactly_once() {
        let cfg = BundleConfig::default();
        let passes = [
            ("shared islands entry", shared_request(&cfg, true)),
            (
                "module worker",
                named_request(&cfg, "/tmp/worker.ts", "worker-src-s-worker-d-ts", None),
            ),
            (
                "client script",
                named_request(&cfg, "/tmp/widget.client.tsx", "widget", None),
            ),
            (
                "client-script module worker",
                named_request(
                    &cfg,
                    "/tmp/widget.worker.ts",
                    "worker-src-s-widget-d-worker-d-ts",
                    None,
                ),
            ),
        ];

        for (pass, request) in passes {
            let aliases = request
                .resolve
                .aliases
                .iter()
                .filter(|(key, target)| key == "zfb" && target == "@takazudo/zfb")
                .count();
            assert_eq!(
                aliases, 1,
                "{pass} should receive exactly one SDK alias: {request:?}"
            );
        }
    }

    /// Regression for issue #287 (zudolab/zzmod#154).
    ///
    /// `--define:import.meta.env.PROD=…` and `--define:import.meta.env.DEV=…`
    /// MUST appear in the islands esbuild arg list so consumer `'use client'`
    /// source code referencing either expression (e.g. `if
    /// (import.meta.env.DEV) console.log(…)`) is folded at bundle time
    /// rather than shipped to the browser where `import.meta.env` is
    /// `undefined` and `import.meta.env.DEV` throws at module init.
    ///
    /// Production mode, regardless of minification, → `PROD=true`,
    /// `DEV=false`, and `NODE_ENV="production"`. The values are JS literals,
    /// not quoted strings (except the string-valued `NODE_ENV`).
    #[test]
    fn bundle_request_defines_import_meta_env_in_prod() {
        let request = shared_request(&BundleConfig::production().with_minify(false), true);
        assert_eq!(define_of(&request, "import.meta.env.PROD"), Some("true"));
        assert_eq!(define_of(&request, "import.meta.env.DEV"), Some("false"));
        assert_eq!(
            define_of(&request, "process.env.NODE_ENV"),
            Some("\"production\"")
        );
    }

    /// Development mode, even when minification is requested, flips both
    /// flags and pins `NODE_ENV="development"`. This keeps mode independent
    /// of output-size optimization across all client pipelines.
    #[test]
    fn bundle_request_defines_import_meta_env_in_dev() {
        let request = shared_request(&BundleConfig::dev().with_minify(true), true);
        assert!(request.minify);
        assert_eq!(define_of(&request, "import.meta.env.PROD"), Some("false"));
        assert_eq!(define_of(&request, "import.meta.env.DEV"), Some("true"));
        assert_eq!(
            define_of(&request, "process.env.NODE_ENV"),
            Some("\"development\"")
        );
    }

    #[test]
    fn client_script_request_uses_the_same_explicit_dev_mode_contract() {
        let request = named_request(
            &BundleConfig::dev().with_minify(true),
            "/tmp/widget.client.ts",
            "widget",
            None,
        );

        assert_eq!(layout_names(&request).0, Some("widget"));
        assert_eq!(
            define_of(&request, "import.meta.env.DEV"),
            Some("true"),
            "client-script DEV must be mode-driven: {request:?}"
        );
        assert_eq!(
            define_of(&request, "import.meta.env.PROD"),
            Some("false"),
            "client-script PROD must be mode-driven: {request:?}"
        );
        assert_eq!(
            define_of(&request, "process.env.NODE_ENV"),
            Some("\"development\""),
            "client-script NODE_ENV must be mode-driven: {request:?}"
        );
    }

    #[test]
    fn bundle_request_carries_inline_loaders_and_sorted_raw_defines() {
        let cfg = BundleConfig::production()
            .with_loaders(std::collections::BTreeMap::from([
                (".txt".to_string(), "text".to_string()),
                (".bin".to_string(), "binary".to_string()),
            ]))
            .with_define(std::collections::BTreeMap::from([
                ("__ZETA__".to_string(), "true".to_string()),
                ("__ALPHA__".to_string(), "\"zfb\"".to_string()),
            ]));
        let request = shared_request(&cfg, true);

        assert_eq!(loader_of(&request, ".module.css"), Some(Loader::Empty));
        assert_eq!(loader_of(&request, ".bin"), Some(Loader::Binary));
        assert_eq!(loader_of(&request, ".txt"), Some(Loader::Text));

        let position = |key: &str| {
            request
                .defines
                .iter()
                .position(|(candidate, _)| candidate == key)
                .unwrap_or_else(|| panic!("missing define {key}: {request:?}"))
        };
        assert_eq!(define_of(&request, "__ALPHA__"), Some("\"zfb\""));
        assert_eq!(define_of(&request, "__ZETA__"), Some("true"));
        assert!(
            position("import.meta.env.DEV") < position("__ALPHA__")
                && position("__ALPHA__") < position("__ZETA__"),
            "raw defines follow the mode defines in key order: {request:?}"
        );
    }

    /// A configured loader name the bundler does not know fails the job by
    /// name, and a configured raw `css` loader reaches the native bundler,
    /// which owns the actionable rejection.
    #[test]
    fn bundle_request_rejects_unknown_loader_and_forwards_raw_css() {
        let error = build_bundle_request(
            &IslandsBundlerConfig::default().with_working_dir("/tmp/zfb-proj"),
            &BundleConfig::default().with_loaders(BTreeMap::from([(
                ".txt".to_string(),
                "local-css".to_string(),
            )])),
            None,
            None,
            JobShape::shared_entry(
                Path::new("/tmp/zfb-out"),
                Path::new("/tmp/entry.tsx"),
                true,
                None,
            ),
        )
        .expect_err("unknown loader must be rejected");
        assert!(
            format!("{error:#}").contains("Unknown module type: local-css"),
            "{error:#}"
        );

        let request = shared_request(
            &BundleConfig::default()
                .with_loaders(BTreeMap::from([(".css".to_string(), "css".to_string())])),
            true,
        );
        assert_eq!(loader_of(&request, ".css"), Some(Loader::Css));
    }

    /// A client script that reaches raw CSS through a configured `css` loader
    /// fails with the native bundler's own diagnostic, naming the file, and
    /// writes nothing.
    #[test]
    fn client_script_raw_css_surfaces_the_native_rejection() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("widget.client.ts");
        std::fs::write(dir.path().join("a.css"), "a{color:red}\n").unwrap();
        std::fs::write(&entry, "import './a.css';\nconsole.log('widget');\n").unwrap();
        let bundler =
            IslandsBundler::new(IslandsBundlerConfig::default().with_working_dir(dir.path()));
        let config = BundleConfig::production()
            .with_loaders(BTreeMap::from([(".css".to_string(), "css".to_string())]));
        let error = bundler
            .bundle_client_script_file("widget", &entry, &config)
            .expect_err("raw CSS must be rejected");
        let message = format!("{error:#}");
        assert!(message.contains("a.css"), "{message}");
        assert!(message.to_lowercase().contains("raw css"), "{message}");
        assert!(!dir.path().join("widget.js").exists());
    }

    /// Operator resolution settings and plugin aliases reach every request in
    /// order: plugin virtual modules, then single-file plugin aliases, then the
    /// built-in SDK alias. An explicitly empty module path means none, even
    /// when the process inherited one.
    #[test]
    fn bundle_request_carries_operator_resolution_and_plugin_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let bundler = IslandsBundlerConfig {
            externals: vec![
                External::Package("vendor-lib".to_string()),
                External::Prefix("@takazudo/".to_string()),
            ],
            conditions: Some(vec!["custom".to_string()]),
            ..IslandsBundlerConfig::default().with_working_dir(dir.path())
        };
        let resolver = zfb_plugin_resolver::build_resolver_inputs(
            &[],
            &[("virtual:data".to_string(), "export default 1;".to_string())],
            dir.path(),
            &BTreeMap::new(),
            &["zfb"],
        )
        .unwrap();
        let tsconfig = dir.path().join("synthetic.json");
        let job = |bundler: &IslandsBundlerConfig| {
            build_bundle_request(
                bundler,
                &BundleConfig::default(),
                Some(&resolver),
                Some(&tsconfig),
                JobShape::named(
                    Path::new("/tmp/zfb-out"),
                    Path::new("/tmp/widget.client.ts"),
                    "widget",
                    None,
                ),
            )
            .unwrap()
        };

        let path = std::env::join_paths(["/staged/a", "/staged/b"]).unwrap();
        let request = job(&bundler.clone().with_node_path(path));
        assert_eq!(request.cwd, dir.path());
        assert_eq!(
            request.externals,
            vec![
                External::Prefix("node:".to_string()),
                External::Package("vendor-lib".to_string()),
                External::Prefix("@takazudo/".to_string()),
            ]
        );
        assert_eq!(request.resolve.conditions, Some(vec!["custom".to_string()]));
        assert_eq!(
            request.resolve.extra_module_dirs,
            vec![PathBuf::from("/staged/a"), PathBuf::from("/staged/b")]
        );
        assert_eq!(request.tsconfig, Some(tsconfig.clone()));
        let virtual_target = resolver.virtual_module_alias_args[0].1.clone();
        assert_eq!(
            request.resolve.aliases,
            vec![
                ("virtual:data".to_string(), virtual_target),
                ("zfb".to_string(), "@takazudo/zfb".to_string()),
            ]
        );

        let request = job(&bundler.with_node_path(""));
        assert!(request.resolve.extra_module_dirs.is_empty());
    }

    #[test]
    fn owned_request_uses_runtime_without_compat_aliases() {
        let request = shared_request(&BundleConfig::default(), true);
        assert_eq!(request.jsx.runtime, Some(JsxRuntime::Automatic));
        assert_eq!(
            request.jsx.import_source.as_deref(),
            Some("@takazudo/zfb/zudo-react")
        );
        assert!(request.keep_names);
        assert!(!request.resolve.aliases.iter().any(
            |(key, _)| key.starts_with("react") || key.starts_with("@takazudo/zfb/jsx-factory")
        ));
    }

    #[test]
    fn owned_glue_is_deterministic_and_uses_runtime_client() {
        let islands = [Island::with_marker_name(
            "Counter",
            "/fixture/counter.tsx",
            "Counter",
        )];
        let first = render_shared_bundle_entry_source_with_build(
            &islands,
            false,
            Some("build-1"),
            &BTreeMap::new(),
        );
        let second = render_shared_bundle_entry_source_with_build(
            &islands,
            false,
            Some("build-1"),
            &BTreeMap::new(),
        );
        assert_eq!(first, second);
        assert_eq!(first.matches("@takazudo/zfb/zudo-react/client").count(), 1);
        assert!(first.contains("@takazudo/zfb/zudo-react/client"));
        assert!(first.contains("build-1"));
        assert!(first.contains("hydrate(h(C, props)"));
        assert!(first.contains("mount(h(C, props)"));
        assert!(!first.contains("from \"preact\""));
        assert!(!first.contains("from \"react\""));
    }

    #[test]
    fn shared_bundle_registration_uses_configured_module_label() {
        let source_path = PathBuf::from("/shadow/components/Counter.tsx");
        let islands = [Island::new("Counter", source_path.clone())];
        let labels = BTreeMap::from([(source_path, "components/Counter.tsx".to_string())]);

        let src = render_shared_bundle_entry_source_with_build(
            &islands,
            false,
            Some(TEST_BUILD_TOKEN),
            &labels,
        );
        let register_line = src
            .lines()
            .find(|line| line.starts_with("__zfb_register("))
            .expect("registration line");

        assert_eq!(
            register_line,
            "__zfb_register(__zfb_island_0, \"Counter\", \"Counter\", \"components/Counter.tsx\");"
        );
        assert!(
            src.contains(r#"import * as __zfb_island_0 from "/shadow/components/Counter.tsx";"#)
        );
    }

    #[test]
    fn shared_bundle_registration_falls_back_to_file_name_without_labels() {
        let islands = [Island::new("Counter", "/abs/components/Counter.tsx")];
        let src = render_shared_bundle_entry_source_with_build(
            &islands,
            false,
            Some(TEST_BUILD_TOKEN),
            &BTreeMap::new(),
        );
        let register_line = src
            .lines()
            .find(|line| line.starts_with("__zfb_register("))
            .expect("registration line");

        assert_eq!(
            register_line,
            "__zfb_register(__zfb_island_0, \"Counter\", \"Counter\", \"Counter.tsx\");"
        );
        assert!(!register_line.contains("/abs/"));
    }

    #[test]
    fn shared_bundle_registration_rejects_absolute_configured_labels() {
        let source_path = PathBuf::from("/abs/components/Counter.tsx");
        let islands = [Island::new("Counter", source_path.clone())];
        let labels = BTreeMap::from([(
            source_path,
            "/Users/builder/components/Counter.tsx".to_string(),
        )]);
        let src = render_shared_bundle_entry_source_with_build(
            &islands,
            false,
            Some(TEST_BUILD_TOKEN),
            &labels,
        );
        let register_line = src
            .lines()
            .find(|line| line.starts_with("__zfb_register("))
            .expect("registration line");

        assert_eq!(
            register_line,
            "__zfb_register(__zfb_island_0, \"Counter\", \"Counter\", \"Counter.tsx\");"
        );
    }

    // -----------------------------------------------------------------------
    // #261 — Islands esbuild resolver: alias + virtual-module tests
    // -----------------------------------------------------------------------

    /// Zero alias/virtual-module registrations → config fields are empty by
    /// default and no plugin-specific aliases appear. The built-in `zfb`
    /// SDK alias is still present in every real client pass.
    #[test]
    fn zero_registrations_produce_no_plugin_aliases_in_config() {
        let cfg = IslandsBundlerConfig::default();
        assert!(
            cfg.alias_entries.is_empty(),
            "alias_entries must default to empty"
        );
        assert!(
            cfg.virtual_modules.is_empty(),
            "virtual_modules must default to empty"
        );
    }

    /// `with_alias_entries` stores the pairs and overwrites the previous list.
    #[test]
    fn with_alias_entries_stores_pairs() {
        let aliases = vec![
            ("@/foo".to_string(), "/abs/project/src/foo.tsx".to_string()),
            ("@/bar".to_string(), "/abs/project/src/bar.tsx".to_string()),
        ];
        let cfg = IslandsBundlerConfig::default().with_alias_entries(aliases.clone());
        assert_eq!(cfg.alias_entries, aliases);
    }

    /// `with_virtual_modules` stores the pairs and overwrites the previous list.
    #[test]
    fn with_virtual_modules_stores_pairs() {
        let vms = vec![(
            "virtual:my-data".to_string(),
            "export const x = 1;".to_string(),
        )];
        let cfg = IslandsBundlerConfig::default().with_virtual_modules(vms.clone());
        assert_eq!(cfg.virtual_modules, vms);
    }

    /// Bundling in mock mode with alias entries included — the mock path must
    /// still succeed (alias args are a real-subprocess-only concern; mock mode
    /// returns the configured output unchanged).
    #[test]
    fn mock_mode_succeeds_with_alias_entries() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = IslandsBundlerConfig {
            mock_bundle: true,
            mock_output: "// alias mock output".to_string(),
            alias_entries: vec![("@/foo".to_string(), "/abs/src/foo.tsx".to_string())],
            ..IslandsBundlerConfig::default()
        };
        let bundler = IslandsBundler::new(cfg);
        let islands = vec![Island::new("Counter", "/abs/components/Counter.tsx")];
        let bundle_cfg = BundleConfig::default()
            .with_zudo_react_build(Some(TEST_BUILD_TOKEN.to_string()))
            .with_outdir(dir.path().to_path_buf());
        let out = bundler.bundle(&islands, &bundle_cfg).unwrap();
        // Mock output is returned in-memory — no disk write.
        assert_eq!(out.bytes, b"// alias mock output");
    }

    /// Bundling in mock mode with virtual modules — the mock path must succeed.
    #[test]
    fn mock_mode_succeeds_with_virtual_modules() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = IslandsBundlerConfig {
            mock_bundle: true,
            mock_output: "// virtual mock output".to_string(),
            virtual_modules: vec![(
                "virtual:config".to_string(),
                "export const site = 'zfb';".to_string(),
            )],
            ..IslandsBundlerConfig::default()
        };
        let bundler = IslandsBundler::new(cfg);
        let islands = vec![Island::new("Counter", "/abs/components/Counter.tsx")];
        let bundle_cfg = BundleConfig::default()
            .with_zudo_react_build(Some(TEST_BUILD_TOKEN.to_string()))
            .with_outdir(dir.path().to_path_buf());
        let out = bundler.bundle(&islands, &bundle_cfg).unwrap();
        // Mock output is returned in-memory — no disk write.
        assert_eq!(out.bytes, b"// virtual mock output");
    }

    /// Verify that the `with_alias_entries` + `with_virtual_modules` builders
    /// can be chained together and that both field lists are populated.
    #[test]
    fn builder_methods_are_chainable() {
        let cfg = IslandsBundlerConfig::default()
            .with_alias_entries(vec![(
                "@/components".to_string(),
                "/abs/src/components".to_string(),
            )])
            .with_virtual_modules(vec![(
                "virtual:meta".to_string(),
                "export const v = 1;".to_string(),
            )]);
        assert_eq!(cfg.alias_entries.len(), 1);
        assert_eq!(cfg.virtual_modules.len(), 1);
        assert_eq!(cfg.alias_entries[0].0, "@/components");
        assert_eq!(cfg.virtual_modules[0].0, "virtual:meta");
    }

    /// Parity check: the source text stored in `virtual_modules` is the same
    /// string that the plugin host loader produces. When both the V8 host
    /// resolver (#260) and the islands esbuild resolver (#261) receive the
    /// same `(specifier, source)` pair from the orchestrator (which fetches
    /// via `PluginHost::invoke_virtual_loader`), they can't drift because
    /// they both read from the same in-memory string.
    ///
    /// This test asserts the round-trip: store source → retrieve from config
    /// → same bytes. No subprocess involvement needed.
    #[test]
    fn virtual_module_source_round_trips_through_config() {
        let source = "export const data = { key: 'value' };\nexport default data;\n";
        let cfg = IslandsBundlerConfig::default()
            .with_virtual_modules(vec![("virtual:data".to_string(), source.to_string())]);
        let retrieved = &cfg.virtual_modules[0].1;
        assert_eq!(
            retrieved.as_str(),
            source,
            "source must survive config round-trip unchanged"
        );
    }
}
