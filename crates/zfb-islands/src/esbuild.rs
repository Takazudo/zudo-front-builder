//! Production [`ClientBundler`] implementation: shell out to the esbuild
//! CLI binary.
//!
//! Sub 2 of Epic #6 lives here. The trait + shared types
//! ([`crate::bundler::ClientBundler`], [`crate::bundler::Island`],
//! [`crate::bundler::BundleConfig`], [`crate::bundler::BundleOutput`]) are
//! owned by Sub 1 in [`crate::bundler`]; this module is the production
//! implementation that wraps the esbuild subprocess.
//!
//! Mirror of `zfb_css::engine::TailwindSubprocessEngine`: a config struct
//! that locates the binary (defaulting to `crates/zfb/binaries/esbuild`),
//! an implementation that builds the command line, runs esbuild, reads the
//! output back into memory, and returns it in [`crate::bundler::BundleOutput`]
//! — the bundler does NOT write `islands.js` to disk. Downstream consumers
//! (the prod pipeline, the dev server) own all disk writes.
//! `ProductionAssetPipeline` is the single source of truth for content
//! hashing per the Prod Asset Graph epic.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};
use zfb_types::json_string;

use crate::bundler::{
    bundle_link_href, island_link_href, BundleChunk, BundleConfig, BundleOutput, ClientBundler,
    FrameworkKind, Island, IslandBundle, ModuleId, PerIslandBundleOutput,
};

/// The pinned esbuild CLI version this crate runs against.
///
/// Re-exported from `zfb_toolchain_pins::EXPECTED_ESBUILD_VERSION`, the
/// single source of truth for all external tool version pins. To bump,
/// update that constant and follow the "External tool version pins"
/// procedure in `CONTRIBUTING.md` at the workspace root.
pub use zfb_toolchain_pins::EXPECTED_ESBUILD_VERSION;

/// esbuild `--entry-names` template for the shared islands bundle.
///
/// `islands` (no extension — esbuild appends `.js`) pins the entry's
/// basename to the stable `STABLE_ISLANDS_FILENAME` so the staging
/// read-back can recognise the entry file among the esbuild outdir
/// contents. A `debug_assert!` in `bundle_one_entry` ties this template
/// to `STABLE_ISLANDS_FILENAME`'s stem so the two can never silently
/// drift.
pub(crate) const ESBUILD_ENTRY_NAME_TEMPLATE: &str = "islands";

/// esbuild `--chunk-names` template for code-split chunks.
///
/// `islands-chunk-[hash]` puts esbuild's own content hash in each chunk's
/// **flat** basename, in the SAME directory as the entry. This is the core
/// of the bundler split contract: esbuild — not the prod pipeline — owns
/// the chunk hash, and bakes relative `import("./islands-chunk-<hash>.js")`
/// references between chunks/entry. Downstream therefore MUST ship chunks
/// under these exact names (never rename them); only the entry may later be
/// content-hashed by the prod pipeline, because its relative chunk imports
/// still resolve from the shared directory.
///
/// A future bundler swap (the Rolldown spike — issue #318) MUST reproduce
/// this contract: stable-named entry, self-hashed flat chunks, relative
/// inter-chunk imports. See [`crate::BundleOutput::chunks`] and
/// `ESBUILD_CHUNK_FILENAME_PREFIX` (the read-back validation gate).
pub(crate) const ESBUILD_CHUNK_NAME_TEMPLATE: &str = "islands-chunk-[hash]";

/// Basename prefix every emitted chunk file must carry
/// (`islands-chunk-...`), derived from [`ESBUILD_CHUNK_NAME_TEMPLATE`].
/// `bundle_one_entry`'s read-back rejects any non-entry output file that
/// is not flat (no path separators / traversal) and does not start with
/// this prefix — a defence against a future esbuild change or a malformed
/// template silently smuggling an unexpected file into `BundleOutput`.
pub(crate) const ESBUILD_CHUNK_FILENAME_PREFIX: &str = "islands-chunk-";

/// SHA-256 of the pinned esbuild binary for the current platform,
/// lowercase hex.
///
/// The constant is resolved at compile time using `cfg!()` so the
/// correct hash is embedded for every supported platform without
/// runtime branching.  Platforms outside the supported set compile to
/// the empty string `""`, which skips the checksum gate with a warning
/// (the `--version` gate still runs).
///
/// These digests are the SHA-256 of the *extracted* binary inside the
/// platform-specific npm package (e.g. `@esbuild/linux-x64`), **not**
/// of the .tgz itself.  They must be bumped in lock-step with
/// `EXPECTED_ESBUILD_VERSION` and with the matching constants in
/// `crates/zfb/build.rs` whenever the esbuild pin is updated.
///
/// Verified on 2026-05-05 against npm registry v0.25.12.
pub const EXPECTED_ESBUILD_SHA256: &str = {
    // Linux x86_64
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "bab29b2ca7a9e89b67cf720b77b2d743f9f31f5cf0d5bd74ee8c8de30ced7014"
    }

    // Linux aarch64
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "840ad255d6fd587b126d8b2d59ab506d8562785b9bc76249dc3b0e1bdd2ca449"
    }

    // macOS aarch64 (Apple Silicon)
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "3e030ee2aa86ad3c33e5e95ae0e53bb03de40e0da35c9b1180a67de4a497cae5"
    }

    // macOS x86_64 (Intel)
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "bd09e65a6a1a903c40269d3a4ae23ffc6139f691703728c1faf25f62e48baa40"
    }

    // Windows x86_64
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "cae1bbc86f4df800b01d99e28aea0a154b02243de6797e98f48a9b88a64a7be0"
    }

    // Fallback for unsupported platforms: skip checksum gate.
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
    )))]
    {
        ""
    }
};

/// One-time cache of `(binary_path → outcome)` for the version + checksum
/// verification. The first invocation of an `EsbuildSubprocessBundler`
/// pays the cost of running `esbuild --version` and hashing the binary;
/// subsequent invocations short-circuit. Wrapped in a `Mutex<BTreeMap>`
/// so a process running multiple bundlers (e.g. an integration test
/// suite) verifies each pinned binary path exactly once.
fn verification_cache() -> &'static Mutex<BTreeMap<PathBuf, ()>> {
    static CACHE: OnceLock<Mutex<BTreeMap<PathBuf, ()>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Run the version + checksum gate on `binary_path`. Idempotent across
/// the process: each path is verified at most once, but every distinct
/// path is verified the first time it is seen.
///
/// `skip` lets callers (notably tests using `mock_subprocess`) bypass
/// the gate without having to expose the cache directly.
fn ensure_binary_verified(binary_path: &Path, skip: bool) -> Result<()> {
    if skip {
        return Ok(());
    }
    let key = binary_path.to_path_buf();
    {
        let cache = verification_cache().lock().unwrap_or_else(|p| {
            tracing::warn!(
                site = "esbuild::verification_cache",
                "mutex poisoned, recovered"
            );
            p.into_inner()
        });
        if cache.contains_key(&key) {
            return Ok(());
        }
    }

    if !binary_path.exists() {
        return Err(anyhow!(
            "esbuild binary not found at {}; \
             set ZFB_ESBUILD_BIN or update EsbuildSubprocessConfig::binary_path",
            binary_path.display()
        ));
    }

    // 1. Version gate.
    let version_output = Command::new(binary_path)
        .arg("--version")
        .output()
        .with_context(|| {
            format!(
                "failed to spawn `{} --version` for the version gate",
                binary_path.display()
            )
        })?;
    if !version_output.status.success() {
        let stderr = String::from_utf8_lossy(&version_output.stderr);
        return Err(anyhow!(
            "esbuild --version exited with status {}: {}",
            version_output.status,
            stderr.trim()
        ));
    }
    let reported = String::from_utf8_lossy(&version_output.stdout)
        .trim()
        .to_string();
    if reported != EXPECTED_ESBUILD_VERSION {
        return Err(anyhow!(
            "esbuild version mismatch: expected `{}` (pinned in zfb-toolchain-pins), got `{}` from {}. \
             To resolve, follow the \"External tool version pins\" procedure in CONTRIBUTING.md \
             at the workspace root: bump EXPECTED_ESBUILD_VERSION in \
             crates/zfb-toolchain-pins/src/lib.rs (and EXPECTED_ESBUILD_SHA256 in \
             crates/zfb-islands/src/esbuild.rs) in lock-step with the binary under \
             crates/zfb/binaries/esbuild/esbuild.",
            EXPECTED_ESBUILD_VERSION,
            reported,
            binary_path.display()
        ));
    }

    // 2. SHA-256 gate. Skipped when the constant is empty (development
    // / pre-release-engineering slot).
    if !EXPECTED_ESBUILD_SHA256.is_empty() {
        let bytes = std::fs::read(binary_path)
            .with_context(|| format!("failed to read {} for checksum", binary_path.display()))?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let actual = hex::encode(hasher.finalize());
        if !actual.eq_ignore_ascii_case(EXPECTED_ESBUILD_SHA256) {
            return Err(anyhow!(
                "esbuild binary checksum mismatch for {}: \
                 expected sha256 `{}`, got `{}`. \
                 To resolve, follow the \"External tool version pins\" procedure in \
                 CONTRIBUTING.md at the workspace root: either replace the binary with \
                 one matching EXPECTED_ESBUILD_SHA256, or — when bumping intentionally — \
                 update EXPECTED_ESBUILD_VERSION in crates/zfb-toolchain-pins/src/lib.rs \
                 and EXPECTED_ESBUILD_SHA256 in crates/zfb-islands/src/esbuild.rs \
                 in lock-step with the new binary.",
                binary_path.display(),
                EXPECTED_ESBUILD_SHA256,
                actual
            ));
        }
    }

    verification_cache()
        .lock()
        .unwrap_or_else(|p| {
            tracing::warn!(
                site = "esbuild::verification_cache",
                "mutex poisoned, recovered"
            );
            p.into_inner()
        })
        .insert(key, ());
    Ok(())
}

#[derive(Debug, Clone)]
pub struct EsbuildSubprocessConfig {
    /// Path to the esbuild CLI binary.
    ///
    /// Default: `crates/zfb/binaries/esbuild/esbuild` (relative to the
    /// workspace root). Sub 2 reserves the parent `esbuild/` directory in
    /// the release tarball layout (mirroring the Tailwind v4 slot pattern;
    /// the directory shape gives release tooling room to drop sidecars
    /// like a checksum manifest next to the binary). Override via
    /// [`Self::with_binary_path`] or via the `ZFB_ESBUILD_BIN` environment
    /// variable (checked at engine construction time, not at every
    /// invocation).
    pub binary_path: PathBuf,

    /// Working directory for the subprocess. esbuild resolves entry points
    /// and `node_modules` lookups relative to this directory.
    ///
    /// Default: the current working directory at engine construction time.
    pub working_dir: PathBuf,

    /// Extra CLI args appended to the subprocess invocation, for escape
    /// hatches like `--define:process.env.NODE_ENV='production'`. Passed
    /// verbatim — each element becomes a separate `argv` entry, so shell
    /// quoting is not needed and shell injection is not possible.
    ///
    /// **Trust boundary**: callers are expected to assemble these from
    /// build configuration (zfb.config.ts, env vars under operator
    /// control), NOT from end-user input. The same caveat applies to
    /// island `source_path`s — they come from project-local source files
    /// the scanner has identified, not from user-supplied data.
    pub extra_args: Vec<OsString>,

    /// When true, the bundler will return a *fake* JS payload instead of
    /// invoking the subprocess. Used by unit tests to avoid depending on
    /// the binary being installed. The string returned is taken from
    /// [`Self::mock_output`].
    pub mock_subprocess: bool,

    /// Output to return when `mock_subprocess` is true.
    pub mock_output: String,

    /// Extra environment variables to set on the esbuild subprocess.
    ///
    /// Primarily used to pass `NODE_PATH=<path>` so esbuild resolves bare
    /// imports against an externally-staged `node_modules` tree (e.g. the
    /// `include_dir!`-extracted `@takazudo/*` + framework runtime tree
    /// from the cargo-installed binary). esbuild walks the importing file
    /// upward looking for `node_modules` and falls back to `NODE_PATH`
    /// when the walk doesn't find a match — see issue zfb#221's
    /// follow-up commit.
    ///
    /// Each entry becomes a `cmd.env(key, value)` call before spawn.
    /// Pre-existing process env is inherited; setting an entry here
    /// overrides any value the parent process had for the same key (for
    /// the subprocess only — the parent env is untouched).
    pub env_vars: Vec<(OsString, OsString)>,

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
    /// literal exact-match shape in TS / esbuild's path-mapping
    /// pipeline) and passes `--tsconfig=<that file>` to esbuild. No
    /// `--alias` flags are emitted for plugin entries — esbuild's
    /// `--alias` is prefix-with-slash and would silently rewrite
    /// `@/foo/bar` to `<to>/bar`, contradicting the V8 host's
    /// exact-match contract
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
    /// for the duration of the subprocess call and dropped afterwards.
    ///
    /// An empty `Vec` (the default) adds no temp files and no
    /// synthetic tsconfig entry — zero-registration builds are
    /// byte-identical to today.
    pub virtual_modules: Vec<(String, String)>,
}

