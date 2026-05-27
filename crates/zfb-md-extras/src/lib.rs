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

use serde::{Deserialize, Serialize};

/// Test harness module — `run_fixture` and helpers for fixture-based
/// snapshot tests. Gated behind `cfg(any(test, feature = "test-utils"))` so
/// it is never compiled into production builds.
///
/// Wave 2+ feature crates enable this via:
///   `zfb-md-extras = { path = "...", features = ["test-utils"] }`
#[cfg(any(test, feature = "test-utils"))]
pub mod test_harness;

// ── Feature stub modules ────────────────────────────────────────────────────
//
// One module per planned feature. Each is empty in Wave 2 — Wave 4-6
// will port the corresponding remark/rehype plugin into each stub.

/// GitHub-style alert blocks (`> [!NOTE]`, `> [!WARNING]`, etc.).
// TODO: Wave 4 — port github-alerts
pub mod github_alerts {}

/// Reading-time estimate injected into document frontmatter.
// TODO: Wave 4 — port reading-time
pub mod reading_time {}

/// GitHub-style `owner/repo#123` and SHA autolinks.
// TODO: Wave 4 — port github-autolinks
pub mod github_autolinks {}

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

// ── Feature config types ────────────────────────────────────────────────────
//
// Defined here (not in `zfb`) so that `zfb-content` can accept them without
// an upward dependency on `zfb`. Mirrors `MarkdownFeaturesConfig` and
// `FeatureToggle` in `packages/zfb/src/config.ts` and `crates/zfb/src/config.rs`.
//
// The `zfb` crate keeps its own copy of these types for config-file
// deserialization; a conversion step (added in Wave 3.1) bridges between
// the two so existing config paths are unaffected.

/// Per-feature toggle: `bool` shorthand or a feature-options object.
///
/// `true` enables the feature with defaults; `false` (or absent) disables it.
/// The object form carries per-feature options (fields filled in by each
/// feature's port sub-issue — stubs today).
///
/// `serde(untagged)` with `Options` listed first mirrors the `GfmFlag` pattern
/// in `zfb::config`: serde tries the object form before the bool.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum FeatureToggle {
    /// Per-feature options object.
    Options(FeatureOptions),
    /// Shorthand: `true` = enabled with defaults, `false` = disabled.
    Bool(bool),
}

/// Empty options object for features that accept `{ ... }` but have no
/// user-facing knobs yet.
///
/// `deny_unknown_fields` prevents silently accepting arbitrary objects as
/// "enabled with defaults" when a user typos a config key.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FeatureOptions {}

/// Per-feature markdown pipeline configuration.
///
/// Each field corresponds to one pluggable markdown feature. All fields are
/// optional; absent = feature disabled, behaviour unchanged from the
/// pre-features build.
///
/// Defined in `zfb-md-extras` (not in `zfb`) so that `zfb-content` can
/// consume it without a dependency cycle. `zfb` carries a parallel copy for
/// config-file deserialization; a conversion step bridges the two in Wave 3.1.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MarkdownFeaturesConfig {
    /// GitHub-style alert blocks (`> [!NOTE]`, `> [!WARNING]`, etc.).
    #[serde(default)]
    pub github_alerts: Option<FeatureToggle>,

    /// Reading-time estimate injected into the document frontmatter.
    #[serde(default)]
    pub reading_time: Option<FeatureToggle>,

    /// GitHub-style `owner/repo#123` and SHA autolinks. Requires `repo`.
    #[serde(default)]
    pub github_autolinks: Option<FeatureToggle>,

    /// Code-block enrichment (copy button, language label, etc.).
    #[serde(default)]
    pub code_enrichment: Option<FeatureToggle>,

    /// Grouped code blocks rendered as tabs.
    #[serde(default)]
    pub code_tabs: Option<FeatureToggle>,

    /// Ruby annotation support (`{base}^{ruby}` syntax).
    #[serde(default)]
    pub ruby: Option<FeatureToggle>,

    /// Export the page TOC as structured data (e.g. for sidebar rendering).
    #[serde(default)]
    pub toc_export: Option<FeatureToggle>,

    /// Auto-detect and inject `width`/`height` on `<img>` elements.
    #[serde(default)]
    pub image_dimensions: Option<FeatureToggle>,

    /// Validate internal and external links at build time.
    #[serde(default)]
    pub link_validation: Option<FeatureToggle>,

    /// Transclusion of other MDX files (`![[path]]` syntax).
    #[serde(default)]
    pub transclude: Option<FeatureToggle>,

    /// Framework admonitions preset (maps `:::note` etc. to components).
    #[serde(default)]
    pub admonitions_preset: Option<FeatureToggle>,

    /// Mermaid diagram rendering.
    #[serde(default)]
    pub mermaid: Option<FeatureToggle>,

    /// Lightbox / image-enlarge on click.
    #[serde(default)]
    pub image_enlarge: Option<FeatureToggle>,

    /// Inline heading-marker TOC.
    #[serde(default)]
    pub heading_marker_toc: Option<FeatureToggle>,
}
