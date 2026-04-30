//! [`ClientBundler`] trait and shared types.
//!
//! Sub 2 of Epic #6 implements the production `EsbuildSubprocessBundler` —
//! today that means shelling out to the esbuild CLI binary. The trait below
//! is the seam between scanning ([`crate::scanner`]) and bundling: anything
//! handed an islands set and a [`BundleConfig`] can produce a
//! `dist/assets/islands-{hash}.js` asset.
//!
//! ## Why a trait?
//!
//! esbuild is delivered as a Go-built CLI. To keep the build pipeline
//! portable we shell out today, but the long-term plan is to swap in a
//! Rust-native bundler (e.g. `oxc_minifier` on `oxc_resolver`, or
//! `rolldown`, or whichever lands first) without touching the scanner or
//! the render orchestrator. See [`crate::future_rust_native`] for the
//! placeholder bundler that documents the swap path.
//!
//! ## Contract
//!
//! - The bundler MUST be deterministic: the same `(islands, config)` pair
//!   must produce the same asset bytes and module-id list. Downstream
//!   hashing in [`zfb_build::pipeline::prod::ProductionAssetPipeline`]
//!   assumes byte-stable bundle output.
//! - The output asset filename is the **stable** name
//!   `{outdir}/assets/islands.js` (per `zfb_types::STABLE_ISLANDS_FILENAME`)
//!   — `ProductionAssetPipeline` is the single source of truth for
//!   content hashing, so emitters under this crate no longer bake
//!   `-<hash>` into the on-disk name. Mirror of `zfb-css`'s
//!   `styles.css` stable-name layout.
//! - `module_ids` returns the bundled module identifiers (typically the
//!   `component_name` of each entry; bundlers MAY widen the list to also
//!   include shared chunks). Order MUST be stable across runs.

use std::path::{Path, PathBuf};

use anyhow::Result;

/// Stable identifier of a bundled module emitted by [`ClientBundler::bundle`].
///
/// Today this is just the component name; reserved as a type alias so future
/// bundlers can carry richer info (chunk index, original specifier, …)
/// without breaking call sites.
pub type ModuleId = String;

/// One `"use client"` component selected for hydration.
///
/// Two islands are equal iff their `(source_path, component_name)` pair is
/// equal — this is the stable component-name identity the scanner promises.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Island {
    /// Exported component name from the source file. For default exports
    /// this is the literal string `"default"`.
    pub component_name: String,
    /// Path to the .tsx/.ts/.jsx/.js source that carried the
    /// `"use client"` directive. Whatever form the scanner returned —
    /// typically the resolver's resolved-path representation.
    pub source_path: PathBuf,
}

impl Island {
    /// Construct a new island.
    ///
    /// No path canonicalisation is done here; callers building islands by
    /// hand should pass the same path representation that the bundler's
    /// resolver expects so dedup keys line up with the rest of the
    /// pipeline.
    pub fn new(component_name: impl Into<String>, source_path: impl Into<PathBuf>) -> Self {
        Self {
            component_name: component_name.into(),
            source_path: source_path.into(),
        }
    }
}

/// Bundle configuration handed to [`ClientBundler::bundle`].
#[derive(Debug, Clone)]
pub struct BundleConfig {
    /// Minify the output. Production: `true`. Dev: `false`.
    pub minify: bool,

    /// Emit a sourcemap alongside the asset (and, for esbuild, a
    /// `//# sourceMappingURL=` comment). Production: `false`. Dev: `true`.
    pub sourcemap: bool,

    /// Output root. The asset lands at the stable path
    /// `{outdir}/assets/islands.js`. (Hashing is the
    /// `ProductionAssetPipeline`'s job — see crate-level contract.)
    /// Default: `dist/`.
    pub outdir: PathBuf,

    /// Public base URL prefix used by [`bundle_link_href`].
    /// Default: `"/"`.
    pub base_url: String,
}

impl Default for BundleConfig {
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

    /// Dev preset: minify off, sourcemap on. Same as [`Default`].
    pub fn dev() -> Self {
        Self::default()
    }