impl Default for EsbuildSubprocessConfig {
    fn default() -> Self {
        Self::default_with_env_getter(|name| std::env::var_os(name))
    }
}

impl EsbuildSubprocessConfig {
    /// Build a default config, but resolve the `ZFB_ESBUILD_BIN`
    /// override via the supplied getter rather than touching the real
    /// process environment. Tests use this to drive the env-override
    /// path without calling `std::env::set_var`, which is `unsafe`
    /// under Rust 2024 because it races other threads reading the env
    /// table. Production callers should keep using
    /// [`Default::default`].
    pub fn default_with_env_getter<F>(getter: F) -> Self
    where
        F: Fn(&str) -> Option<OsString>,
    {
        let env_override = getter("ZFB_ESBUILD_BIN");
        let binary_path = match env_override {
            Some(p) => PathBuf::from(p),
            None => PathBuf::from("crates/zfb/binaries/esbuild/esbuild"),
        };
        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            binary_path,
            working_dir,
            extra_args: Vec::new(),
            mock_subprocess: false,
            mock_output: String::new(),
            env_vars: Vec::new(),
            alias_entries: Vec::new(),
            virtual_modules: Vec::new(),
        }
    }
}

impl EsbuildSubprocessConfig {
    /// Override the binary path (chainable).
    pub fn with_binary_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.binary_path = path.into();
        self
    }

    /// Override the working directory (chainable).
    pub fn with_working_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.working_dir = dir.into();
        self
    }

    /// Configure the bundler to skip the subprocess and return `output`
    /// instead. Used by unit tests.
    pub fn with_mock_output(mut self, output: impl Into<String>) -> Self {
        self.mock_subprocess = true;
        self.mock_output = output.into();
        self
    }

    /// Append an extra environment variable applied to the esbuild
    /// subprocess (chainable). Most useful for `NODE_PATH=<path>` so
    /// esbuild resolves bare imports against an externally-staged
    /// `node_modules` tree.
    pub fn with_extra_env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env_vars.push((key.into(), value.into()));
        self
    }

    /// Set the alias entries for the islands bundler (chainable).
    ///
    /// Replaces the current `alias_entries` list. Each `(from, to)`
    /// pair becomes an exact-match `compilerOptions.paths` entry in the
    /// synthetic tsconfig the bundler hands to esbuild (#269); `to`
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
}

/// The default [`ClientBundler`]: shells out to the esbuild CLI binary.
///
/// The binary is invoked on one synthesized entry source, plus `--bundle
/// --format=esm --splitting=true --tree-shaking=true` (esbuild tree-shakes
/// ESM by default but we set the flag explicitly so the contract is
/// visible). `--minify` and `--sourcemap=linked` are appended per
/// [`BundleConfig`]. Output goes to a staging `--outdir` (splitting requires
/// directory mode): the entry is emitted under the stable name `islands.js`
/// (`--entry-names=islands`) and any code-split chunks under self-hashed
/// flat names (`--chunk-names=islands-chunk-[hash]`). All emitted files are
/// read back into memory and returned via [`BundleOutput`] — the bundler
/// does **NOT** write to `{outdir}/assets/islands.js`. Downstream consumers
/// (prod pipeline, dev server) own all disk writes; chunks are returned via
/// [`BundleOutput::chunks`] (see that field's contract).
/// `ProductionAssetPipeline` performs the content-hash + rename pass on the
/// *entry* at deploy time; chunks already carry esbuild's own content hash
/// and must never be renamed.
///
/// ### Example
///
/// ```no_run
/// use zfb_islands::{
///     BundleConfig, ClientBundler, EsbuildSubprocessBundler,
///     EsbuildSubprocessConfig, Island,
/// };
/// use std::path::PathBuf;
///
/// let bundler = EsbuildSubprocessBundler::new(EsbuildSubprocessConfig::default());
/// let islands = vec![Island::new("Counter", PathBuf::from("components/counter.tsx"))];
/// let out = bundler.bundle(&islands, &BundleConfig::production()).unwrap();
/// assert_eq!(out.asset_url, "/assets/islands.js");
/// ```
#[derive(Debug, Clone)]
pub struct EsbuildSubprocessBundler {
    config: EsbuildSubprocessConfig,
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
/// When entries exist, `paths_map` is **seeded from the user's `compilerOptions.paths`**
/// (`zfb_plugin_resolver::read_tsconfig_paths_into_map`, which returns absolutised targets
/// with wildcards preserved) **before** the plugin entries are merged on top. This is the
/// #1238 fix: an explicit `--tsconfig=` makes esbuild stop walking up to the real tsconfig,
/// so without seeding the user's own aliases (e.g. `@/*`) would silently vanish the moment a
/// plugin registered anything. `merge_into_tsconfig_paths` applies a "user wins" policy on a
/// key collision, so plugin entries are additive on top of the user's hand-written contract.
///
/// `label` distinguishes the two call sites in temp-file names / error context
/// (`"islands"` vs `"client-script"`); `comment` is the `//` provenance string embedded in
/// the emitted JSON.
fn build_plugin_tsconfig(
    working_dir: &Path,
    resolver_inputs: &zfb_plugin_resolver::ResolverInputs,
    label: &str,
    comment: &str,
) -> Result<Option<tempfile::NamedTempFile>> {
    // INVARIANT: early-return BEFORE reading the user tsconfig or writing any synthetic
    // tsconfig, so the no-plugin path reads nothing and emits no `--tsconfig` (byte-identical).
    if resolver_inputs.paths_entries.is_empty() {
        return Ok(None);
    }

    let mut paths_map = zfb_plugin_resolver::read_tsconfig_paths_into_map(working_dir);
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
    Ok(Some(tmp))
}

impl EsbuildSubprocessBundler {
    /// Construct a new bundler with the given config.
    pub fn new(config: EsbuildSubprocessConfig) -> Self {
        Self { config }
    }

    /// Construct a new bundler with the default config.
    pub fn with_default_config() -> Self {
        Self::new(EsbuildSubprocessConfig::default())
    }

    /// Borrow the underlying config.
    pub fn config(&self) -> &EsbuildSubprocessConfig {
        &self.config
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
        // Derive the mount-glue framework from `config.jsx_import_source`
        // — the single field the orchestrator already sets via
        // `with_jsx_import_source(config.framework…)`. This guarantees the
        // emitted hydration glue (Preact `h()` vs React `createRoot`)
        // and the esbuild `--jsx-import-source` flag can never disagree.
        let framework = FrameworkKind::from_jsx_import_source(&config.jsx_import_source);
        let entry_source =
            render_shared_bundle_entry_source(framework, islands, config.client_router);
        // Shared-bundle path: splitting ON. This is the ONLY path that ships
        // and serves chunks (#806/#808/#809) — `OneEntryOutput::chunks` is
        // threaded into `BundleOutput` here.
        self.bundle_one_entry(&entry_source, config, true)
    }

    /// Per-island bundle pass.
    ///
    /// Generates one tiny entry script per island that imports the
    /// island's component module **and** the framework-specific
    /// hydration glue, then runs `esbuild --bundle --format=esm` on it
    /// (one subprocess per island — code-split by isolation rather than
    /// by `--splitting`, so dynamic-imports from the runtime stay 1:1
    /// with components). Each per-island bundle is written to the
    /// stable path `{outdir}/islands/{ComponentName}.js`. Sourcemaps
    /// are preserved end-to-end when [`BundleConfig::sourcemap`] is
    /// true: esbuild emits a sibling `<file>.map` alongside the asset
    /// and a `//# sourceMappingURL=` comment in the JS so DevTools
    /// picks it up automatically.
    ///
    /// In addition to the per-island bundles, a small **runtime**
    /// bundle is emitted under
    /// `{outdir}/islands/islands-runtime.js`. The runtime is
    /// produced from a generated entry that imports
    /// `scheduleHydrate` + `mountIslands` from the
    /// `@takazudo/zfb-runtime` style runtime (`packages/zfb/src/runtime.ts`)
    /// and ships a manifest of `marker_name → island-bundle-URL` so
    /// the runtime can dynamic-import the right per-island bundle for
    /// each `[data-zfb-island]` element on the page.
    ///
    /// # Hashing
    ///
    /// All filenames written by this method are **stable** — no
    /// `-<hash>` suffix. The Prod Asset Graph epic centralises content
    /// hashing in `ProductionAssetPipeline`, which is the only place
    /// allowed to produce hashed URLs/filenames so HTML rewrites do
    /// not double-hash. For dev-mode change detection, the per-island
    /// hash is still computed and exposed through
    /// [`crate::IslandBundle::hash`].
    ///
    /// # Determinism
    ///
    /// The per-island and runtime bundles are deterministic for a
    /// given `(islands, framework, config)` triple: the island order is
    /// preserved verbatim from the input slice, the runtime manifest is
    /// keyed by `marker_name` (the `data-zfb-island` attribute value the
    /// runtime looks up at hydration time), and the on-disk filenames are
    /// stable sequential slugs `island-{i}.js` (0-based) that are immune
    /// to `marker_name` non-uniqueness across source files.
    pub fn bundle_per_island(
        &self,
        islands: &[Island],
        framework: FrameworkKind,
        config: &BundleConfig,
    ) -> Result<PerIslandBundleOutput> {
        // Outdir layout: {outdir}/islands/.
        let islands_dir = config.outdir.join("islands");
        std::fs::create_dir_all(&islands_dir)
            .with_context(|| format!("failed to create {}", islands_dir.display()))?;

        // Per-island path always knows the framework explicitly (it is
        // a separate parameter), so override `BundleConfig::jsx_import_source`
        // with the framework's accessor before handing it down. This
        // keeps `bundle_one_entry` reading the JSX import source from a
        // single source of truth (the config) regardless of which
        // entrypoint started the bundle.
        let mut framework_cfg = config.clone();
        framework_cfg.jsx_import_source = framework.jsx_import_source().to_string();
        let config = &framework_cfg;

        let mut bundle_entries: Vec<IslandBundle> = Vec::with_capacity(islands.len());
        let mut manifest: Vec<(String, String)> = Vec::with_capacity(islands.len());

        for (i, island) in islands.iter().enumerate() {
            // 1. Generate the per-island entry source.
            let entry_source = render_island_entry_source(framework, island);

            // 2. Bundle it. In mock mode the entry source itself doubles
            //    as the bundled output so unit tests can assert what we
            //    *would* have shipped without spawning esbuild.
            //
            //    Per-island chunk shipping/serving is OUT OF SCOPE (#802/#806):
            //    this path is not production-wired. We bundle with `splitting =
            //    false` so esbuild INLINES any dynamic `import()` into the
            //    single entry — no chunk files are emitted, so taking only the
            //    entry `.js` is lossless and the on-disk shape (`island-{i}.js`)
            //    stays self-contained. (Were splitting left on, an island doing
            //    `import("./heavy")` would emit a dangling
            //    `import("./islands-chunk-*.js")` to a chunk this path never
            //    writes.)
            let bundled_js = self.bundle_one_entry(&entry_source, config, false)?.js;

            // Stable sequential filename: `<outdir>/islands/island-{i}.js`.
            // Using an index (not marker_name) guarantees collision-freedom:
            // marker_name is non-unique across files (two files can export
            // identically-named functions, or both default-export unnamed
            // components — both get the same marker_name). The sequential
            // scheme mirrors the shared-bundle's `__zfb_island_{i}` naming.
            // `ProductionAssetPipeline` is the only place allowed to emit
            // content-addressed names; this path keeps a stable shape so
            // S0's "no hashed URLs in zfb-islands" rule applies to both the
            // shared-bundle and per-island emitters. The `IslandBundle::hash`
            // field is still populated so dev-mode change detection has
            // something to compare against without touching the on-disk name.
            let hash = hash_8(&bundled_js);
            let filename = format!("island-{i}.js");
            // Sequential index filenames can never contain a path separator,
            // but assert the invariant so future scheme changes don't silently
            // introduce a security hole (path traversal via separator injection).
            debug_assert!(
                !filename.contains('/') && !filename.contains(std::path::MAIN_SEPARATOR),
                "island filename must not contain a path separator: {filename:?}"
            );
            let asset_path = islands_dir.join(&filename);
            std::fs::write(&asset_path, bundled_js.as_bytes())
                .with_context(|| format!("failed to write {}", asset_path.display()))?;

            let asset_url = island_link_href(&config.base_url, &asset_path);
            // Manifest is keyed by marker_name (the data-zfb-island attribute
            // value). The runtime's mountIslands does manifest[componentName]
            // where componentName is derived from the SSR marker — which is
            // always marker_name, NOT component_name (the export-side name).
            manifest.push((island.marker_name.clone(), asset_url.clone()));

            bundle_entries.push(IslandBundle {
                component_name: island.component_name.clone(),
                asset_path,
                asset_url,
                hash,
            });
        }

        // 3. Runtime bundle. The manifest entries are passed directly to
        //    render_runtime_entry_source, which handles serialisation
        //    internally. Stable filename `islands-runtime.js` —
        //    `ProductionAssetPipeline` would do any hashing pass.
        let runtime_entry_source = render_runtime_entry_source(&manifest);
        // Runtime bundle: same out-of-scope, splitting-off handling as the
        // per-island entries above — dynamic imports inline, no chunks emitted,
        // take only the entry JS.
        let runtime_js = self
            .bundle_one_entry(&runtime_entry_source, config, false)?
            .js;
        let runtime_asset_path = islands_dir.join("islands-runtime.js");
        std::fs::write(&runtime_asset_path, runtime_js.as_bytes())
            .with_context(|| format!("failed to write {}", runtime_asset_path.display()))?;
        let runtime_asset_url = island_link_href(&config.base_url, &runtime_asset_path);

        Ok(PerIslandBundleOutput {
            islands: bundle_entries,
            runtime_asset_path,
            runtime_asset_url,
        })
    }

