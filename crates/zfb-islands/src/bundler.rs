//! The "client bundler" abstraction.
//!
//! esbuild is delivered as a Go-built standalone CLI. To keep our build
//! pipeline portable we shell out to that binary today — but the long-term
//! plan is to swap in a Rust-native implementation (e.g. swc-bundler,
//! turbopack-style, or our own equivalent) without touching the rest of the
//! pipeline.
//!
//! That swap is exactly what [`ClientBundler`] models: take a set of island
//! entry points (collected by Sub 1's AST scanner), return a single hashed
//! JS asset that the hydration runtime (Sub 3) can `import()` from.
//! Everything stage-2 and beyond (hashing, asset emission, URL derivation)
//! is engine-agnostic.
//!
//! ## Coordination with Sub 1
//!
//! The [`Island`] / [`BundleConfig`] / [`BundleOutput`] / [`ModuleId`] /
//! [`ClientBundler`] types and the [`bundle_link_href`] helper are drafted
//! here so Sub 2 can be exercised in isolation. Sub 1 (topic-ast-scanner)
//! owns the canonical scanner population of [`Island`]; the trait shape
//! here mirrors what Sub 1 published so the team-lead's merge is a no-op
//! aside from de-duplication.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};

/// String alias for an exported module id inside the bundled JS asset.
/// One per island; the order matches the order of the islands slice that
/// produced the bundle.
pub type ModuleId = String;

/// A single island: one component flagged with `"use client"` that the
/// scanner found in the project sources. Bundler entry points are derived
/// from `source_path`; `component_name` is recorded in the bundle's
/// `module_ids` list so the hydration runtime can correlate a DOM marker
/// to its hydratable export.
///
/// Sub 1 owns the canonical population of this type. Default exports
/// surface as the literal string `"default"` for `component_name`.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Island {
    /// The exported component name as it appears in the source file.
    pub component_name: String,

    /// Absolute or workspace-relative path to the TS/TSX source file that
    /// contains the component. Used as the esbuild entry point.
    pub source_path: PathBuf,
}

/// Engine-agnostic bundling configuration.
///
/// Engine-specific knobs (binary path, extra CLI args) live on a sibling
/// per-engine config struct — see [`EsbuildSubprocessConfig`].
#[derive(Debug, Clone)]
pub struct BundleConfig {
    /// Production minification: maps to esbuild's `--minify`.
    pub minify: bool,

    /// Source maps: maps to esbuild's `--sourcemap=linked`. Recommended on
    /// in dev builds, off in production.
    pub sourcemap: bool,

    /// Where to write the bundle asset. The pipeline emits
    /// `{outdir}/assets/islands-{hash}.js` under this directory.
    /// Default: `dist/`.
    pub outdir: PathBuf,

    /// Public base URL prefix for the asset, e.g. `"/"` (default) or
    /// `"https://cdn.example.com/"`. Used by [`bundle_link_href`].
    pub base_url: String,
}

impl Default for BundleConfig {
    /// Dev-equivalent default: minify off, sourcemap on, outdir `dist/`,
    /// base_url `/`.
    fn default() -> Self {
        Self {
            minify: false,
            sourcemap: true,
            outdir: PathBuf::from("dist"),
            base_url: "/".to_string(),
        }
    }
}

impl BundleConfig {
    /// Production preset: minify on, sourcemap off.
    pub fn production() -> Self {
        Self {
            minify: true,
            sourcemap: false,
            ..Self::default()
        }
    }

    /// Development preset: same as [`Self::default`].
    pub fn dev() -> Self {
        Self::default()
    }

    /// Override the output root directory (chainable).
    pub fn with_outdir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.outdir = dir.into();
        self
    }

    /// Override the public base URL prefix (chainable).
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Override the minify flag (chainable).
    pub fn with_minify(mut self, on: bool) -> Self {
        self.minify = on;
        self
    }

    /// Override the sourcemap flag (chainable).
    pub fn with_sourcemap(mut self, on: bool) -> Self {
        self.sourcemap = on;
        self
    }
}

