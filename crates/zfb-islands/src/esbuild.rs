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
//! that locates the binary (defaulting to
//! `crates/zfb/binaries/esbuild`), an implementation that builds the
//! command line, runs it, and writes
//! `{outdir}/assets/islands.js` (the **stable** name —
//! `ProductionAssetPipeline` is the single source of truth for
//! content hashing per the Prod Asset Graph epic).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};

use crate::bundler::{
    bundle_link_href, island_link_href, BundleConfig, BundleOutput, ClientBundler, FrameworkKind,
    Island, IslandBundle, ModuleId, PerIslandBundleOutput,
};

/// The pinned esbuild CLI version this crate runs against.
///
/// At subprocess startup we run `esbuild --version` and abort with a
/// clear error if the reported version does not match. To bump, see
/// the "External tool version pins" section in `CONTRIBUTING.md` at
/// the workspace root.
pub const EXPECTED_ESBUILD_VERSION: &str = "0.25.12";

/// SHA-256 of the pinned esbuild binary, lowercase hex.
///
/// Set to the empty string when release engineering has not yet
/// populated the slot — in that case the checksum verification is
/// skipped (with a clear log line) and only the `--version` check is
/// enforced. Once populated, the checksum verification runs on first
/// subprocess invocation and any mismatch aborts with a clear error
/// pointing at the bump procedure in `CONTRIBUTING.md`.
pub const EXPECTED_ESBUILD_SHA256: &str = "";

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
            "esbuild version mismatch: expected `{}` (pinned in zfb-islands), got `{}` from {}. \
             To resolve, follow the \"External tool version pins\" procedure in CONTRIBUTING.md \
             at the workspace root: bump EXPECTED_ESBUILD_VERSION (and EXPECTED_ESBUILD_SHA256) \
             in crates/zfb-islands/src/esbuild.rs in lock-step with the binary under \
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
                 update both the version and SHA256 constants in \
                 crates/zfb-islands/src/esbuild.rs in lock-step with the new binary.",
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
}