    /// Internal: bundle a single in-memory entry source. Writes the
    /// source to a temp file (because esbuild expects entry points on
    /// disk), runs the subprocess, returns the bundled JS string.
    fn bundle_one_entry(
        &self,
        entry_source: &str,
        config: &BundleConfig,
        splitting: bool,
    ) -> Result<OneEntryOutput> {
        if self.config.mock_subprocess {
            // Mock mode: return either the configured mock_output (when
            // set) or the entry source itself so tests can assert the
            // generated entry shape end-to-end. Mock mode never produces
            // chunks — the real esbuild subprocess is the only chunk
            // source, so the splitting path is exercised by the
            // `#[ignore]` integration tests that run the actual binary.
            if !self.config.mock_output.is_empty() {
                return Ok(OneEntryOutput::js_only(self.config.mock_output.clone()));
            }
            return Ok(OneEntryOutput::js_only(entry_source.to_string()));
        }

        ensure_binary_verified(&self.config.binary_path, false)?;

        // Entry temp file (.tsx so esbuild's loader inference picks up
        // JSX automatically — the entry imports component .tsx files).
        //
        // **Why we allocate inside `working_dir` rather than `$TMPDIR`**
        //
        // The synthesised entry's bare imports (`@takazudo/zfb/runtime`,
        // `preact`) need to resolve against the project's
        // `node_modules/`. esbuild walks UP from the entry file's
        // directory looking for `node_modules`, **not** from process
        // cwd. If the entry lives at `/tmp/zfb-esbuild-entry-XXXX.tsx`,
        // esbuild walks `/tmp -> /` and never reaches the project, so
        // both bare imports fail with `Could not resolve "preact"` /
        // `Could not resolve "@takazudo/zfb/runtime"` (issue #147 /
        // zudolab/zudo-doc#1355 Wave 6 follow-up). Allocating inside
        // `working_dir` (= the consumer project root, set by
        // `EsbuildSubprocessConfig::with_working_dir`) puts the temp
        // file next to the project's `node_modules/` so esbuild's
        // upward walk finds the runtime + preact on the first hop.
        //
        // We fall back to the system tempdir (the original behaviour)
        // when `working_dir` does not exist on disk; that is the case
        // for the `zfb-islands` unit tests that construct an
        // `EsbuildSubprocessConfig::default()` from a repo whose
        // working_dir was wherever the test binary was launched from
        // (real builds always have an existing project root, so
        // production never takes this branch).
        let entry_tmp = if self.config.working_dir.is_dir() {
            tempfile::Builder::new()
                .prefix(".zfb-esbuild-entry-")
                .suffix(".tsx")
                .tempfile_in(&self.config.working_dir)
                .context("failed to allocate entry temp file inside working_dir")?
        } else {
            tempfile::Builder::new()
                .prefix("zfb-esbuild-entry-")
                .suffix(".tsx")
                .tempfile()
                .context("failed to allocate entry temp file")?
        };
        std::fs::write(entry_tmp.path(), entry_source.as_bytes())
            .context("failed to write entry temp file")?;

        // Staging output **directory**. `--splitting` requires `--outdir`
        // (esbuild rejects `--outfile` with splitting), and esbuild emits the
        // entry plus any code-split chunks here. We stage to a throwaway temp
        // dir and read every emitted file back into memory — the bundler never
        // writes into `dist/`; the prod pipeline / dev server own all `dist`
        // writes (see the `BundleOutput::chunks` contract). A system tempdir is
        // fine for the *output* (only the *entry* temp file must sit beside the
        // project's `node_modules` for esbuild's upward resolution walk).
        let out_dir = tempfile::Builder::new()
            .prefix("zfb-esbuild-out-")
            .tempdir()
            .context("failed to allocate out temp dir")?;

        // Plugin-registered aliases + virtual modules (#269). Both
        // surface through a synthetic `compilerOptions.paths` map
        // esbuild reads via `--tsconfig=<temp tsconfig>`, NOT as
        // `--alias` flags. esbuild's `--alias:<from>=<to>` is
        // prefix-with-slash — registering `@/foo` would silently
        // rewrite `@/foo/bar`, contradicting the embedded V8 host's
        // exact-match contract
        // (`zfb-render::BundleModuleLoader::resolve_alias`). A
        // wildcard-free `compilerOptions.paths` entry is a literal
        // exact match in TS / esbuild's path-mapping pipeline.
        //
        // `zfb_plugin_resolver::build_resolver_inputs` materializes
        // each virtual module's source to a `.zfb-virtual-*.mjs` temp
        // file inside `working_dir` (same node_modules-walk rationale
        // as the entry temp file above) and returns POSIX-normalized
        // `(specifier, absolute-path)` pairs. The held-alive temp
        // files live in `resolver_inputs._temp_files` and are dropped
        // after the subprocess returns.
        let resolver_inputs = zfb_plugin_resolver::build_resolver_inputs(
            &self.config.alias_entries,
            &self.config.virtual_modules,
            &self.config.working_dir,
        )
        .context("zfb-islands: failed materializing plugin resolver inputs")?;

        // Synthetic tsconfig. Allocated only when there is at least one
        // plugin-registered entry; otherwise islands stays byte-identical
        // to the no-plugin path — esbuild walks up from the entry file to
        // find the user's `tsconfig.json` as it does today. When allocated,
        // it seeds the user's own `compilerOptions.paths` first, then merges
        // the plugin entries on top (see `build_plugin_tsconfig`) — the
        // explicit `--tsconfig=` would otherwise drop the user's `@/`-style
        // aliases (#1238).
        let plugin_tsconfig = build_plugin_tsconfig(
            &self.config.working_dir,
            &resolver_inputs,
            "islands",
            "Synthetic tsconfig generated by zfb-islands::esbuild. \
             Drives plugin-registered alias / virtual-module \
             exact-match resolution through compilerOptions.paths.",
        )?;

        let args = build_esbuild_args(
            config,
            &self.config.extra_args,
            out_dir.path(),
            entry_tmp.path(),
            splitting,
        );
        let mut cmd = Command::new(&self.config.binary_path);
        cmd.current_dir(&self.config.working_dir);
        // Apply caller-supplied env vars (e.g. NODE_PATH=<embedded
        // node_modules path> so esbuild can resolve bare imports against
        // an externally-staged tree when the consumer project has no
        // on-disk `node_modules`).
        for (key, value) in &self.config.env_vars {
            cmd.env(key, value);
        }
        // `--tsconfig=<synthetic>` goes first so it's adjacent to its
        // adjacent in the argv (deterministic + reviewable). Only
        // emitted when plugin entries are present; the zero-plugin
        // path is byte-identical to the previous behaviour.
        if let Some(ref tsconfig_tmp) = plugin_tsconfig {
            cmd.arg(OsString::from(format!(
                "--tsconfig={}",
                tsconfig_tmp.path().display()
            )));
        }
        for arg in &args {
            cmd.arg(arg);
        }

        let output = cmd
            .output()
            .with_context(|| format!("failed to spawn {}", self.config.binary_path.display()))?;

        // Drop `resolver_inputs` and the synthetic tsconfig now — the
        // subprocess has finished and esbuild no longer needs either.
        // Explicit drops make the lifetime intent visible; both delete
        // their on-disk file via Drop.
        drop(resolver_inputs);
        drop(plugin_tsconfig);

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "esbuild exited with status {}: {}",
                output.status,
                stderr.trim()
            ));
        }

        read_back_outdir(out_dir.path())
    }
}

/// In-memory result of one `bundle_one_entry` pass: the entry JS plus any
/// code-split chunks esbuild emitted beside it.
///
/// `chunks` is empty for a zero-dynamic-import entry, always in mock mode,
/// and always on the per-island/runtime path (which bundles with
/// `splitting = false`, so esbuild inlines dynamic imports instead of
/// emitting chunks). The per-island path — not production-wired, chunk
/// shipping/serving out of scope — reads only `js`; the shared-bundle path
/// bundles with `splitting = true` and threads `chunks` into
/// [`BundleOutput`].
#[derive(Debug)]
struct OneEntryOutput {
    js: String,
    chunks: Vec<BundleChunk>,
}

impl OneEntryOutput {
    /// Entry-only output with no chunks (mock mode + the read-back path's
    /// zero-dynamic-import case).
    fn js_only(js: String) -> Self {
        Self {
            js,
            chunks: Vec::new(),
        }
    }
}

/// Read every file esbuild staged into `out_dir` back into memory, split
/// into the stable entry (`islands.js`) and its self-hashed chunks.
///
/// Contract enforced here (the read-back is the trust boundary for the
/// `BundleOutput::chunks` shape downstream consumers rely on):
///
/// - Exactly one entry named [`STABLE_ISLANDS_FILENAME`] must exist.
/// - Every *other* `.js` file is treated as a code-split chunk and must be
///   FLAT (the directory walk is non-recursive; we additionally reject any
///   name containing a path separator or `..` as defence-in-depth) and must
///   start with [`ESBUILD_CHUNK_FILENAME_PREFIX`]. Anything else is rejected
///   rather than silently shipped.
/// - Sourcemap siblings (`*.js.map`) are ignored on read-back, matching the
///   pre-splitting behaviour (the old single-file path requested
///   `--sourcemap=linked` but only ever read the `.js` back). Keeping that
///   exact behaviour means enabling splitting introduces zero new bytes for
///   the non-splitting case.
fn read_back_outdir(out_dir: &Path) -> Result<OneEntryOutput> {
    let mut entry_js: Option<String> = None;
    let mut chunks: Vec<BundleChunk> = Vec::new();

    for dirent in std::fs::read_dir(out_dir)
        .with_context(|| format!("failed to read esbuild outdir {}", out_dir.display()))?
    {
        let dirent = dirent.context("failed to read esbuild outdir entry")?;
        if !dirent.file_type().map(|t| t.is_file()).unwrap_or(false) {
            // esbuild emits flat files only; a non-file here would mean
            // an unexpected layout — skip directories/symlinks rather than
            // trust them.
            continue;
        }
        let os_name = dirent.file_name();
        let name = os_name.to_str().ok_or_else(|| {
            anyhow!(
                "esbuild emitted a non-UTF-8 filename in {}",
                out_dir.display()
            )
        })?;

        // Ignore sourcemap siblings (see fn doc) — never shipped today.
        if name.ends_with(".map") {
            continue;
        }

        if name == zfb_types::STABLE_ISLANDS_FILENAME {
            let js = std::fs::read_to_string(dirent.path())
                .context("failed to read esbuild entry output")?;
            entry_js = Some(js);
            continue;
        }

        // Everything else is a chunk — validate it hard before trusting it.
        // `filename` must be flat and `islands-chunk-*`-shaped; reject path
        // separators / traversal and any unexpected prefix. A future esbuild
        // change (or a botched naming template) that emits something else
        // surfaces as a build error here instead of leaking an arbitrary file
        // into `BundleOutput.chunks` (which the prod pipeline writes to
        // `dist/assets/` verbatim).
        validate_chunk_filename(name)?;
        let bytes = std::fs::read(dirent.path())
            .with_context(|| format!("failed to read esbuild chunk {name}"))?;
        chunks.push(BundleChunk {
            filename: name.to_string(),
            bytes,
        });
    }

    let js = entry_js.ok_or_else(|| {
        anyhow!(
            "esbuild produced no `{}` entry in {}",
            zfb_types::STABLE_ISLANDS_FILENAME,
            out_dir.display()
        )
    })?;

    // Sort chunks by filename for a deterministic `BundleOutput.chunks`
    // order (directory-iteration order is filesystem-dependent). The chunk
    // *names* are already content-stable across rebuilds; sorting makes the
    // Vec order stable too.
    chunks.sort_by(|a, b| a.filename.cmp(&b.filename));

    Ok(OneEntryOutput { js, chunks })
}