/// Result of running [`ClientBundler::bundle`].
///
/// `asset_path` and `asset_url` are derived from the same hash; the
/// renderer (Sub 3) typically uses `asset_url` directly for the
/// `<script type="module" src="...">` tag.
#[derive(Debug, Clone)]
pub struct BundleOutput {
    /// Absolute or `outdir`-relative path the asset was written to.
    /// Convention: `{outdir}/assets/islands-{hash}.js`.
    pub asset_path: PathBuf,

    /// Public URL for the asset, derived via [`bundle_link_href`].
    /// Convention: `{base_url}/assets/islands-{hash}.js` (slash-normalised).
    pub asset_url: String,

    /// Per-island module ids exported by the bundle, in the same order as
    /// the islands slice passed to [`ClientBundler::bundle`].
    /// Hydration glue uses this to look up the hydratable export for each
    /// island's DOM marker.
    pub module_ids: Vec<ModuleId>,
}

/// Abstraction over "bundle these island entry points into a single hashed
/// JS asset".
///
/// ## Why a trait?
///
/// We currently shell out to the official esbuild CLI binary
/// ([`EsbuildSubprocessBundler`]). When a Rust-native option becomes viable
/// (see [`crate::future_rust_native::NativeRustBundler`]), we want to swap
/// it in at one site without rewriting hashing, asset emission, or
/// hydration glue.
///
/// ## Contract
///
/// - The bundler MUST return a [`BundleOutput`] whose `asset_path` is
///   present on disk by the time `bundle` returns.
/// - The bundler MUST be deterministic for a given (islands, config) pair
///   — the same inputs must produce byte-identical bundle bytes (and thus
///   an identical 8-char hash).
/// - The bundler MAY widen the bundle's contents beyond the listed entry
///   points (transitive imports are expected). It MUST NOT silently drop
///   listed entry points.
/// - The bundler SHOULD emit ESM with code-splitting disabled in v0 so the
///   hydration runtime only ever resolves a single asset URL.
pub trait ClientBundler {
    /// Bundle the given island entry points.
    fn bundle(&self, islands: &[Island], config: &BundleConfig) -> Result<BundleOutput>;
}

/// Configuration for [`EsbuildSubprocessBundler`].
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
    /// verbatim.
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
        let env_override = std::env::var_os("ZFB_ESBUILD_BIN");
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
/// written to a temp file (`--outfile=`), read back, hashed, and written
/// to its final location at `{outdir}/assets/islands-{hash}.js`.
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
/// assert!(out.asset_url.starts_with("/assets/islands-"));
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
    fn produce_bundle_js(&self, islands: &[Island], config: &BundleConfig) -> Result<String> {
        if self.config.mock_subprocess {
            return Ok(self.config.mock_output.clone());
        }

        if !self.config.binary_path.exists() {
            return Err(anyhow!(
                "esbuild binary not found at {}; \
                 set ZFB_ESBUILD_BIN or update EsbuildSubprocessConfig::binary_path",
                self.config.binary_path.display()
            ));
        }

        // Allocate a temp file in this engine's working dir so esbuild's
        // `--outfile` can be a relative path resolvable from the subprocess
        // cwd. Drop happens when we return — file vanishes after read.
        let tmp = tempfile::Builder::new()
            .prefix("zfb-esbuild-")
            .suffix(".js")
            .tempfile()
            .context("failed to allocate temp file for esbuild output")?;

        let mut cmd = Command::new(&self.config.binary_path);
        cmd.current_dir(&self.config.working_dir);
        cmd.arg("--bundle");
        cmd.arg("--format=esm");
        cmd.arg("--splitting=false");
        // Tree-shaking is on by default for ESM in esbuild; we set the
        // flag explicitly so the contract is visible at the call site.
        cmd.arg("--tree-shaking=true");
        if config.minify {
            cmd.arg("--minify");
        }
        if config.sourcemap {
            cmd.arg("--sourcemap=linked");
        }
        cmd.arg(format!("--outfile={}", tmp.path().display()));
        for extra in &self.config.extra_args {
            cmd.arg(extra);
        }
        for island in islands {
            cmd.arg(&island.source_path);
        }

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

        let js = std::fs::read_to_string(tmp.path())
            .context("failed to read esbuild output file")?;
        Ok(js)
    }
}

