//! Stable public-URL and on-disk-path constants for the production asset
//! graph.
//!
//! ## Rationale: which islands model survives, and where this lives
//!
//! `zfb-islands` historically shipped two competing islands models:
//! a **shared-bundle** model exposed through the public
//! [`ClientBundler::bundle`] trait that emits a single
//! `dist/assets/islands.js` (stable name; previously suffixed with a
//! content hash), and a **per-island** model on the concrete
//! `EsbuildSubprocessBundler` that emits `dist/islands/<Component>.js`
//! plus a `dist/islands/islands-runtime.js` shim. The Prod Asset Graph
//! epic picks the **shared-bundle** model as the prod source of truth:
//! `ProductionAssetPipeline` ships exactly one [`AssetEmitter`] per
//! [`AssetKind`] (one CSS asset, one islands asset), and the
//! shared-bundle output matches that 1:1 contract directly. The
//! per-island path remains in the codebase for now (it has its own
//! tests and a different runtime story), but it is **not** the path
//! `zfb build` will wire through `ProductionAssetPipeline`. Per the
//! single-source-of-truth-for-hashing rule, every emitter under
//! `zfb-islands` writes a stable filename and lets
//! `ProductionAssetPipeline::apply()` perform the hashing + HTML
//! rewrite — without that, S3's emitter wrapper would double-hash.
//!
//! ## Why these constants live in `zfb-types`
//!
//! S1 (renderer head injection) needs to embed the same stable URL
//! that S3 (islands emitter adapter) declares to
//! `ProductionAssetPipeline`. They must agree at compile time, not by
//! string-matching. The natural home for cross-crate shared constants
//! is a low-dependency leaf crate that every consumer can already
//! depend on. `zfb-build` would create a circular import with
//! `zfb-islands` and `zfb-render` (`zfb-build` already depends on
//! `zfb-render`, and `zfb-render` cannot depend on `zfb-build`); the
//! S0 spec explicitly invited a different path "if more sensible," so
//! the constants live here.

/// Public URL the renderer embeds for the global stylesheet.
///
/// Production builds let `ProductionAssetPipeline` rewrite this to
/// `/assets/styles-<hash>.css`. Dev builds keep the stable URL.
pub const STABLE_CSS_URL: &str = "/assets/styles.css";

/// Public URL the renderer embeds for the islands client bundle.
///
/// Production builds let `ProductionAssetPipeline` rewrite this to
/// `/assets/islands-<hash>.js`. Dev builds keep the stable URL.
pub const STABLE_ISLANDS_URL: &str = "/assets/islands.js";

/// Public URL prefix shared by the CSS and islands assets — kept
/// separate from the per-asset URLs so callers building related URLs
/// (manifest entries, secondary chunks) do not have to re-derive it.
pub const STABLE_ASSETS_URL_PREFIX: &str = "/assets/";

/// Directory name (relative to `dist_root`) where hashed asset files
/// are written. Combined with `dist_root` by emitters at construction
/// time. Kept as a bare segment, not a path, so callers can splice it
/// into either OS-native or forward-slash forms without re-parsing.
pub const DIST_ASSETS_DIR: &str = "assets";

/// On-disk filename (relative to `<dist_root>/assets/`) for the global
/// stylesheet before hashing. Mirrors the suffix of [`STABLE_CSS_URL`].
pub const STABLE_CSS_FILENAME: &str = "styles.css";

/// On-disk filename (relative to `<dist_root>/assets/`) for the
/// islands client bundle before hashing. Mirrors the suffix of
/// [`STABLE_ISLANDS_URL`].
pub const STABLE_ISLANDS_FILENAME: &str = "islands.js";

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time-ish guarantee that the URL and filename pairs stay
    /// in sync — drift between them would silently break HTML rewrite.
    #[test]
    fn stable_urls_end_with_their_filenames() {
        assert!(STABLE_CSS_URL.ends_with(STABLE_CSS_FILENAME));
        assert!(STABLE_ISLANDS_URL.ends_with(STABLE_ISLANDS_FILENAME));
    }

    #[test]
    fn stable_urls_share_assets_prefix() {
        assert!(STABLE_CSS_URL.starts_with(STABLE_ASSETS_URL_PREFIX));
        assert!(STABLE_ISLANDS_URL.starts_with(STABLE_ASSETS_URL_PREFIX));
    }

    #[test]
    fn dist_assets_dir_matches_url_prefix() {
        // `/assets/` ↔ `assets`. The constants are cousins, not
        // copies — emitters joining `dist_root` with the dir get the
        // same on-disk layout the renderer references via URL.
        assert_eq!(STABLE_ASSETS_URL_PREFIX, "/assets/");
        assert_eq!(DIST_ASSETS_DIR, "assets");
    }
}
