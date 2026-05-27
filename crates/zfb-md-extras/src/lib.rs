//! `zfb-md-extras` — remark/rehype plugin ports (GFM, footnotes, etc.).
//!
//! This crate skeleton was bootstrapped by Wave 1 (#567). The actual
//! markdown pipeline and visitor traits land in Wave 2 (#569).
//!
//! # Crate boundary
//!
//! Dep direction: `zfb-content → zfb-md-extras → zfb-md-ast`.
//! This crate MUST NOT depend on `zfb-content` (would create a cycle).
//!
//! The single entry point from `zfb-content` into this crate is
//! `register_features`, which lives in `zfb-content::pipeline` (NOT here)
//! because it takes a `&mut Pipeline` — a type defined in `zfb-content`.
//! Wave 4-6 will add visitor builders in each feature module below;
//! `register_features` will call into them to wire the visitors.

// MarkdownFeaturesConfig + per-feature option structs + TocConfig live in
// zfb-md-ast. Re-export them here so callers of zfb-md-extras can find them
// in the natural location (this is the "features" crate, after all) without
// reaching past it for the type.
pub use zfb_md_ast::{
    feature_enabled, heading_marker_toc_enabled, CodeEnrichmentConfig, FeatureOptions,
    FeatureToggle, GithubAutolinksConfig, HeadingMarkerTocFeature, ImageDimensionsConfig,
    LinkValidationConfig, MarkdownFeaturesConfig, TocConfig, TocExportConfig, TranscludeConfig,
};

/// Test harness module — `run_fixture` and helpers for fixture-based
/// snapshot tests. Gated behind `cfg(any(test, feature = "test-utils"))` so
/// it is never compiled into production builds.
///
/// Wave 2+ feature crates enable this via:
///   `zfb-md-extras = { path = "...", features = ["test-utils"] }`
#[cfg(any(test, feature = "test-utils"))]
pub mod test_harness;

// ── Wave 3 feature modules (moved from zfb-content in #570) ────────────────

/// Admonitions preset — the six built-in directive definitions (`note`,
/// `tip`, `warning`, `danger`, `info`, `details`).
/// Wire via `features.admonitionsPreset = true` in `zfb.config.ts`.
pub mod admonitions_preset;

/// Heading-marker TOC visitor. Inserts a `<ul>/<li>` TOC after the
/// configured anchor heading. Wire via `features.headingMarkerToc`.
pub mod heading_marker_toc;

/// Image-enlarge visitor. Wraps `<p><img></p>` in
/// `<figure class="zd-enlargeable">`. Wire via `features.imageEnlarge = true`.
pub mod image_enlarge;

/// Mermaid visitor. Replaces `<pre><code class="language-mermaid">` with
/// `<div class="mermaid">`. Wire via `features.mermaid = true`.
pub mod mermaid;

// ── Feature stub modules (Wave 4-6) ────────────────────────────────────────
//
// One module per planned feature. Each is empty in Wave 3 — Wave 4-6
// will port the corresponding remark/rehype plugin into each stub.

/// GitHub-style alert blocks (`> [!NOTE]`, `> [!WARNING]`, etc.).
/// Wire via `features.githubAlerts: true` in `zfb.config.ts`.
pub mod github_alerts;

/// Reading-time estimate injected into document frontmatter.
// TODO: Wave 4 — port reading-time
pub mod reading_time {}

/// GitHub-style `owner/repo#123` and SHA autolinks.
/// Wire via `features.githubAutolinks: { repo: "owner/repo" }` in `zfb.config.ts`.
pub mod github_autolinks;

/// Code-block enrichment (copy button, language label, etc.).
// TODO: Wave 5 — port code-enrichment
pub mod code_enrichment {}

/// Grouped code blocks rendered as tabs.
// TODO: Wave 5 — port code-tabs
pub mod code_tabs {}

/// Ruby annotation support (`{base}^{ruby}` syntax).
// TODO: Wave 5 — port ruby
pub mod ruby {}

/// Export the page TOC as structured data (e.g. for sidebar rendering).
// TODO: Wave 6 — port toc-export
pub mod toc_export {}

/// Auto-detect and inject `width`/`height` on `<img>` elements.
// TODO: Wave 6 — port image-dimensions
pub mod image_dimensions {}

/// Validate internal and external links at build time.
// TODO: Wave 6 — port link-validation
pub mod link_validation {}

/// Transclusion of other MDX files (`![[path]]` syntax).
// TODO: Wave 6 — port transclude
pub mod transclude {}

// Feature config types live in zfb-md-ast and are re-exported at the top
// of this file. The canonical definitions use the rich per-feature option
// structs (GithubAutolinksConfig, TocConfig, etc.) so the new
// Pipeline::with_defaults_and_features API and zfb::config share one
// authoritative shape.