impl ClientBundler for EsbuildSubprocessBundler {
    fn bundle(&self, islands: &[Island], config: &BundleConfig) -> Result<BundleOutput> {
        let js = self.produce_bundle_js(islands, config)?;

        let hash = hash_8(&js);
        let asset_path = config
            .outdir
            .join("assets")
            .join(format!("islands-{hash}.js"));

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
        let module_ids: Vec<ModuleId> =
            islands.iter().map(|i| i.component_name.clone()).collect();

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

/// Build the public URL for the islands JS asset.
///
/// `base_url` is concatenated with the asset path's filename portion under
/// an `assets/` segment. `base_url` is normalised to drop any trailing `/`.
/// Exact mirror of `zfb_css::link_href`.
///
/// Examples:
/// ```
/// use std::path::PathBuf;
/// use zfb_islands::bundle_link_href;
///
/// let p = PathBuf::from("dist/assets/islands-abc12345.js");
/// assert_eq!(bundle_link_href("/", &p), "/assets/islands-abc12345.js");
/// assert_eq!(
///     bundle_link_href("https://cdn.example.com", &p),
///     "https://cdn.example.com/assets/islands-abc12345.js",
/// );
/// ```
pub fn bundle_link_href(base_url: &str, asset_path: &Path) -> String {
    let filename = asset_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let trimmed = base_url.trim_end_matches('/');
    format!("{trimmed}/assets/{filename}")
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
    fn bundle_link_href_normalises_trailing_slash() {
        let p = PathBuf::from("dist/assets/islands-deadbeef.js");
        assert_eq!(bundle_link_href("/", &p), "/assets/islands-deadbeef.js");
        assert_eq!(bundle_link_href("", &p), "/assets/islands-deadbeef.js");
        assert_eq!(
            bundle_link_href("https://cdn.example.com/", &p),
            "https://cdn.example.com/assets/islands-deadbeef.js"
        );
        assert_eq!(
            bundle_link_href("https://cdn.example.com", &p),
            "https://cdn.example.com/assets/islands-deadbeef.js"
        );
    }

    #[test]
    fn bundle_config_dev_default_matches_default() {
        let dev = BundleConfig::dev();
        let default = BundleConfig::default();
        assert_eq!(dev.minify, default.minify);
        assert_eq!(dev.sourcemap, default.sourcemap);
        assert_eq!(dev.outdir, default.outdir);
        assert_eq!(dev.base_url, default.base_url);
    }

    #[test]
    fn bundle_config_production_flips_minify_and_sourcemap() {
        let prod = BundleConfig::production();
        assert!(prod.minify);
        assert!(!prod.sourcemap);
    }

    #[test]
    fn bundle_config_chainable_setters() {
        let cfg = BundleConfig::default()
            .with_outdir("custom-dist")
            .with_base_url("https://cdn.example.com/")
            .with_minify(true)
            .with_sourcemap(false);
        assert_eq!(cfg.outdir, PathBuf::from("custom-dist"));
        assert_eq!(cfg.base_url, "https://cdn.example.com/");
        assert!(cfg.minify);
        assert!(!cfg.sourcemap);
    }

    #[test]
    fn esbuild_subprocess_config_env_override_is_honoured() {
        // Snapshot, mutate, restore.
        let prev = std::env::var_os("ZFB_ESBUILD_BIN");
        std::env::set_var("ZFB_ESBUILD_BIN", "/tmp/zfb-esbuild-overridden");
        let cfg = EsbuildSubprocessConfig::default();
        assert_eq!(
            cfg.binary_path,
            PathBuf::from("/tmp/zfb-esbuild-overridden")
        );
        match prev {
            Some(v) => std::env::set_var("ZFB_ESBUILD_BIN", v),
            None => std::env::remove_var("ZFB_ESBUILD_BIN"),
        }
    }
}
