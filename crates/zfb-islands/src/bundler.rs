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
//!   must produce the same asset path and module-id list. Downstream
//!   hashing assumes byte-stable bundle output.
//! - The output asset filename convention is `islands-{hash}.js` under
//!   `{outdir}/assets/` — mirror of `zfb_css`'s `styles-{hash}.css` layout.
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

    /// Output root. The asset lands at
    /// `{outdir}/assets/islands-{hash}.js`.
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
    /// Output file path on disk, e.g. `dist/assets/islands-abc12345.js`.
    pub asset_path: PathBuf,

    /// Public URL the renderer should reference, e.g.
    /// `/assets/islands-abc12345.js`. Computed via [`bundle_link_href`]
    /// from the asset path and the `base_url` in [`BundleConfig`].
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
}