/// The default [`ClientBundler`]: shells out to the esbuild CLI binary.
///
/// The binary is invoked with each island's `source_path` as an entry
/// point, plus `--bundle --format=esm --splitting=false
/// --tree-shaking=true` (esbuild tree-shakes ESM by default but we set the
/// flag explicitly so the contract is visible). `--minify` and
/// `--sourcemap=linked` are appended per [`BundleConfig`]. The payload is
/// written to a temp file (`--outfile=`), read back, and written to its
/// final location at the **stable** path
/// `{outdir}/assets/islands.js`.
/// `ProductionAssetPipeline` performs the content-hash + rename pass
/// at deploy time.
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
/// let islands = vec![Island {
///     component_name: "Counter".into(),
///     source_path: PathBuf::from("components/counter.tsx"),
/// }];
/// let out = bundler.bundle(&islands, &BundleConfig::production()).unwrap();
/// assert_eq!(out.asset_url, "/assets/islands.js");
/// ```
#[derive(Debug, Clone)]
pub struct EsbuildSubprocessBundler {
    config: EsbuildSubprocessConfig,
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
    /// asset-write step.
    ///
    /// Implementation: synthesizes a single-entry source that imports
    /// every island module by absolute path, then routes through
    /// [`Self::bundle_one_entry`] (single input + `--outfile`). This
    /// keeps the on-disk contract for the shared bundle
    /// (`dist/assets/islands.js`) stable while sidestepping esbuild's
    /// "Must use \"outdir\" when there are multiple input files" rule
    /// (issue #138): passing N island `source_path`s as N separate
    /// inputs trips it for any N >= 2.
    fn produce_bundle_js(&self, islands: &[Island], config: &BundleConfig) -> Result<String> {
        let entry_source = render_shared_bundle_entry_source(islands);
        self.bundle_one_entry(&entry_source, config)
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
    /// and ships a manifest of `ComponentName → island-bundle-URL` so
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
    /// preserved verbatim from the input slice and the runtime
    /// manifest is keyed by `component_name` in the same order.
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

        let mut bundle_entries: Vec<IslandBundle> = Vec::with_capacity(islands.len());
        let mut manifest: Vec<(String, String)> = Vec::with_capacity(islands.len());

        for island in islands {
            // 1. Generate the per-island entry source.
            let entry_source = render_island_entry_source(framework, island);

            // 2. Bundle it. In mock mode the entry source itself doubles
            //    as the bundled output so unit tests can assert what we
            //    *would* have shipped without spawning esbuild.
            let bundled_js = self.bundle_one_entry(&entry_source, config)?;

            // Stable filename: `<outdir>/islands/<Component>.js`.
            // `ProductionAssetPipeline` is the only place allowed to
            // emit content-addressed names; this path keeps a stable
            // shape so S0's "no hashed URLs in zfb-islands" rule
            // applies to both the shared-bundle and per-island
            // emitters. The `IslandBundle::hash` field is still
            // populated so dev-mode change detection has something to
            // compare against without touching the on-disk name.
            let hash = hash_8(&bundled_js);
            let asset_path = islands_dir.join(format!("{}.js", island.component_name));
            std::fs::write(&asset_path, bundled_js.as_bytes())
                .with_context(|| format!("failed to write {}", asset_path.display()))?;

            let asset_url = island_link_href(&config.base_url, &asset_path);
            manifest.push((island.component_name.clone(), asset_url.clone()));

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
        let runtime_js = self.bundle_one_entry(&runtime_entry_source, config)?;
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
    fn bundle_one_entry(&self, entry_source: &str, config: &BundleConfig) -> Result<String> {
        if self.config.mock_subprocess {
            // Mock mode: return either the configured mock_output (when
            // set) or the entry source itself so tests can assert the
            // generated entry shape end-to-end.
            if !self.config.mock_output.is_empty() {
                return Ok(self.config.mock_output.clone());
            }
            return Ok(entry_source.to_string());
        }

        ensure_binary_verified(&self.config.binary_path, false)?;

        // Entry temp file (.tsx so esbuild's loader inference picks up
        // JSX automatically — the entry imports component .tsx files).
        let entry_tmp = tempfile::Builder::new()
            .prefix("zfb-esbuild-entry-")
            .suffix(".tsx")
            .tempfile()
            .context("failed to allocate entry temp file")?;
        std::fs::write(entry_tmp.path(), entry_source.as_bytes())
            .context("failed to write entry temp file")?;

        // Output temp file. esbuild writes a sibling `.map` next to it
        // when sourcemap=linked is requested — we read both back.
        let out_tmp = tempfile::Builder::new()
            .prefix("zfb-esbuild-out-")
            .suffix(".js")
            .tempfile()
            .context("failed to allocate out temp file")?;

        let mut cmd = Command::new(&self.config.binary_path);
        cmd.current_dir(&self.config.working_dir);
        cmd.arg("--bundle");
        cmd.arg("--format=esm");
        cmd.arg("--splitting=false");
        cmd.arg("--tree-shaking=true");
        // Per-island bundles ship to the browser. See `produce_bundle_js`
        // for the full rationale; mirrored here so both the legacy
        // shared-bundle and per-island paths stay aligned on platform
        // target + node:* externalization.
        cmd.arg("--platform=browser");
        cmd.arg("--external:node:*");
        if config.minify {
            cmd.arg("--minify");
        }
        if config.sourcemap {
            cmd.arg("--sourcemap=linked");
        }
        cmd.arg(format!("--outfile={}", out_tmp.path().display()));
        // Inline NODE_ENV so React/Preact pick their production build
        // when minifying. esbuild expects the define value to be a JS
        // literal, hence the embedded quotes.
        if config.minify {
            cmd.arg("--define:process.env.NODE_ENV=\"production\"");
        } else {
            cmd.arg("--define:process.env.NODE_ENV=\"development\"");
        }
        for extra in &self.config.extra_args {
            cmd.arg(extra);
        }
        cmd.arg(entry_tmp.path());

        let output = cmd
            .output()
            .with_context(|| format!("failed to spawn {}", self.config.binary_path.display()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "esbuild exited with status {}: {}",
                output.status,
                stderr.trim()
            ));
        }

        let js =
            std::fs::read_to_string(out_tmp.path()).context("failed to read esbuild output")?;
        Ok(js)
    }
}

/// Generate the synthetic single-entry source for the legacy shared
/// bundle.
///
/// The shared bundle's contract is "every island module's source code
/// is present in `dist/assets/islands.js` so the runtime's DOM walk can
/// find each `data-zfb-island="…"` element's component bundled into the
/// page". Producing one synthetic entry sidesteps esbuild's
/// multi-input-with-`--outfile` restriction (issue #138) without
/// changing the public on-disk shape (`dist/assets/islands.js`).
///
/// # Why we call `mountIslands` from the entry
///
/// Bundling every island's source is necessary but not sufficient:
/// without a top-level call into the hydration runtime, the SSR'd
/// `data-zfb-island` markers stay un-hydrated and interactivity never
/// activates (issue #146 / zudolab/zudo-doc#1355 Wave 6). The
/// per-island path emits `mountIslands(MANIFEST)` from
/// `render_runtime_entry_source`; the shared-bundle path mirrors that
/// here so the on-disk contract (`dist/assets/islands.js`) is the only
/// script the page needs to load to get hydration glue **and** the
/// island source code in one HTTP request.
///
/// The synthesised entry imports each island as a namespace and
/// constructs an inline manifest of `componentName → { mount }`
/// descriptors. The runtime's `mountIslands` accepts this object
/// shape directly (no second dynamic import) — see the
/// `IslandManifestValue` widening in `packages/zfb/src/runtime.ts`.
/// Each `mount` is a thin Preact wrapper that mirrors
/// `render_island_entry_source(Preact, …)`: it picks `ns[Name]` then
/// `ns.default`, builds a vnode, and dispatches to `hydrate` /
/// `render` based on the SSR / SSR-skip mode.
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
/// `mountIslands(MANIFEST)` argument — which is itself a top-level
/// side effect esbuild MUST preserve. So tree-shaking remains
/// defanged in the new shape too.
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
/// The synthesised wrapper uses bare `preact` imports — matching the
/// per-island path's `FrameworkKind::Preact` branch in
/// `render_island_entry_source`. Mixed-framework projects are not
/// supported by the production shared-bundle path today (the
/// per-island path is the right home for that, since it composes the
/// framework glue per-island).
pub fn render_shared_bundle_entry_source(islands: &[Island]) -> String {
    let mut out =
        String::from("// Generated by zfb-islands::EsbuildSubprocessBundler::produce_bundle_js\n");
    if islands.is_empty() {
        // Defensive: `build_production_islands_asset` already short-
        // circuits on empty input, so this path is rarely taken; keep
        // the output minimal (header-only) so an unexpected zero-island
        // call still produces valid JS rather than a tail comma in the
        // generated array literal.
        return out;
    }
    out.push_str(r#"import { mountIslands } from "@takazudo/zfb/runtime";"#);
    out.push('\n');
    out.push_str(r#"import { h, hydrate, render } from "preact";"#);
    out.push('\n');
    for (i, island) in islands.iter().enumerate() {
        let path = island.source_path.to_string_lossy();
        out.push_str(&format!(
            "import * as __zfb_island_{i} from {};\n",
            json_string(&path)
        ));
    }
    // Inline manifest. Each entry maps a `data-zfb-island` /
    // `data-zfb-island-skip-ssr` attribute value to an `{ mount }`
    // descriptor. The runtime's `mountIslands` recognises this object
    // shape (vs. the per-island URL string) and calls `mount` directly
    // without a second dynamic import.
    //
    // The mount function picks the named export first, falling back to
    // `default`, mirroring `render_island_entry_source`. This handles
    // both host-shape `export default function ComponentName(...)`
    // islands (where the namespace exposes only `default`) and the
    // `export const Name = ...` / `displayName = ...` patterns used by
    // `@zudo-doc/zudo-doc-v2` (where the named export is present).
    //
    // The argument to `mountIslands` is a top-level side effect esbuild
    // MUST preserve, and references every namespace by identifier —
    // tree-shaking therefore retains every island's exports just as the
    // previous `(globalThis).__zfb_islands ??= [...]` anchor did.
    out.push_str("mountIslands({\n");
    for (i, island) in islands.iter().enumerate() {
        let key = json_string(&island.component_name);
        let name_lit = json_string(&island.component_name);
        out.push_str(&format!(
            "  {key}: {{ mount: (props, element, mode) => {{ \
const C = __zfb_island_{i}[{name_lit}] ?? __zfb_island_{i}.default; \
const v = h(C, props); \
if (mode === \"hydrate\") {{ hydrate(v, element); }} else {{ render(v, element); }} \
}} }},\n"
        ));
    }
    out.push_str("});\n");
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
    match framework {
        FrameworkKind::Preact => format!(
            r#"// Generated by zfb-islands::EsbuildSubprocessBundler::bundle_per_island
import * as Mod from {path_lit};
import {{ h, hydrate, render }} from "preact";
const Component = (Mod as any)[{component_lit}] ?? (Mod as any).default;
export function mount(props, element, mode) {{
  const vnode = h(Component, props);
  if (mode === "hydrate") {{
    hydrate(vnode, element);
  }} else {{
    render(vnode, element);
  }}
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
export function mount(props, element, mode) {{
  const vnode = createElement(Component, props);
  if (mode === "hydrate") {{
    hydrateRoot(element, vnode);
  }} else {{
    createRoot(element).render(vnode);
  }}
}}
export default mount;
"#
        ),
    }
}

/// Generate the runtime entry script from a manifest of
/// `(component_name → asset_url)` pairs.
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

/// Serialize a manifest of `(component_name → asset_url)` pairs to a
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

/// Encode `s` as a JSON string literal (with surrounding double
/// quotes). Used to splice user-supplied paths and component names
/// into generated entry sources without risking syntax errors from
/// stray quotes / backslashes / control chars.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

impl ClientBundler for EsbuildSubprocessBundler {
    fn bundle(&self, islands: &[Island], config: &BundleConfig) -> Result<BundleOutput> {
        let js = self.produce_bundle_js(islands, config)?;

        // Stable filename: `dist/assets/islands.js`. Per the Prod
        // Asset Graph epic, the **single source of truth for content
        // hashing is `ProductionAssetPipeline`** — no emitter under
        // `zfb-islands` may bake a hash into the on-disk filename or
        // public URL anymore. The pipeline reads these stable bytes,
        // computes `sha256(bytes)[..8]`, renames to
        // `assets/islands-<hash>.js`, and rewrites the
        // `STABLE_ISLANDS_URL` references in rendered HTML in one
        // pass. Without this stable-filename contract the pipeline
        // would double-hash (S0 spec, acceptance criterion 3).
        let asset_path = config
            .outdir
            .join(zfb_types::DIST_ASSETS_DIR)
            .join(zfb_types::STABLE_ISLANDS_FILENAME);

        if let Some(parent) = asset_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        std::fs::write(&asset_path, js.as_bytes())
            .with_context(|| format!("failed to write {}", asset_path.display()))?;

        let asset_url = bundle_link_href(&config.base_url, &asset_path);

        // Default `module_ids` mapping: identity per-island. Sub 1's
        // scanner is the source of truth for the canonical mapping (e.g.
        // when the exported name differs from the component name); until
        // it lands a richer Island shape, `component_name` is the right
        // contract for the hydration runtime to assume.
        let module_ids: Vec<ModuleId> = islands.iter().map(|i| i.component_name.clone()).collect();

        Ok(BundleOutput {
            asset_path,
            asset_url,
            module_ids,
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

        for entry in &out.islands {
            // Hash is still computed and exposed (8 lowercase hex
            // chars) for dev-mode change detection, but it is **not**
            // part of the on-disk filename or URL — those are stable
            // per the S0 single-source-of-truth-for-hashing contract.
            assert_eq!(entry.hash.len(), 8);
            assert!(entry.hash.chars().all(|c| c.is_ascii_hexdigit()));
            // File exists on disk at the stable path
            // `<outdir>/islands/<Component>.js`.
            assert!(entry.asset_path.exists(), "{:?}", entry.asset_path);
            assert!(entry.asset_path.starts_with(dir.path().join("islands")));
            let expected_filename = format!("{}.js", entry.component_name);
            assert_eq!(
                entry.asset_path.file_name().unwrap().to_string_lossy(),
                expected_filename,
            );
            // Public URL is the stable `/islands/<Component>.js`.
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
        let src = render_shared_bundle_entry_source(&islands);
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
        // every namespace, otherwise esbuild's tree-shaker would drop
        // the namespace import as unused (the bug #144 fixes) AND the
        // page would never call into the hydration runtime (the bug
        // #146 fixes).
        assert!(
            src.contains(r#"import { mountIslands } from "@takazudo/zfb/runtime""#),
            "missing mountIslands import: {src}"
        );
        assert!(
            src.contains("mountIslands({"),
            "missing mountIslands call: {src}"
        );
        assert!(
            src.contains("__zfb_island_0[\"Counter\"]"),
            "expected Counter mount to reference its namespace: {src}"
        );
        assert!(
            src.contains("__zfb_island_1[\"Modal\"]"),
            "expected Modal mount to reference its namespace: {src}"
        );
    }

    #[test]
    fn render_shared_bundle_entry_source_escapes_quotes_in_paths() {
        // Path containing a literal double-quote (rare but legal on
        // macOS / Linux) must not break the synthesized JS — it gets
        // JSON-escaped just like component names.
        let islands = vec![Island::new("Weird", "/abs/components/has\"quote/Weird.tsx")];
        let src = render_shared_bundle_entry_source(&islands);
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
        let src = render_shared_bundle_entry_source(&[]);
        assert!(!src.contains("import"));
        assert!(!src.contains("mountIslands"));
        assert!(src.starts_with("// Generated"));
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
        let src = render_shared_bundle_entry_source(&islands);
        assert!(src.contains("import * as __zfb_island_0 from "));
        assert!(src.contains("import * as __zfb_island_1 from "));
        assert!(src.contains("import * as __zfb_island_2 from "));
        // Every namespace identifier must appear in the mount manifest
        // so esbuild keeps each module's exports alive.
        assert!(src.contains("__zfb_island_0[\"ThemeToggle\"]"));
        assert!(src.contains("__zfb_island_1[\"ThemeToggle\"]"));
        assert!(src.contains("__zfb_island_2[\"Sidebar\"]"));
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
        let src = render_shared_bundle_entry_source(&islands);

        assert!(
            src.contains(r#"import { mountIslands } from "@takazudo/zfb/runtime""#),
            "missing mountIslands import: {src}"
        );
        assert!(
            src.contains(r#"import { h, hydrate, render } from "preact""#),
            "missing preact glue imports: {src}"
        );

        // Manifest entry per island, mapping component name to an
        // `{ mount }` descriptor that the runtime's mountIslands
        // accepts inline (no second HTTP fetch).
        assert!(src.contains("\"Counter\": { mount:"));
        assert!(src.contains("\"Modal\": { mount:"));

        // Each mount picks the named export first, then default —
        // matching `render_island_entry_source` so host-shape
        // `export default function ComponentName(...)` islands are
        // hydrated correctly.
        assert!(src.contains("__zfb_island_0[\"Counter\"] ?? __zfb_island_0.default"));
        assert!(src.contains("__zfb_island_1[\"Modal\"] ?? __zfb_island_1.default"));

        // hydrate vs render branching mirrors render_island_entry_source.
        assert!(src.contains(r#"if (mode === "hydrate") { hydrate(v, element); }"#));
        assert!(src.contains("else { render(v, element); }"));
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
        // the on-disk shape (the legacy contract S0 / S4 depend on).
        assert_eq!(out.asset_url, "/assets/islands.js");
        assert_eq!(out.asset_path, dir.path().join("assets").join("islands.js"));
        assert!(out.asset_path.exists());

        // Module IDs preserve input order.
        assert_eq!(out.module_ids, vec!["Counter", "Modal", "Sidebar"]);

        // The bytes on disk are the synthesized entry source (echoed by
        // mock mode) — verify each island path appears in it under the
        // namespace-import shape (issue #144 fix). The previous shape
        // (`import "<path>";` side-effect import) tree-shook every
        // island whose body had no top-level effect.
        //
        // The bytes also carry the `mountIslands(...)` invocation that
        // the issue #146 fix added so the SSR'd markers hydrate.
        let on_disk = std::fs::read_to_string(&out.asset_path).expect("read asset");
        assert!(
            on_disk.contains(r#"import * as __zfb_island_0 from "/abs/components/Counter.tsx";"#)
        );
        assert!(on_disk.contains(r#"import * as __zfb_island_1 from "/abs/components/Modal.tsx";"#));
        assert!(
            on_disk.contains(r#"import * as __zfb_island_2 from "/abs/components/Sidebar.tsx";"#)
        );
        assert!(on_disk.contains(r#"import { mountIslands } from "@takazudo/zfb/runtime""#));
        assert!(on_disk.contains("mountIslands({"));
        assert!(on_disk.contains("\"Counter\": { mount:"));
        assert!(on_disk.contains("\"Modal\": { mount:"));
        assert!(on_disk.contains("\"Sidebar\": { mount:"));
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
}