/// Validate a discovered chunk filename against the split contract: it must
/// be a flat basename (no path separators, no `..` traversal segment) and
/// carry the [`ESBUILD_CHUNK_FILENAME_PREFIX`]. Returns an error describing
/// the rejected name otherwise.
fn validate_chunk_filename(name: &str) -> Result<()> {
    if name.contains('/') || name.contains('\\') || name.contains(std::path::MAIN_SEPARATOR) {
        return Err(anyhow!(
            "esbuild chunk filename must be flat (no path separator): {name:?}"
        ));
    }
    // A legitimate self-hashed chunk basename never contains `..`; reject the
    // substring outright (defence-in-depth against traversal, belt-and-braces
    // with the separator check above).
    if name.contains("..") {
        return Err(anyhow!(
            "esbuild chunk filename must not contain `..`: {name:?}"
        ));
    }
    if !name.starts_with(ESBUILD_CHUNK_FILENAME_PREFIX) {
        return Err(anyhow!(
            "esbuild emitted an unexpected output file {name:?}; \
             expected the entry `{}` or a `{ESBUILD_CHUNK_FILENAME_PREFIX}*` chunk",
            zfb_types::STABLE_ISLANDS_FILENAME
        ));
    }
    Ok(())
}

/// Compose the esbuild CLI argument list for one entry-source bundle
/// pass. Split out from `bundle_one_entry` so unit tests can assert
/// against the flag list without spawning the subprocess.
///
/// The args mirror the historical `bundle_one_entry` shape verbatim
/// **plus** the `--jsx=automatic --jsx-import-source=<value>` pair
/// (issue #151). Without those two flags esbuild's default classic
/// JSX transform emits bare `React.createElement(…)` references that
/// throw `ReferenceError: React is not defined` at mount time when
/// host components have been migrated to `preact/compat`.
///
/// Order is stable across calls so callers (and tests) can rely on a
/// deterministic argv layout.
pub(crate) fn build_esbuild_args(
    config: &BundleConfig,
    extra_args: &[OsString],
    out_dir: &Path,
    entry_path: &Path,
    splitting: bool,
) -> Vec<OsString> {
    // The entry-name template must produce exactly `STABLE_ISLANDS_FILENAME`
    // (esbuild appends `.js`), or the read-back won't recognise the entry.
    // Assert the two stay in sync.
    debug_assert_eq!(
        format!("{ESBUILD_ENTRY_NAME_TEMPLATE}.js"),
        zfb_types::STABLE_ISLANDS_FILENAME,
        "entry-name template must match STABLE_ISLANDS_FILENAME stem"
    );
    build_esbuild_args_with_entry_name(
        config,
        extra_args,
        out_dir,
        entry_path,
        splitting,
        ESBUILD_ENTRY_NAME_TEMPLATE,
    )
}

/// Like [`build_esbuild_args`] but with a caller-supplied entry name
/// template for `--entry-names`. Used by the client-script bundling path
/// where the stable output filename must match `<entry_name>.js` rather
/// than the shared-bundle `islands.js`.
///
/// When `entry_name_template == ESBUILD_ENTRY_NAME_TEMPLATE` the output
/// is byte-identical to `build_esbuild_args`.
pub(crate) fn build_esbuild_args_with_entry_name(
    config: &BundleConfig,
    extra_args: &[OsString],
    out_dir: &Path,
    entry_path: &Path,
    splitting: bool,
    entry_name_template: &str,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    args.push(OsString::from("--bundle"));
    args.push(OsString::from("--format=esm"));
    // Code-splitting is enabled ONLY for the shared-bundle path (esm-only; we
    // already force `--format=esm`). With splitting on, esbuild emits the entry
    // plus one chunk per shared / dynamically-imported module graph, so a
    // consumer's `import("shiki")` lands in its own chunk instead of being
    // inlined into the multi-MB entry every page loads (issue #800). A
    // zero-dynamic-import project emits only the entry — identical to the
    // pre-splitting single-file build.
    //
    // The per-island and runtime paths pass `splitting = false`: their chunk
    // shipping/serving is OUT OF SCOPE (#802/#806), and those paths read only
    // the entry `.js` (dropping chunks). With splitting on, an island doing
    // `import("./heavy")` would emit a relative `import("./islands-chunk-*.js")`
    // referencing a chunk file that never gets written — a dangling import. With
    // splitting off, esbuild INLINES dynamic imports into the single entry,
    // matching the pre-#806 behaviour and keeping the per-island output
    // self-contained.
    if splitting {
        args.push(OsString::from("--splitting=true"));
    }
    args.push(OsString::from("--tree-shaking=true"));
    // Per-island bundles ship to the browser. See `produce_bundle_js`
    // for the full rationale; mirrored here so both the legacy
    // shared-bundle and per-island paths stay aligned on platform
    // target + node:* externalization.
    args.push(OsString::from("--platform=browser"));
    args.push(OsString::from("--external:node:*"));
    // Issue #151: route esbuild through the automatic JSX transform
    // pointed at the host framework's import source (typically
    // `"preact"`). Without these two flags esbuild defaults to the
    // classic transform and emits bare `React.createElement` /
    // `React.Fragment` references; islands using `preact/compat` for
    // hooks have no `React` binding and crash at mount time
    // (zudolab/zudo-doc#1355 Wave 8).
    args.push(OsString::from("--jsx=automatic"));
    args.push(OsString::from(format!(
        "--jsx-import-source={}",
        config.jsx_import_source
    )));
    // Mirror the main SSR bundler's Preact `--alias` block in
    // `crates/zfb-build/src/bundler.rs` (the `Framework::Preact` arm of the
    // SSR esbuild command builder). next.18 dist modules carry an explicit,
    // framework-neutral `import { jsx } from "react/jsx-runtime"` (e.g.
    // `@takazudo/zfb-runtime/client-router`, which the shared islands bundle
    // side-effect-imports when `client_router` is set — see
    // `render_shared_bundle_entry_source`). In a Preact project `react` is not
    // installed, so esbuild cannot resolve `react/jsx-runtime` and the islands
    // build aborts (issue #633). Rewrite it (and the dev-runtime sibling) to the
    // Preact runtime — the same trick the SSR bundler and the wider Preact
    // ecosystem use. React projects resolve `react/jsx-runtime` natively, so the
    // alias is Preact-only (same gate as the SSR bundler), leaving the React
    // argv byte-identical. `--alias`'s prefix-with-slash semantics are safe here
    // because `react/jsx-runtime` has no deeper subpath to corrupt.
    if matches!(
        FrameworkKind::from_jsx_import_source(&config.jsx_import_source),
        FrameworkKind::Preact
    ) {
        args.push(OsString::from(
            "--alias:react/jsx-runtime=preact/jsx-runtime",
        ));
        args.push(OsString::from(
            "--alias:react/jsx-dev-runtime=preact/jsx-dev-runtime",
        ));
    }
    if config.minify {
        args.push(OsString::from("--minify"));
    }
    if config.sourcemap {
        args.push(OsString::from("--sourcemap=linked"));
    }
    // Directory-mode output: esbuild writes the entry (and, when splitting is
    // on, any chunks) into the *staging* `out_dir` (a throwaway tempdir —
    // NOT `dist/assets/`). `--outdir` is always present — the read-back
    // walks this dir for the `<entry_name_template>.js` entry regardless of
    // splitting. `--entry-names` pins the entry's basename to `entry_name_template`
    // (esbuild appends `.js`) so the read-back can identify it by name.
    // `--chunk-names` (splitting only) self-content-hashes each chunk FLAT
    // in the same dir (`islands-chunk-<hash>.js`); esbuild bakes relative
    // `import("./...")` references between them, so names must never be
    // renamed downstream. Omitted without `--splitting`.
    // See `ESBUILD_ENTRY_NAME_TEMPLATE` / `ESBUILD_CHUNK_NAME_TEMPLATE` and
    // the `BundleOutput::chunks` contract for the full rationale.
    // Flag spellings verified against the pinned esbuild 0.25.x CLI.
    args.push(OsString::from(format!("--outdir={}", out_dir.display())));
    args.push(OsString::from(format!(
        "--entry-names={entry_name_template}"
    )));
    if splitting {
        args.push(OsString::from(format!(
            "--chunk-names={ESBUILD_CHUNK_NAME_TEMPLATE}"
        )));
    }
    // Inline NODE_ENV so React/Preact pick their production build
    // when minifying. esbuild expects the define value to be a JS
    // literal, hence the embedded quotes.
    if config.minify {
        args.push(OsString::from(
            "--define:process.env.NODE_ENV=\"production\"",
        ));
    } else {
        args.push(OsString::from(
            "--define:process.env.NODE_ENV=\"development\"",
        ));
    }
    // Inline `import.meta.env.{PROD,DEV}` so consumer `'use client'`
    // code that references either expression (e.g. `if
    // (import.meta.env.DEV) console.log(…)`) is folded at bundle time
    // instead of leaving the literal expression in the shipped bundle.
    // The shared-bundle path (`crates/zfb-build/src/bundler.rs::2395`)
    // already does this for the SSR/runtime bundle; mirroring here
    // keeps both pipelines aligned. Without these defines,
    // `import.meta.env` is `undefined` at module-init time in the
    // browser and `import.meta.env.DEV` throws
    // `TypeError: Cannot read properties of undefined (reading 'DEV')`
    // before any island can hydrate (issue #287).
    let prod = config.minify;
    args.push(OsString::from(format!(
        "--define:import.meta.env.PROD={}",
        prod
    )));
    args.push(OsString::from(format!(
        "--define:import.meta.env.DEV={}",
        !prod
    )));
    for extra in extra_args {
        args.push(extra.clone());
    }
    args.push(OsString::from(entry_path.as_os_str()));
    args
}

/// JS boolean expression that tests whether `value_var` is component-shaped:
/// a plain function, or a compat `memo()`/`forwardRef()` object carrying
/// `$$typeof` (issue #998). `dollar_receiver` is the expression used to read
/// `.$$typeof` off the value — the per-island sites (TS `.tsx` entries where
/// the value is typed `any`) cast via `(Component as any)`, the shared-bundle
/// sites read it directly. Hoisted so all four island render paths share one
/// predicate instead of copy-pasting it.
fn component_shape_predicate(value_var: &str, dollar_receiver: &str) -> String {
    format!(
        "typeof {value_var} === \"function\" || (typeof {value_var} === \"object\" && {value_var} !== null && {dollar_receiver}.$$typeof)"
    )
}

/// JS `console.warn(...)` statement emitted when an island export is not
/// component-shaped (issue #998). `export_name_expr` / `module_label_expr`
/// are JS expressions naming the export and its source module; `action` is
/// the skipped-verb phrase (`"registration"` for the shared bundle,
/// `"mount"` for per-island). Hoisted so all four island render paths emit
/// one identical warning string instead of copy-pasting it.
fn non_component_warn(
    value_var: &str,
    export_name_expr: &str,
    module_label_expr: &str,
    action: &str,
) -> String {
    format!(
        "console.warn(\"[zfb] island export \" + {export_name_expr} + \" from \" + {module_label_expr} + \" is not a component (got \" + ({value_var} === null ? \"null\" : typeof {value_var}) + \"); skipping {action}.\");"
    )
}