    /// Override the output directory (chainable).
    pub fn with_outdir(mut self, outdir: impl Into<PathBuf>) -> Self {
        self.outdir = outdir.into();
        self
    }

    /// Override the public base URL (chainable).
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Toggle minification (chainable).
    pub fn with_minify(mut self, minify: bool) -> Self {
        self.minify = minify;
        self
    }

    /// Toggle sourcemap emission (chainable).
    pub fn with_sourcemap(mut self, sourcemap: bool) -> Self {
        self.sourcemap = sourcemap;
        self
    }
}

/// Result of a successful [`ClientBundler::bundle`] call.
#[derive(Debug, Clone)]
pub struct BundleOutput {
    /// Output file path on disk — the stable form
    /// `dist/assets/islands.js`. `ProductionAssetPipeline` reads these
    /// bytes and renames to `dist/assets/islands-<hash>.js` at deploy
    /// time; the name handed to callers here stays unhashed so the
    /// pipeline can drive HTML rewrites against
    /// `STABLE_ISLANDS_URL` without coordinating an extra string
    /// channel.
    pub asset_path: PathBuf,

    /// Public URL the renderer should reference — the stable form
    /// `/assets/islands.js` (see `zfb_types::STABLE_ISLANDS_URL`).
    /// Computed via [`bundle_link_href`] from the asset path and the
    /// `base_url` in [`BundleConfig`].
    pub asset_url: String,

    /// Identifiers of the modules included in the bundle. Order is stable
    /// across runs for a given input.
    pub module_ids: Vec<ModuleId>,
}

/// Abstraction over "bundle this islands set into a single browser-ready
/// JS asset".
///
/// See the module-level docs for the swap-in story behind the trait.
pub trait ClientBundler {
    /// Bundle `islands` according to `config`. Must be deterministic for a
    /// given `(islands, config)` pair.
    fn bundle(&self, islands: &[Island], config: &BundleConfig) -> Result<BundleOutput>;
}

/// One per-island bundle output, produced by
/// [`crate::EsbuildSubprocessBundler::bundle_per_island`].
///
/// Per-island bundles land at the stable path
/// `{outdir}/islands/{component}.js` so the runtime can dynamic-import
/// each island's JS independently. Sharing is the bundler's concern —
/// at this layer we just record one entry per island. The
/// content-hash field is still computed and exposed (see
/// [`IslandBundle::hash`]) for dev-mode change detection and for
/// downstream consumers that wrap this output through
/// `ProductionAssetPipeline`, but it is **not** baked into the
/// filename or URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IslandBundle {
    /// Component export name. Mirrors [`Island::component_name`] of the
    /// input island so callers can pair entries by name.
    pub component_name: String,
    /// Output file path on disk — the stable form
    /// `dist/islands/{component}.js`.
    pub asset_path: PathBuf,
    /// Public URL the runtime should `import()` from — the stable form
    /// `/islands/{component}.js`.
    pub asset_url: String,
    /// 8-char content hash (lowercase hex) of the bundled JS. Reported
    /// for dev-mode change detection and for downstream consumers
    /// that delegate hashing to `ProductionAssetPipeline`. The hash is
    /// **not** part of the on-disk filename or URL — those are
    /// stable-named per the S0 single-source-of-truth-for-hashing
    /// contract.
    pub hash: String,
}

/// Result of a successful per-island bundle pass.
///
/// In addition to the per-island bundles, the per-island pipeline emits
/// a small **runtime** bundle — the framework-agnostic shim that walks
/// `[data-zfb-island]` / `[data-zfb-island-skip-ssr]` elements in the
/// DOM, dynamic-imports the matching per-island bundle, and dispatches
/// to `hydrate` / `render`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerIslandBundleOutput {
    /// One entry per island, in the same order the input slice provided.
    pub islands: Vec<IslandBundle>,
    /// Runtime bundle file path — the stable form
    /// `dist/islands/islands-runtime.js`. Hashing, when needed, is
    /// applied later by `ProductionAssetPipeline`.
    pub runtime_asset_path: PathBuf,
    /// Runtime bundle public URL — the `<script type="module" src="…">`
    /// the page-router HTML pass injects into `<head>`. Stable form:
    /// `/islands/islands-runtime.js`.
    pub runtime_asset_url: String,
}