/// Generate the synthetic single-entry source for the legacy shared
/// bundle.
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
/// per-island path emits `mountIslands(MANIFEST)` from
/// `render_runtime_entry_source`; the shared-bundle path mirrors that
/// here so the islands bundle is the only script the page needs to load
/// to get hydration glue **and** the island source code in one HTTP
/// request.
///
/// The synthesised entry imports each island as a namespace and
/// **registers** it into a manifest at top-level using a
/// `__zfb_register(ns, exportName, markerName, moduleLabel)` helper: the
/// helper picks the component (`__zfb_pick` — named export first, then
/// `default`) and stashes a mount thunk under `markerName`.
///
/// `markerName` is the **scanner-derived SSR-marker name** baked into the
/// generated source as a static JSON literal — NOT a runtime
/// `displayName ?? name` read. It matches the value the SSR side
/// (`packages/zfb/src/island.ts::captureComponentName`, which derives
/// `data-zfb-island="…"` from `type.displayName ?? type.name`) writes onto
/// the marker, so both sides of the boundary agree on the key. Keying on
/// the export-side `Island::component_name` instead would collapse every
/// host-shape default-export island onto the literal `"default"` key (issue
/// #149); a runtime `function.name` read is unsafe because esbuild
/// minification renames functions (lesson from PR #148). `moduleLabel`
/// names the source module in the non-component skip warning (#998). See
/// the inline `__zfb_register` comment below for the full rationale.
///
/// The runtime's `mountIslands` accepts this object shape directly
/// (no second dynamic import) — see the `IslandManifestValue`
/// widening in `packages/zfb/src/runtime.ts`. Each `mount` thunk
/// mirrors `render_island_entry_source(Preact, …)`: build a vnode and
/// dispatch to `hydrate` / `render` based on the SSR / SSR-skip mode.
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
/// Each island path / component name is JSON-encoded to defang stray
/// quotes / backslashes from absolute paths on Windows or arbitrary
/// file names; the order follows the input slice so the resulting
/// bytes are deterministic for a given `(islands, config)` pair
/// (downstream tests rely on this for the bundle's content hash to be
/// stable across runs).
///
/// # Framework
///
/// The synthesised wrapper emits framework-specific imports + mount
/// glue selected by `framework`, mirroring the per-island path's
/// `FrameworkKind` branches in `render_island_entry_source`:
/// - `Preact` → bare `preact` imports + `h()/hydrate()/render()` glue.
/// - `React` → `react` `createElement` + `react-dom/client`
///   `hydrateRoot`/`createRoot` glue with a module-scope `WeakMap` of
///   roots so `unmount` can dispose the right root.
///
/// Mixed-framework projects are not supported by the production
/// shared-bundle path today (the per-island path is the right home for
/// that, since it composes the framework glue per-island). The whole
/// project shares one `framework`, derived from the configured
/// `jsx_import_source`.
pub fn render_shared_bundle_entry_source(
    framework: FrameworkKind,
    islands: &[Island],
    client_router: bool,
) -> String {
    let mut out =
        String::from("// Generated by zfb-islands::EsbuildSubprocessBundler::produce_bundle_js\n");
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
    out.push_str(r#"import { mountIslands } from "@takazudo/zfb/runtime";"#);
    out.push('\n');
    // Framework-specific hydration imports. Both branches mirror the
    // per-island path's `FrameworkKind` arms in
    // `render_island_entry_source`. For React the roots map is declared
    // at module scope (one WeakMap shared across all islands) so each
    // manifest entry's `unmount` thunk can find and dispose the root it
    // created with `hydrateRoot`/`createRoot`.
    match framework {
        FrameworkKind::Preact => {
            out.push_str(r#"import { h, hydrate, render } from "preact";"#);
            out.push('\n');
        }
        FrameworkKind::React => {
            out.push_str(r#"import { createElement } from "react";"#);
            out.push('\n');
            out.push_str(r#"import { hydrateRoot, createRoot } from "react-dom/client";"#);
            out.push('\n');
            out.push_str("const __zfb_roots = new WeakMap();\n");
        }
    }
    for (i, island) in islands.iter().enumerate() {
        let path = island.source_path.to_string_lossy();
        out.push_str(&format!(
            "import * as __zfb_island_{i} from {};\n",
            json_string(&path)
        ));
    }
    // Manifest registration helper.
    //
    // `__zfb_pick(ns, exportName)` returns the component value: it
    // prefers a *truthy* named export under `exportName` (so that
    // `ns.default = function Foo(){}` plus `ns.Foo = undefined` does
    // not pick `undefined` and lose the component) and falls back to
    // `ns.default`. That mirrors `render_island_entry_source` for the
    // per-island path.
    //
    // `__zfb_register(ns, exportName, markerName, moduleLabel)` writes a
    // mount thunk for the resolved component under the **static marker
    // name** the scanner discovered for this island (issue #149). The marker
    // name is a JSON-encoded literal in the generated source, NOT a runtime
    // introspection of `displayName ?? name` — that's the lesson from the
    // previous round (PR #148): esbuild minification renames functions, so
    // `function.name` is unstable and unsafe to key the manifest on.
    // `moduleLabel` names the source module in the non-component skip
    // warning (#998).
    //
    // The mount thunk picks `hydrate` vs `render` based on the SSR /
    // SSR-skip mode the runtime supplies, mirroring the per-island
    // entry script's behaviour exactly.
    out.push_str(
        "const __zfb_manifest = {};\n\
function __zfb_pick(ns, exportName) {\n\
  const named = ns[exportName];\n\
  return (named !== undefined && named !== null) ? named : ns.default;\n\
}\n",
    );
    // The mount / unmount thunk bodies are framework-specific and mirror
    // the per-island path's `FrameworkKind` arms exactly. Keeping the
    // helper-function shape identical across frameworks (same name, same
    // args, same `__zfb_manifest[markerName]` slot) means only the thunk
    // internals change. The guard prelude (component-shape check + the
    // non-component skip warning) is identical for both frameworks, so it
    // is built once from the shared helpers (#998).
    let register_prelude = format!(
        "function __zfb_register(ns, exportName, markerName, moduleLabel) {{\n\
  const C = __zfb_pick(ns, exportName);\n\
  if (!({predicate})) {{\n\
    {warn}\n\
    return;\n\
  }}\n",
        predicate = component_shape_predicate("C", "C"),
        warn = non_component_warn("C", "exportName", "moduleLabel", "registration"),
    );
    match framework {
        FrameworkKind::Preact => {
            out.push_str(&register_prelude);
            out.push_str(
                "  __zfb_manifest[markerName] = {\n\
    mount: (props, element, mode) => {\n\
      const v = h(C, props);\n\
      if (mode === \"hydrate\") { hydrate(v, element); } else { render(v, element); }\n\
    },\n\
    unmount: (element) => { render(null, element); },\n\
  };\n\
}\n",
            );
        }
        FrameworkKind::React => {
            out.push_str(&register_prelude);
            out.push_str(
                "  __zfb_manifest[markerName] = {\n\
    mount: (props, element, mode) => {\n\
      const v = createElement(C, props);\n\
      if (mode === \"hydrate\") {\n\
        const root = hydrateRoot(element, v);\n\
        __zfb_roots.set(element, root);\n\
      } else {\n\
        const root = createRoot(element);\n\
        root.render(v);\n\
        __zfb_roots.set(element, root);\n\
      }\n\
    },\n\
    unmount: (element) => {\n\
      const root = __zfb_roots.get(element);\n\
      if (root) { root.unmount(); __zfb_roots.delete(element); }\n\
    },\n\
  };\n\
}\n",
            );
        }
    }
    // One register call per island. The `__zfb_register(...)` calls
    // are top-level side effects esbuild MUST preserve, and they
    // reference each namespace identifier — so tree-shaking retains
    // every island's exports just as the previous
    // `(globalThis).__zfb_islands ??= [...]` anchor did (#144).
    //
    // The third argument is the **scanner-derived SSR-marker name**
    // (`Island::marker_name`), which matches the value the SSR side
    // writes into `data-zfb-island` / `data-zfb-island-skip-ssr`. For
    // host-shape default-export islands the scanner uses the function
    // identifier name (`export default function FooBar()` →
    // `"FooBar"`); for SSR-skip wrappers it uses the literal first
    // argument of `renderSsrSkipPlaceholder("X", …)` — see
    // `crates/zfb-islands/src/scanner.rs::exported_island_records`.
    for (i, island) in islands.iter().enumerate() {
        let name_lit = json_string(&island.component_name);
        let marker_lit = json_string(&island.marker_name);
        // Issue #998: pass the resolved source path as `moduleLabel` so the
        // non-component skip warning can name which module the bad export
        // came from.
        let module_lit = json_string(&island.source_path.to_string_lossy());
        out.push_str(&format!(
            "__zfb_register(__zfb_island_{i}, {name_lit}, {marker_lit}, {module_lit});\n"
        ));
    }
    out.push_str("mountIslands(__zfb_manifest);\n");
    out
}

/// Generate the per-island entry script for `framework`.
///
/// The entry imports the component module by its resolved source path
/// (esbuild resolves it from the bundler's working directory), wraps
/// it in framework-specific `mount(props, element, mode)` glue, and
/// exposes the function as the bundle's default export. The hydration
/// runtime dynamic-imports this default export.
///
/// Mode is one of:
/// - `"hydrate"` — used for SSR'd islands (`[data-zfb-island]`).
/// - `"render"` — used for SSR-skip islands
///   (`[data-zfb-island-skip-ssr]`); avoids hydrate-mismatch warnings
///   because the DOM is empty when we mount.
pub fn render_island_entry_source(framework: FrameworkKind, island: &Island) -> String {
    let path = island.source_path.to_string_lossy();
    let path_lit = json_string(&path);
    let component_name = &island.component_name;
    let component_lit = json_string(component_name);
    // Component-shape guard + non-component skip warning, hoisted into the
    // shared helpers so the shared-bundle path and both per-island framework
    // arms emit one identical snippet (#998). `Component` is typed `any`
    // here (it comes off `(Mod as any)[…]`), so the `$$typeof` read is cast.
    let predicate = component_shape_predicate("Component", "(Component as any)");
    let warn = non_component_warn("Component", &component_lit, &path_lit, "mount");
    match framework {
        FrameworkKind::Preact => format!(
            r#"// Generated by zfb-islands::EsbuildSubprocessBundler::bundle_per_island
import * as Mod from {path_lit};
import {{ h, hydrate, render }} from "preact";
const Component = (Mod as any)[{component_lit}] ?? (Mod as any).default;
// Issue #998: only mount component-shaped exports. A plain function is a
// component; the compat memo()/forwardRef() helpers produce an object
// carrying `$$typeof`. Anything else (a string/object constant that slipped
// through as an island marker) is skipped with a loud warning rather than
// handed to h() — which would otherwise build a DOM element from a bogus type.
const __zfb_ok = {predicate};
if (!__zfb_ok) {{
  {warn}
}}
export function mount(props, element, mode) {{
  if (!__zfb_ok) return;
  const vnode = h(Component, props);
  if (mode === "hydrate") {{
    hydrate(vnode, element);
  }} else {{
    render(vnode, element);
  }}
}}
export function unmount(element) {{
  if (!__zfb_ok) return;
  render(null, element);
}}
export default mount;
"#
        ),
        FrameworkKind::React => format!(
            r#"// Generated by zfb-islands::EsbuildSubprocessBundler::bundle_per_island
import * as Mod from {path_lit};
import {{ createElement }} from "react";
import {{ hydrateRoot, createRoot }} from "react-dom/client";
const Component = (Mod as any)[{component_lit}] ?? (Mod as any).default;
// Issue #998: only mount component-shaped exports. A plain function is a
// component; react memo()/forwardRef() produce an object carrying
// `$$typeof`. Anything else (a string/object constant that slipped through
// as an island marker) is skipped with a loud warning rather than handed to
// createElement() — which would otherwise try to build a DOM element from a
// bogus type.
const __zfb_ok = {predicate};
if (!__zfb_ok) {{
  {warn}
}}
const __zfb_roots = new WeakMap();
export function mount(props, element, mode) {{
  if (!__zfb_ok) return;
  const vnode = createElement(Component, props);
  if (mode === "hydrate") {{
    const root = hydrateRoot(element, vnode);
    __zfb_roots.set(element, root);
  }} else {{
    const root = createRoot(element);
    root.render(vnode);
    __zfb_roots.set(element, root);
  }}
}}
export function unmount(element) {{
  if (!__zfb_ok) return;
  const root = __zfb_roots.get(element);
  if (root) {{ root.unmount(); __zfb_roots.delete(element); }}
}}
export default mount;
"#
        ),
    }
}

/// Generate the runtime entry script from a manifest of
/// `(marker_name → asset_url)` pairs.
///
/// The runtime imports the framework-agnostic hydration runtime from
/// the `zfb` SDK package, hands it the manifest so it can
/// dynamic-import the right per-island bundle for each
/// `[data-zfb-island]` element. The manifest is inlined as a JSON
/// literal — no extra fetch — so the runtime works in static-host
/// environments.
///
/// The `framework` parameter has been removed; the shim is
/// framework-agnostic because each island's per-island bundle carries
/// the framework-specific glue. This function takes the manifest
/// entries directly rather than a pre-serialised JSON string so
/// callers do not need to reach into the private [`serialize_manifest`]
/// helper.
pub fn render_runtime_entry_source(manifest: &[(String, String)]) -> String {
    let manifest_json = serialize_manifest(manifest);
    format!(
        r#"// Generated by zfb-islands::EsbuildSubprocessBundler::bundle_per_island
import {{ mountIslands }} from "@takazudo/zfb/runtime";
const ISLAND_MANIFEST = {manifest_json};
mountIslands(ISLAND_MANIFEST);
"#
    )
}

/// Serialize a manifest of `(marker_name → asset_url)` pairs to a
/// JSON object literal. Order of keys is preserved (the runtime does
/// not rely on it but determinism matters for hashing).
fn serialize_manifest(entries: &[(String, String)]) -> String {
    let mut s = String::from("{");
    for (i, (k, v)) in entries.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&json_string(k));
        s.push(':');
        s.push_str(&json_string(v));
    }
    s.push('}');
    s
}

impl ClientBundler for EsbuildSubprocessBundler {
    fn bundle(&self, islands: &[Island], config: &BundleConfig) -> Result<BundleOutput> {
        let OneEntryOutput { js, chunks } = self.produce_bundle_js(islands, config)?;

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
        })
    }
}