/// Which JS framework the islands pipeline should target.
///
/// This is intentionally a small enum local to `zfb-islands` so the
/// crate stays free of a `zfb-render` dependency (mirroring the
/// `Adapter` contract from there). The orchestrator wires the two
/// together at the seam where it constructs the bundler.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FrameworkKind {
    /// Preact — bare `preact` + `preact/jsx-runtime`.
    #[default]
    Preact,
    /// React 18+ — `react` + `react-dom/client`.
    React,
}

impl FrameworkKind {
    /// Stable lowercase name. Mirrors `zfb_render::Adapter::name()`.
    pub fn name(self) -> &'static str {
        match self {
            FrameworkKind::Preact => "preact",
            FrameworkKind::React => "react",
        }
    }
}

/// Build the public URL for a per-island JS asset.
///
/// Mirrors [`bundle_link_href`] but lives under `/islands/` instead of
/// `/assets/` so per-island and shared bundles can share an outdir
/// without colliding. With S0's stable-naming contract the typical
/// inputs look like `dist/islands/Counter.js` →
/// `/islands/Counter.js`; `ProductionAssetPipeline` is the only
/// component allowed to substitute hashed forms.
pub fn island_link_href(base_url: &str, asset_path: &Path) -> String {
    let filename = asset_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let trimmed = base_url.trim_end_matches('/');
    format!("{trimmed}/islands/{filename}")
}

/// Build the public URL for the islands JS asset.
///
/// Mirrors `zfb_css::link_href` exactly — we don't depend on `zfb-css`,
/// but the behaviour is intentionally identical so the renderer can reach
/// for either helper interchangeably.
///
/// Examples:
/// ```
/// use std::path::PathBuf;
/// use zfb_islands::bundle_link_href;
///
/// let p = PathBuf::from("dist/assets/islands.js");
/// assert_eq!(bundle_link_href("/", &p), "/assets/islands.js");
/// assert_eq!(
///     bundle_link_href("https://cdn.example.com", &p),
///     "https://cdn.example.com/assets/islands.js",
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
    fn island_equality_uses_path_and_name() {
        let a = Island::new("Counter", "/abs/components/Counter.tsx");
        let b = Island::new("Counter", "/abs/components/Counter.tsx");
        let c = Island::new("Counter", "/abs/components/Other.tsx");
        let d = Island::new("Other", "/abs/components/Counter.tsx");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
    }

    #[test]
    fn bundle_config_presets() {
        let prod = BundleConfig::production();
        assert!(prod.minify);
        assert!(!prod.sourcemap);

        let dev = BundleConfig::dev();
        assert!(!dev.minify);
        assert!(dev.sourcemap);
    }

    #[test]
    fn bundle_config_chainable() {
        let cfg = BundleConfig::production()
            .with_outdir("build")
            .with_base_url("https://cdn.example.com/")
            .with_minify(false)
            .with_sourcemap(true);
        assert_eq!(cfg.outdir, PathBuf::from("build"));
        assert_eq!(cfg.base_url, "https://cdn.example.com/");
        assert!(!cfg.minify);
        assert!(cfg.sourcemap);
    }

    #[test]
    fn link_href_normalises_trailing_slash() {
        // S0 contract: emitter writes the stable filename
        // `assets/islands.js`. Hashed forms are produced by
        // `ProductionAssetPipeline`, not by the bundler.
        let p = PathBuf::from("dist/assets/islands.js");
        assert_eq!(bundle_link_href("/", &p), "/assets/islands.js");
        assert_eq!(bundle_link_href("", &p), "/assets/islands.js");
        assert_eq!(
            bundle_link_href("https://cdn.example.com/", &p),
            "https://cdn.example.com/assets/islands.js"
        );
        assert_eq!(
            bundle_link_href("https://cdn.example.com", &p),
            "https://cdn.example.com/assets/islands.js"
        );
    }
}