impl EsbuildSubprocessBundler {
    /// Bundle a single client-script file (`.client.{ts,tsx,js,jsx}`) directly.
    ///
    /// Unlike the shared-islands path (which synthesises a virtual entry that
    /// imports all islands), a client-script file IS the entry — no synthetic
    /// wrapper is generated. The file is bundled with `--splitting=false` so any
    /// dynamic `import()` calls are inlined into the single output file (identical
    /// rationale to the per-island path at line 619-628).
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
        if self.config.mock_subprocess {
            if !self.config.mock_output.is_empty() {
                return Ok(self.config.mock_output.clone());
            }
            // In mock mode with no canned output, read the file off disk and
            // return its source text (same convention as `bundle_one_entry`'s
            // mock path: return what was given so callers can assert the shape).
            return std::fs::read_to_string(entry_path).with_context(|| {
                format!("mock: failed to read entry file {}", entry_path.display())
            });
        }

        ensure_binary_verified(&self.config.binary_path, false)?;

        // Staging output directory — same pattern as `bundle_one_entry`.
        // A system tempdir is fine for the output because esbuild's
        // `node_modules` upward-walk starts from the actual entry file's
        // directory (not the outdir), so bare imports resolve against the
        // project's `node_modules/` naturally.
        let out_dir = tempfile::Builder::new()
            .prefix("zfb-esbuild-client-out-")
            .tempdir()
            .context("failed to allocate out temp dir for client script bundling")?;

        let resolver_inputs = zfb_plugin_resolver::build_resolver_inputs(
            &self.config.alias_entries,
            &self.config.virtual_modules,
            &self.config.working_dir,
        )
        .context("zfb-islands: failed materializing plugin resolver inputs for client script")?;

        let plugin_tsconfig = build_plugin_tsconfig(
            &self.config.working_dir,
            &resolver_inputs,
            "client-script",
            "Synthetic tsconfig generated by zfb-islands::esbuild (client-script path). \
             Drives plugin-registered alias / virtual-module \
             exact-match resolution through compilerOptions.paths.",
        )?;

        // Build the esbuild args reusing the shared arg-builder. Pass
        // `splitting = false` — client-script bundles inline all dynamic
        // imports (same rationale as per-island path; no chunk shipping in v1).
        let args = build_esbuild_args_with_entry_name(
            config,
            &self.config.extra_args,
            out_dir.path(),
            entry_path,
            false, // splitting = false
            entry_name,
        );

        let mut cmd = Command::new(&self.config.binary_path);
        cmd.current_dir(&self.config.working_dir);
        for (key, value) in &self.config.env_vars {
            cmd.env(key, value);
        }
        if let Some(ref tsconfig_tmp) = plugin_tsconfig {
            cmd.arg(OsString::from(format!(
                "--tsconfig={}",
                tsconfig_tmp.path().display()
            )));
        }
        for arg in &args {
            cmd.arg(arg);
        }

        let output = cmd
            .output()
            .with_context(|| format!("failed to spawn {}", self.config.binary_path.display()))?;

        drop(resolver_inputs);
        drop(plugin_tsconfig);

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "esbuild exited with status {} bundling client script {:?}: {}",
                output.status,
                entry_path,
                stderr.trim()
            ));
        }

        // Read back the single output file named `<entry_name>.js`.
        let expected_filename = format!("{entry_name}.js");
        let expected_path = out_dir.path().join(&expected_filename);
        std::fs::read_to_string(&expected_path).with_context(|| {
            format!(
                "esbuild produced no `{expected_filename}` for client-script entry `{entry_name}`"
            )
        })
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
    fn json_string_escapes_specials() {
        assert_eq!(json_string("a/b"), "\"a/b\"");
        assert_eq!(json_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(json_string("a\\b"), "\"a\\\\b\"");
        assert_eq!(json_string("a\nb"), "\"a\\nb\"");
    }

    #[test]
    fn serialize_manifest_round_trips_through_serde_json() {
        // Stable URLs per the S0 contract — hashing happens later in
        // `ProductionAssetPipeline`, not in the bundler.
        let entries = vec![
            ("Counter".into(), "/islands/Counter.js".into()),
            ("Button".into(), "/islands/Button.js".into()),
        ];
        let s = serialize_manifest(&entries);
        let parsed: serde_json::Value = serde_json::from_str(&s).expect("valid json");
        assert_eq!(parsed["Counter"].as_str(), Some("/islands/Counter.js"));
        assert_eq!(parsed["Button"].as_str(), Some("/islands/Button.js"));
    }

    #[test]
    fn render_island_entry_source_preact_imports_bare_preact() {
        let island = Island::new("Counter", "/abs/components/Counter.tsx");
        let src = render_island_entry_source(FrameworkKind::Preact, &island);
        assert!(src.contains(r#"from "preact""#));
        assert!(!src.contains("preact/compat"));
        assert!(src.contains("hydrate"));
        assert!(src.contains("render"));
        assert!(src.contains(r#"from "/abs/components/Counter.tsx""#));
        assert!(src.contains("export function mount"));
        assert!(src.contains("export default mount"));
        // Per-island Preact bundles must export unmount for lifecycle cleanup.
        assert!(
            src.contains("export function unmount"),
            "expected unmount export in Preact bundle: {src}"
        );
        assert!(
            src.contains("render(null, element)"),
            "expected render(null, element) in Preact unmount: {src}"
        );
    }

    #[test]
    fn render_island_entry_source_react_uses_client_apis() {
        let island = Island::new("Modal", "/abs/components/Modal.tsx");
        let src = render_island_entry_source(FrameworkKind::React, &island);
        assert!(src.contains(r#"from "react-dom/client""#));
        assert!(src.contains("hydrateRoot"));
        assert!(src.contains("createRoot"));
        // Distinguish hydrate path (SSR'd) from render path (SSR-skip).
        assert!(src.contains(r#"mode === "hydrate""#));
        // Per-island React bundles must export unmount using WeakMap-tracked Root.
        assert!(
            src.contains("export function unmount"),
            "expected unmount export in React bundle: {src}"
        );
        assert!(
            src.contains("__zfb_roots"),
            "expected WeakMap __zfb_roots in React bundle: {src}"
        );
        assert!(
            src.contains("root.unmount()"),
            "expected root.unmount() call in React unmount: {src}"
        );
        // #1002: unmount must carry the same __zfb_ok gate as the Preact arm.
        assert!(
            src.contains("export function unmount(element) {\n  if (!__zfb_ok) return;"),
            "expected __zfb_ok gate in React unmount: {src}"
        );
    }

    #[test]
    fn render_runtime_entry_source_inlines_manifest_and_calls_mount() {
        // Stable URLs per the S0 contract.
        let manifest = vec![
            ("Counter".into(), "/islands/Counter.js".into()),
            ("Button".into(), "/islands/Button.js".into()),
        ];
        let src = render_runtime_entry_source(&manifest);
        assert!(src.contains(r#"import { mountIslands } from "@takazudo/zfb/runtime""#));
        assert!(src.contains(r#""Counter":"/islands/Counter.js""#));
        assert!(src.contains("mountIslands(ISLAND_MANIFEST);"));
    }

    #[test]
    fn bundle_per_island_in_mock_mode_writes_per_island_assets_and_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = EsbuildSubprocessConfig::default();
        // Empty mock_output makes bundle_one_entry echo the entry source
        // back, so each island's bytes (and therefore its hash) reflect
        // the deterministic entry shape.
        let bundler = EsbuildSubprocessBundler::new(EsbuildSubprocessConfig {
            mock_subprocess: true,
            ..cfg
        });
        let islands = vec![
            Island::new("Counter", "/abs/components/Counter.tsx"),
            Island::new("Modal", "/abs/components/Modal.tsx"),
        ];
        let config = BundleConfig::default()
            .with_outdir(dir.path().to_path_buf())
            .with_base_url("/");
        let out = bundler
            .bundle_per_island(&islands, FrameworkKind::Preact, &config)
            .expect("bundle_per_island in mock mode succeeds");

        // Two per-island bundles, in the same order as input.
        assert_eq!(out.islands.len(), 2);
        assert_eq!(out.islands[0].component_name, "Counter");
        assert_eq!(out.islands[1].component_name, "Modal");

        for (i, entry) in out.islands.iter().enumerate() {
            // Hash is still computed and exposed (8 lowercase hex
            // chars) for dev-mode change detection, but it is **not**
            // part of the on-disk filename or URL — those are stable
            // per the S0 single-source-of-truth-for-hashing contract.
            assert_eq!(entry.hash.len(), 8);
            assert!(entry.hash.chars().all(|c| c.is_ascii_hexdigit()));
            // File exists on disk at the stable sequential path
            // `<outdir>/islands/island-{i}.js`.
            assert!(entry.asset_path.exists(), "{:?}", entry.asset_path);
            assert!(entry.asset_path.starts_with(dir.path().join("islands")));
            let expected_filename = format!("island-{i}.js");
            assert_eq!(
                entry.asset_path.file_name().unwrap().to_string_lossy(),
                expected_filename,
            );
            // Public URL is the stable `/islands/island-{i}.js`.
            assert_eq!(entry.asset_url, format!("/islands/{expected_filename}"));
        }

        // Runtime bundle exists at the stable path
        // `<outdir>/islands/islands-runtime.js` (no hash suffix) and
        // bears the matching stable URL `/islands/islands-runtime.js`.
        assert!(out.runtime_asset_path.exists());
        assert_eq!(
            out.runtime_asset_path
                .file_name()
                .unwrap()
                .to_string_lossy(),
            "islands-runtime.js",
        );
        assert_eq!(out.runtime_asset_url, "/islands/islands-runtime.js");
    }

    #[test]
    fn bundle_per_island_is_deterministic_across_runs() {
        let dir = tempfile::tempdir().unwrap();
        let bundler = EsbuildSubprocessBundler::new(EsbuildSubprocessConfig {
            mock_subprocess: true,
            ..EsbuildSubprocessConfig::default()
        });
        let islands = vec![Island::new("Counter", "/abs/components/Counter.tsx")];
        let config = BundleConfig::default().with_outdir(dir.path().to_path_buf());
        let a = bundler
            .bundle_per_island(&islands, FrameworkKind::Preact, &config)
            .unwrap();
        let b = bundler
            .bundle_per_island(&islands, FrameworkKind::Preact, &config)
            .unwrap();
        // Stable filenames mean the URLs are determined entirely by
        // the input slice — independent of the bundled byte stream.
        // The exposed `hash` field still varies with byte content and
        // therefore must also be stable across identical inputs.
        assert_eq!(a.islands[0].hash, b.islands[0].hash);
        assert_eq!(a.islands[0].asset_url, b.islands[0].asset_url);
        assert_eq!(a.runtime_asset_url, b.runtime_asset_url);
    }

    #[test]
    fn bundle_per_island_default_export_islands_do_not_collide() {
        // Two islands from different files both with marker_name "default"
        // (anonymous default exports). With component_name-based filenames they
        // would have written to the same "default.js" path; the sequential
        // island-{i}.js scheme guarantees distinct files.
        let dir = tempfile::tempdir().unwrap();
        let bundler = EsbuildSubprocessBundler::new(EsbuildSubprocessConfig {
            mock_subprocess: true,
            ..EsbuildSubprocessConfig::default()
        });
        // marker_name="default" for both, but distinct source_path.
        // component_name values ("Foo" / "Bar") are used for the SSR-side
        // pairing; the runtime manifest key is the marker_name ("Foo"/"Bar"
        // here, which are also the component_names — note with_marker_name
        // takes (component_name, source_path, marker_name)).
        let islands = vec![
            Island::with_marker_name("default", "/abs/A.tsx", "Foo"),
            Island::with_marker_name("default", "/abs/B.tsx", "Bar"),
        ];
        let config = BundleConfig::default()
            .with_outdir(dir.path().to_path_buf())
            .with_base_url("/");
        let out = bundler
            .bundle_per_island(&islands, FrameworkKind::Preact, &config)
            .expect("bundle_per_island with colliding marker_names succeeds");

        assert_eq!(out.islands.len(), 2);
        // Sequential filenames are distinct — no overwrite.
        assert_eq!(
            out.islands[0]
                .asset_path
                .file_name()
                .unwrap()
                .to_string_lossy(),
            "island-0.js",
        );
        assert_eq!(
            out.islands[1]
                .asset_path
                .file_name()
                .unwrap()
                .to_string_lossy(),
            "island-1.js",
        );
        assert!(out.islands[0].asset_path.exists());
        assert!(out.islands[1].asset_path.exists());
        // Files have different content (different source_path in entry source).
        let content_0 = std::fs::read(&out.islands[0].asset_path).unwrap();
        let content_1 = std::fs::read(&out.islands[1].asset_path).unwrap();
        assert_ne!(
            content_0, content_1,
            "island-0.js and island-1.js must have distinct content"
        );
        // Runtime manifest has two distinct entries, keyed by marker_name
        // ("Foo" and "Bar"), each pointing to the correct island-{i}.js URL.
        let runtime_src = std::fs::read_to_string(&out.runtime_asset_path).unwrap();
        assert!(
            runtime_src.contains(r#""Foo":"/islands/island-0.js""#),
            "runtime manifest must contain Foo entry: {runtime_src}"
        );
        assert!(
            runtime_src.contains(r#""Bar":"/islands/island-1.js""#),
            "runtime manifest must contain Bar entry: {runtime_src}"
        );
    }

    #[test]
    fn bundle_per_island_manifest_keys_use_marker_name() {
        // A default-export island whose marker_name differs from component_name:
        // the runtime manifest key must be marker_name, not component_name.
        let dir = tempfile::tempdir().unwrap();
        let bundler = EsbuildSubprocessBundler::new(EsbuildSubprocessConfig {
            mock_subprocess: true,
            ..EsbuildSubprocessConfig::default()
        });
        // component_name="default" (export-side), marker_name="SidebarToggle"
        // (the identifier the SSR side and data-zfb-island attribute use).
        let islands = vec![Island::with_marker_name(
            "default",
            "/abs/SidebarToggle.tsx",
            "SidebarToggle",
        )];
        let config = BundleConfig::default()
            .with_outdir(dir.path().to_path_buf())
            .with_base_url("/");
        let out = bundler
            .bundle_per_island(&islands, FrameworkKind::Preact, &config)
            .expect("bundle_per_island with marker_name succeeds");

        assert_eq!(out.islands.len(), 1);
        // The runtime file is produced by render_runtime_entry_source which
        // serialises the manifest. In mock mode the bundler echoes the entry
        // source back, so the file contents contain the raw manifest JS.
        let runtime_src = std::fs::read_to_string(&out.runtime_asset_path).unwrap();
        assert!(
            runtime_src.contains(r#""SidebarToggle":"#),
            "manifest key must be marker_name 'SidebarToggle': {runtime_src}"
        );
        assert!(
            !runtime_src.contains(r#""default":"#),
            "manifest key must NOT be component_name 'default': {runtime_src}"
        );
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
        let src = render_shared_bundle_entry_source(FrameworkKind::Preact, &islands, false);
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
        let src = render_shared_bundle_entry_source(FrameworkKind::Preact, &islands, false);
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
        let src = render_shared_bundle_entry_source(FrameworkKind::Preact, &[], false);
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
        let without = render_shared_bundle_entry_source(FrameworkKind::Preact, &islands, false);
        let with = render_shared_bundle_entry_source(FrameworkKind::Preact, &islands, true);

        assert!(
            !without.contains("@takazudo/zfb-runtime/client-router"),
            "client_router=false must not mention the client-router subpath: {without}"
        );

        let header = "// Generated by zfb-islands::EsbuildSubprocessBundler::produce_bundle_js\n";
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
        // (no preact import, no mountIslands call) so the entry stays
        // minimal.
        let src = render_shared_bundle_entry_source(FrameworkKind::Preact, &[], true);
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
        let src = render_shared_bundle_entry_source(FrameworkKind::Preact, &islands, true);
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
        let src = render_shared_bundle_entry_source(FrameworkKind::Preact, &islands, false);
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
        let src = render_shared_bundle_entry_source(FrameworkKind::Preact, &islands, false);

        assert!(
            src.contains(r#"import { mountIslands } from "@takazudo/zfb/runtime""#),
            "missing mountIslands import: {src}"
        );
        assert!(
            src.contains(r#"import { h, hydrate, render } from "preact""#),
            "missing preact glue imports: {src}"
        );

        // Helper functions present, plus an `__zfb_register` call per
        // island. Issue #149: the manifest key (third arg) is now a
        // **static literal** — the scanner-derived `marker_name`. No
        // runtime `displayName ?? name` introspection: that path was
        // broken by esbuild minification (function names become single
        // letters in production bundles).
        assert!(src.contains("function __zfb_pick("));
        assert!(src.contains("function __zfb_register("));
        assert!(
            !src.contains("function __zfb_keyFor("),
            "issue #149: runtime introspection helper must be gone:\n{src}"
        );
        assert!(src.contains("__zfb_register(__zfb_island_0, \"Counter\", \"Counter\","));
        assert!(src.contains("__zfb_register(__zfb_island_1, \"Modal\", \"Modal\","));

        // hydrate vs render branching mirrors render_island_entry_source.
        assert!(src.contains(r#"if (mode === "hydrate") { hydrate(v, element); }"#));
        assert!(src.contains("else { render(v, element); }"));

        // Shared-bundle manifest entries must include an unmount thunk (Preact-only path).
        assert!(
            src.contains("unmount: (element) => { render(null, element); }"),
            "expected unmount thunk in shared-bundle manifest entry: {src}"
        );

        // Final invocation hands the populated manifest to the runtime.
        assert!(src.contains("mountIslands(__zfb_manifest);"));
    }

    #[test]
    fn render_shared_bundle_entry_source_react_uses_client_apis() {
        // React-mode shared bundle: a project with `framework: "react"`
        // must emit React hydration glue (createElement + react-dom/client
        // hydrateRoot/createRoot + a module-scope WeakMap of roots) instead
        // of the Preact `h()/hydrate()/render()` shape — mirroring the
        // per-island React branch in `render_island_entry_source`. Without
        // this, a React project's shared bundle ships bare `from "preact"`
        // imports that crash at hydrate time.
        let islands = vec![
            Island::new("Counter", "/abs/components/Counter.tsx"),
            Island::new("Modal", "/abs/components/Modal.tsx"),
        ];
        let src = render_shared_bundle_entry_source(FrameworkKind::React, &islands, false);

        // React client imports present; no bare Preact import.
        assert!(
            src.contains(r#"import { createElement } from "react""#),
            "missing react createElement import: {src}"
        );
        assert!(
            src.contains(r#"import { hydrateRoot, createRoot } from "react-dom/client""#),
            "missing react-dom/client import: {src}"
        );
        assert!(
            !src.contains(r#"from "preact""#),
            "react-mode bundle must NOT contain a bare preact import: {src}"
        );

        // Module-scope roots map (declared once, not per-register).
        assert_eq!(
            src.matches("const __zfb_roots = new WeakMap();").count(),
            1,
            "expected exactly one module-scope WeakMap of roots: {src}"
        );

        // The shared helper shape is unchanged; only the thunk bodies differ.
        assert!(src.contains("function __zfb_pick("));
        assert!(src.contains("function __zfb_register("));
        assert!(src.contains("__zfb_register(__zfb_island_0, \"Counter\", \"Counter\","));
        assert!(src.contains("__zfb_register(__zfb_island_1, \"Modal\", \"Modal\","));

        // React mount glue: createElement + hydrateRoot/createRoot, and an
        // unmount thunk that disposes the stored root.
        assert!(
            src.contains("const v = createElement(C, props);"),
            "missing React createElement mount: {src}"
        );
        assert!(
            src.contains("const root = hydrateRoot(element, v);"),
            "missing hydrateRoot call: {src}"
        );
        assert!(
            src.contains("const root = createRoot(element);"),
            "missing createRoot call: {src}"
        );
        assert!(
            src.contains("root.unmount(); __zfb_roots.delete(element);"),
            "missing React unmount glue: {src}"
        );
        // No Preact `h()` / `render()` thunk leakage into the React path.
        assert!(
            !src.contains("const v = h(C, props);"),
            "react-mode bundle must NOT contain the Preact h() thunk: {src}"
        );

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
        let src = render_shared_bundle_entry_source(FrameworkKind::Preact, &islands, false);

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
        let src = render_shared_bundle_entry_source(FrameworkKind::Preact, &islands, false);

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
    /// [`EsbuildSubprocessBundler::bundle_one_entry`] so esbuild always
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
        let bundler = EsbuildSubprocessBundler::new(EsbuildSubprocessConfig {
            mock_subprocess: true,
            // Empty mock_output → bundle_one_entry returns the entry
            // source verbatim. That lets us assert against the
            // synthesized shape end-to-end.
            ..EsbuildSubprocessConfig::default()
        });
        let islands = vec![
            Island::new("Counter", "/abs/components/Counter.tsx"),
            Island::new("Modal", "/abs/components/Modal.tsx"),
            Island::new("Sidebar", "/abs/components/Sidebar.tsx"),
        ];
        let cfg = BundleConfig::default().with_outdir(dir.path().to_path_buf());

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

    #[test]
    fn esbuild_subprocess_config_env_override_is_honoured() {
        // Drive the env-override path through an injected getter rather
        // than mutating the real process environment. `std::env::set_var`
        // is `unsafe` under Rust 2024 because it races other threads
        // reading the env table — and our test suite is multi-threaded.
        let cfg = EsbuildSubprocessConfig::default_with_env_getter(|name| {
            if name == "ZFB_ESBUILD_BIN" {
                Some(OsString::from("/tmp/zfb-esbuild-overridden"))
            } else {
                None
            }
        });
        assert_eq!(
            cfg.binary_path,
            PathBuf::from("/tmp/zfb-esbuild-overridden")
        );
    }

    #[test]
    fn esbuild_subprocess_config_falls_back_to_default_slot_without_env() {
        let cfg = EsbuildSubprocessConfig::default_with_env_getter(|_| None);
        assert_eq!(
            cfg.binary_path,
            PathBuf::from("crates/zfb/binaries/esbuild/esbuild")
        );
    }

    /// Helper: collect the args produced by `build_esbuild_args` as
    /// plain `String`s so test assertions can use `contains` / equality
    /// without juggling `OsString` conversions. `splitting` selects the
    /// shared-bundle path (`true`) vs the per-island/runtime path (`false`).
    fn args_as_strings(config: &BundleConfig, splitting: bool) -> Vec<String> {
        let entry = PathBuf::from("/tmp/entry.tsx");
        let out_dir = PathBuf::from("/tmp/zfb-out");
        build_esbuild_args(config, &[], &out_dir, &entry, splitting)
            .into_iter()
            .map(|os| os.to_string_lossy().into_owned())
            .collect()
    }

    /// Code-splitting contract (#806): the shared-bundle esbuild invocation
    /// must enable `--splitting=true`, write to a directory via `--outdir`
    /// (not `--outfile`), and pin the stable entry name + self-hashed flat
    /// chunk-name template. These flags are what let a consumer's dynamic
    /// `import()` land in its own chunk instead of inlining into the
    /// multi-MB entry every page loads.
    #[test]
    fn build_esbuild_args_enables_splitting_with_outdir_and_naming() {
        let cfg = BundleConfig::default();
        let args = args_as_strings(&cfg, true);

        assert!(
            args.iter().any(|a| a == "--splitting=true"),
            "missing --splitting=true in args: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a == "--splitting=false"),
            "stale --splitting=false still present: {args:?}"
        );
        assert!(
            args.iter().any(|a| a.starts_with("--outdir=")),
            "missing --outdir= in args: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a.starts_with("--outfile=")),
            "stale --outfile= still present: {args:?}"
        );
        assert!(
            args.iter().any(|a| a == "--entry-names=islands"),
            "missing --entry-names=islands in args: {args:?}"
        );
        assert!(
            args.iter()
                .any(|a| a == "--chunk-names=islands-chunk-[hash]"),
            "missing --chunk-names=islands-chunk-[hash] in args: {args:?}"
        );
    }

    /// Per-island/runtime path contract (review-fix, codex finding 3):
    /// `splitting = false` must NOT emit any `--splitting` flag, so esbuild
    /// inlines dynamic imports into the single entry (pre-#806 behaviour).
    /// `--chunk-names` is meaningless without splitting and must be absent,
    /// while `--outdir` + `--entry-names` stay so the read-back still finds
    /// the stable `islands.js` entry.
    #[test]
    fn build_esbuild_args_per_island_path_disables_splitting() {
        let cfg = BundleConfig::default();
        let args = args_as_strings(&cfg, false);

        assert!(
            !args.iter().any(|a| a.starts_with("--splitting")),
            "per-island path must not pass any --splitting flag: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a.starts_with("--chunk-names")),
            "per-island path must not pass --chunk-names (no chunks): {args:?}"
        );
        // The stable-entry read-back contract still holds without splitting.
        assert!(
            args.iter().any(|a| a.starts_with("--outdir=")),
            "missing --outdir= in args: {args:?}"
        );
        assert!(
            args.iter().any(|a| a == "--entry-names=islands"),
            "missing --entry-names=islands in args: {args:?}"
        );
    }

    /// `validate_chunk_filename` accepts a well-formed self-hashed chunk
    /// name and rejects pathological ones (path separators, traversal,
    /// unexpected prefix) so the read-back can never smuggle an arbitrary
    /// file into `BundleOutput.chunks`.
    #[test]
    fn validate_chunk_filename_accepts_and_rejects() {
        assert!(validate_chunk_filename("islands-chunk-WOEGGERP.js").is_ok());

        assert!(validate_chunk_filename("../islands-chunk-X.js").is_err());
        assert!(validate_chunk_filename("nested/islands-chunk-X.js").is_err());
        assert!(validate_chunk_filename("islands-chunk-..").is_err());
        assert!(validate_chunk_filename("evil.js").is_err());
        assert!(validate_chunk_filename("islands.js").is_err());
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
    /// `--jsx=automatic` AND `--jsx-import-source=preact` (the default
    /// framework). Without those two flags esbuild's classic JSX
    /// transform emits bare `React.createElement` references that
    /// throw `ReferenceError: React is not defined` at mount time when
    /// host components have been migrated to `preact/compat` for
    /// hooks.
    #[test]
    fn build_esbuild_args_includes_automatic_jsx_flags_for_preact_default() {
        let cfg = BundleConfig::default();
        let args = args_as_strings(&cfg, true);
        assert!(
            args.iter().any(|a| a == "--jsx=automatic"),
            "missing --jsx=automatic in args: {args:?}"
        );
        assert!(
            args.iter().any(|a| a == "--jsx-import-source=preact"),
            "missing --jsx-import-source=preact in args: {args:?}"
        );
        // Both flags must appear before the entry path (which is
        // always last) so esbuild parses them as flags rather than as
        // additional inputs.
        let entry_idx = args
            .iter()
            .position(|a| a.ends_with("entry.tsx"))
            .expect("entry path present");
        let jsx_idx = args
            .iter()
            .position(|a| a == "--jsx=automatic")
            .expect("--jsx=automatic present");
        let import_idx = args
            .iter()
            .position(|a| a == "--jsx-import-source=preact")
            .expect("--jsx-import-source=preact present");
        assert!(jsx_idx < entry_idx);
        assert!(import_idx < entry_idx);
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
    /// Production mode (`config.minify == true`) → `PROD=true`,
    /// `DEV=false`. The values are JS literal booleans, not quoted
    /// strings, matching the form already used by
    /// `crates/zfb-build/src/bundler.rs::2395`.
    #[test]
    fn build_esbuild_args_defines_import_meta_env_in_prod() {
        let cfg = BundleConfig::default().with_minify(true);
        let args = args_as_strings(&cfg, true);
        assert!(
            args.iter()
                .any(|a| a == "--define:import.meta.env.PROD=true"),
            "missing --define:import.meta.env.PROD=true in args: {args:?}"
        );
        assert!(
            args.iter()
                .any(|a| a == "--define:import.meta.env.DEV=false"),
            "missing --define:import.meta.env.DEV=false in args: {args:?}"
        );
    }

    /// Dev mode (`config.minify == false`) flips both flags so the
    /// `if (import.meta.env.DEV) …` branch is preserved in unminified
    /// builds. Mirrors the `bundler.rs` semantics so consumers see the
    /// same `PROD`/`DEV` substitution in both pipelines.
    #[test]
    fn build_esbuild_args_defines_import_meta_env_in_dev() {
        let cfg = BundleConfig::default();
        assert!(!cfg.minify, "default BundleConfig must be dev mode");
        let args = args_as_strings(&cfg, true);
        assert!(
            args.iter()
                .any(|a| a == "--define:import.meta.env.PROD=false"),
            "missing --define:import.meta.env.PROD=false in args: {args:?}"
        );
        assert!(
            args.iter()
                .any(|a| a == "--define:import.meta.env.DEV=true"),
            "missing --define:import.meta.env.DEV=true in args: {args:?}"
        );
    }

    /// `BundleConfig::jsx_import_source` is honoured verbatim — the
    /// helper does not hardcode `"preact"`, so callers that bundle for
    /// React (or any future adapter) get the right
    /// `--jsx-import-source=<value>` flag.
    #[test]
    fn build_esbuild_args_honours_custom_jsx_import_source() {
        let cfg = BundleConfig::default().with_jsx_import_source("react");
        let args = args_as_strings(&cfg, true);
        assert!(
            args.iter().any(|a| a == "--jsx=automatic"),
            "missing --jsx=automatic in args: {args:?}"
        );
        assert!(
            args.iter().any(|a| a == "--jsx-import-source=react"),
            "missing --jsx-import-source=react in args: {args:?}"
        );
        // The Preact default must NOT leak when the caller overrode it.
        assert!(
            !args.iter().any(|a| a == "--jsx-import-source=preact"),
            "stale --jsx-import-source=preact present: {args:?}"
        );
    }

    /// Regression for issue #633.
    ///
    /// next.18 dist modules carry an explicit framework-neutral
    /// `import { jsx } from "react/jsx-runtime"` (e.g.
    /// `@takazudo/zfb-runtime/client-router`, pulled into the islands shared
    /// bundle when `clientRouter: true`). In a Preact project `react` is not
    /// installed, so the islands esbuild must rewrite `react/jsx-runtime` (and
    /// the dev-runtime sibling) to the Preact runtime — mirroring the main SSR
    /// bundler (`crates/zfb-build/src/bundler.rs`, `Framework::Preact` arm). For
    /// the Preact default both aliases MUST be present.
    #[test]
    fn build_esbuild_args_aliases_react_jsx_runtime_for_preact() {
        let cfg = BundleConfig::default();
        assert_eq!(cfg.jsx_import_source, "preact", "default must be Preact");
        let args = args_as_strings(&cfg, true);
        assert!(
            args.iter()
                .any(|a| a == "--alias:react/jsx-runtime=preact/jsx-runtime"),
            "missing --alias:react/jsx-runtime=preact/jsx-runtime in args: {args:?}"
        );
        assert!(
            args.iter()
                .any(|a| a == "--alias:react/jsx-dev-runtime=preact/jsx-dev-runtime"),
            "missing --alias:react/jsx-dev-runtime=preact/jsx-dev-runtime in args: {args:?}"
        );
    }

    /// Companion to the #633 regression: React projects resolve
    /// `react/jsx-runtime` natively, so the alias is Preact-only and the React
    /// argv MUST NOT carry either `react/jsx-runtime` alias (same Preact-only
    /// gate as the SSR bundler — keeps the React bundle byte-stable).
    #[test]
    fn build_esbuild_args_omits_react_jsx_runtime_alias_for_react() {
        let cfg = BundleConfig::default().with_jsx_import_source("react");
        let args = args_as_strings(&cfg, true);
        assert!(
            !args
                .iter()
                .any(|a| a == "--alias:react/jsx-runtime=preact/jsx-runtime"),
            "react path must not alias react/jsx-runtime: {args:?}"
        );
        assert!(
            !args
                .iter()
                .any(|a| a == "--alias:react/jsx-dev-runtime=preact/jsx-dev-runtime"),
            "react path must not alias react/jsx-dev-runtime: {args:?}"
        );
    }

    // -----------------------------------------------------------------------
    // #261 — Islands esbuild resolver: alias + virtual-module tests
    // -----------------------------------------------------------------------

    /// Zero alias/virtual-module registrations → config fields are empty by
    /// default and no `--alias` flags appear in the subprocess args.
    /// This is the regression guard: "bundling without registrations is
    /// byte-identical to today's bundle output."
    #[test]
    fn zero_registrations_produce_no_alias_flags_in_config() {
        let cfg = EsbuildSubprocessConfig::default();
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
        let cfg = EsbuildSubprocessConfig::default().with_alias_entries(aliases.clone());
        assert_eq!(cfg.alias_entries, aliases);
    }

    /// `with_virtual_modules` stores the pairs and overwrites the previous list.
    #[test]
    fn with_virtual_modules_stores_pairs() {
        let vms = vec![(
            "virtual:my-data".to_string(),
            "export const x = 1;".to_string(),
        )];
        let cfg = EsbuildSubprocessConfig::default().with_virtual_modules(vms.clone());
        assert_eq!(cfg.virtual_modules, vms);
    }

    /// Bundling in mock mode with alias entries included — the mock path must
    /// still succeed (alias args are a real-subprocess-only concern; mock mode
    /// returns the configured output unchanged).
    #[test]
    fn mock_mode_succeeds_with_alias_entries() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = EsbuildSubprocessConfig {
            mock_subprocess: true,
            mock_output: "// alias mock output".to_string(),
            alias_entries: vec![("@/foo".to_string(), "/abs/src/foo.tsx".to_string())],
            ..EsbuildSubprocessConfig::default()
        };
        let bundler = EsbuildSubprocessBundler::new(cfg);
        let islands = vec![Island::new("Counter", "/abs/components/Counter.tsx")];
        let bundle_cfg = BundleConfig::default().with_outdir(dir.path().to_path_buf());
        let out = bundler.bundle(&islands, &bundle_cfg).unwrap();
        // Mock output is returned in-memory — no disk write.
        assert_eq!(out.bytes, b"// alias mock output");
    }

    /// Bundling in mock mode with virtual modules — the mock path must succeed.
    #[test]
    fn mock_mode_succeeds_with_virtual_modules() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = EsbuildSubprocessConfig {
            mock_subprocess: true,
            mock_output: "// virtual mock output".to_string(),
            virtual_modules: vec![(
                "virtual:config".to_string(),
                "export const site = 'zfb';".to_string(),
            )],
            ..EsbuildSubprocessConfig::default()
        };
        let bundler = EsbuildSubprocessBundler::new(cfg);
        let islands = vec![Island::new("Counter", "/abs/components/Counter.tsx")];
        let bundle_cfg = BundleConfig::default().with_outdir(dir.path().to_path_buf());
        let out = bundler.bundle(&islands, &bundle_cfg).unwrap();
        // Mock output is returned in-memory — no disk write.
        assert_eq!(out.bytes, b"// virtual mock output");
    }

    /// Verify that the `with_alias_entries` + `with_virtual_modules` builders
    /// can be chained together and that both field lists are populated.
    #[test]
    fn builder_methods_are_chainable() {
        let cfg = EsbuildSubprocessConfig::default()
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
        let cfg = EsbuildSubprocessConfig::default()
            .with_virtual_modules(vec![("virtual:data".to_string(), source.to_string())]);
        let retrieved = &cfg.virtual_modules[0].1;
        assert_eq!(
            retrieved.as_str(),
            source,
            "source must survive config round-trip unchanged"
        );
    }
}
