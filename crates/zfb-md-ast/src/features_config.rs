//! Per-feature markdown pipeline configuration.
//!
//! Lives in `zfb-md-ast` (rather than `zfb`) so the visitor-contract crate
//! and the downstream plugin crate (`zfb-md-extras`) can share a single
//! canonical definition without a cycle through `zfb`. `zfb` re-exports
//! these types from its `config` module for backwards compatibility with
//! existing `zfb::config::{MarkdownFeaturesConfig, FeatureToggle, ...}`
//! import paths.
//!
//! Mirrored 1:1 by `MarkdownFeaturesConfig` and `FeatureToggle` in
//! `packages/zfb/src/config.ts` for the TypeScript loader.

use serde::{Deserialize, Serialize};

/// Per-feature toggle: `bool` shorthand or a feature-options object.
///
/// `true` enables the feature with defaults; `false` (or absent) disables
/// it. The object form carries per-feature options (fields vary by feature
/// and are filled in by each feature's port sub-issue — stubs today).
///
/// `serde(untagged)` with the `Options` variant listed FIRST ensures serde
/// tries the object form before the bool, mirroring the `GfmFlag` pattern
/// used elsewhere in `zfb::config`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum FeatureToggle {
    /// Per-feature options object (fields are feature-specific).
    Options(FeatureOptions),
    /// Shorthand: `true` = enabled with defaults, `false` = disabled.
    Bool(bool),
}

/// Empty options object for features that accept `{ ... }` but have no
/// user-facing knobs yet. Fields are filled in by each feature's port
/// sub-issue; this stub satisfies the schema shape requirement.
///
/// `deny_unknown_fields` is critical here — without it, the `FeatureToggle`
/// untagged enum would accept ANY object as `Options(FeatureOptions {})`,
/// silently turning `features.githubAlerts = { bogus: true }` into
/// "enabled with defaults" instead of failing fast.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FeatureOptions {}

/// Options for the `githubAutolinks` feature. Requires `repo`.
///
/// TODO: fill in actual fields when the githubAutolinks feature is ported.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GithubAutolinksConfig {
    /// GitHub repository reference (`owner/repo`) used to build autolink URLs.
    // Required by the githubAutolinks feature spec; value is set by users.
    #[serde(default)]
    pub repo: Option<String>,
}

/// Options for the `codeEnrichment` feature.
///
/// Both flags default to `true` (ON) when the feature is enabled with the
/// shorthand `codeEnrichment: {}` or when a field is absent.
///
/// Mirrors `CodeEnrichmentConfig` in `packages/zfb/src/config.ts`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodeEnrichmentConfig {
    /// Enable diff-marker processing: lines containing `// [!code ++]` or
    /// `// [!code --]` (and language-appropriate equivalents) gain
    /// `data-line-diff="added"` / `data-line-diff="removed"` on their
    /// `<span class="line">` element; the marker comment is stripped from
    /// the visible content. Defaults to `true` when absent.
    #[serde(default)]
    pub diff_markers: Option<bool>,
    /// Enable line-highlight processing: the fence info-string may carry a
    /// range like `{1,3-5}` after the language identifier; matching lines
    /// gain `data-line-highlight="true"` on their `<span class="line">`.
    /// Defaults to `true` when absent.
    #[serde(default)]
    pub line_highlight: Option<bool>,
}

/// Options for the `tocExport` feature.
///
/// Controls which headings are included in the exported `toc` JSON.
///
/// **Depth semantics:** `max_depth` is the **absolute heading depth** (2–6),
/// not a count from h2. `max_depth: 2` → h2 only; `max_depth: 3` (default)
/// → h2 + h3. This differs from [`TocConfig::max_depth`] (heading_marker_toc),
/// which counts levels *starting from h2*. The two features are independent
/// and can be enabled simultaneously without conflict.
///
/// Ported in Wave 5 (#578). TS mirror: `TocExportConfig` in
/// `packages/zfb/src/config.ts`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TocExportConfig {
    /// Maximum heading depth to include in the export (absolute, 2–6).
    /// Default: 3 (includes h2 and h3).
    #[serde(default = "default_toc_export_max_depth")]
    pub max_depth: u8,
}

fn default_toc_export_max_depth() -> u8 {
    3
}

impl Default for TocExportConfig {
    fn default() -> Self {
        Self { max_depth: 3 }
    }
}

/// Options stub for the `imageDimensions` feature.
///
/// TODO: fill in actual fields when the imageDimensions feature is ported.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageDimensionsConfig {}

/// Options stub for the `linkValidation` feature.
///
/// TODO: fill in actual fields when the linkValidation feature is ported.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkValidationConfig {}

/// Options for the `transclude` feature.
///
/// Enables `:::include{file="./path.md"}` directives that inline another
/// file's parsed mdast at the include site.
///
/// Ported in Wave 6 (#581).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranscludeConfig {
    /// Maximum transclusion depth (chain length A→B→C→…).
    ///
    /// A depth of `1` allows only direct includes (the included file itself
    /// cannot include further files). Default: `5`. A cycle (A→B→A) is
    /// always detected regardless of `max_depth` and treated as an error.
    #[serde(default = "default_transclude_max_depth")]
    pub max_depth: u8,
}

fn default_transclude_max_depth() -> u8 {
    5
}

impl Default for TranscludeConfig {
    fn default() -> Self {
        Self { max_depth: 5 }
    }
}

/// Configuration for the table-of-contents visitor.
///
/// Lives in `zfb-md-ast` so the `MarkdownFeaturesConfig::heading_marker_toc`
/// field can carry the rich shape without a dep cycle. The TOC visitor
/// (`zfb_content::plugins::TocPlugin`) reads this type via the `zfb_md_ast`
/// import path; `zfb-content` re-exports it under the historical
/// `zfb_content::TocConfig` path for backwards compatibility.
///
/// `deny_unknown_fields` is critical when this struct sits inside
/// [`HeadingMarkerTocFeature::Config`] (an untagged enum) — without it a
/// config like `headingMarkerToc: { bogus: true }` would silently fall
/// back to defaults instead of surfacing the typo.
///
/// Mirrors `TocConfig` in `packages/zfb/src/config.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TocConfig {
    /// Heading text that triggers TOC insertion. Matched
    /// case-insensitively after whitespace trimming. Default: `"TOC"`.
    #[serde(default = "default_toc_heading")]
    pub heading: String,
    /// Number of heading levels to include starting from `<h2>`. `1` →
    /// h2 only, `2` (default) → h2 + h3, `3` → h2–h4, …, max 5 (h2–h6).
    #[serde(default = "default_toc_max_depth")]
    pub max_depth: u8,
}

fn default_toc_heading() -> String {
    "TOC".to_string()
}

fn default_toc_max_depth() -> u8 {
    2
}

impl Default for TocConfig {
    fn default() -> Self {
        Self {
            heading: default_toc_heading(),
            max_depth: default_toc_max_depth(),
        }
    }
}

/// `headingMarkerToc` feature value: either a `bool` shorthand
/// (`true` = enable with [`TocConfig::default`], `false` = disable) or a
/// full [`TocConfig`] options object.
///
/// `serde(untagged)` with `Config` listed first ensures serde tries the
/// object form before the bool — same pattern as [`FeatureToggle`].
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum HeadingMarkerTocFeature {
    /// Full options object — TOC heading text + max depth.
    Config(TocConfig),
    /// Shorthand: `true` = enabled with defaults, `false` = disabled.
    Bool(bool),
}

impl HeadingMarkerTocFeature {
    /// True when the feature should be wired into the pipeline. `Bool(false)`
    /// is treated as disabled even though it deserialises to `Some(...)`.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        match self {
            HeadingMarkerTocFeature::Bool(b) => *b,
            HeadingMarkerTocFeature::Config(_) => true,
        }
    }

    /// Resolve to a concrete [`TocConfig`]. `Bool(true)` and `Bool(false)`
    /// both fall back to `TocConfig::default()`; the caller must gate on
    /// [`Self::is_enabled`] first to know whether to wire the visitor.
    #[must_use]
    pub fn to_config(&self) -> TocConfig {
        match self {
            HeadingMarkerTocFeature::Config(cfg) => cfg.clone(),
            HeadingMarkerTocFeature::Bool(_) => TocConfig::default(),
        }
    }
}

impl FeatureToggle {
    /// True when the feature should be wired into the pipeline.
    /// `Bool(false)` is treated as disabled even though it deserialises
    /// to `Some(...)`. `Options(_)` is always enabled (the user explicitly
    /// supplied an options object).
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        match self {
            FeatureToggle::Bool(b) => *b,
            FeatureToggle::Options(_) => true,
        }
    }
}

/// Helper for the gating pattern in `Pipeline::with_defaults_and_features`:
/// `feature_enabled(&cfg.mermaid)` returns true iff the user explicitly
/// turned the feature on. `None` or `Some(Bool(false))` both return false.
#[must_use]
pub fn feature_enabled(toggle: &Option<FeatureToggle>) -> bool {
    toggle.as_ref().is_some_and(FeatureToggle::is_enabled)
}

/// Same as [`feature_enabled`] but for the heading-marker-TOC feature, which
/// accepts the rich `TocConfig` shape via [`HeadingMarkerTocFeature`].
#[must_use]
pub fn heading_marker_toc_enabled(toggle: &Option<HeadingMarkerTocFeature>) -> bool {
    toggle.as_ref().is_some_and(HeadingMarkerTocFeature::is_enabled)
}

/// Per-feature markdown pipeline configuration.
///
/// Each field corresponds to one pluggable markdown feature. All fields are
/// optional; absent = feature disabled, behaviour unchanged from the
/// pre-features build. Unknown keys are rejected at deserialization time
/// (via `#[serde(deny_unknown_fields)]`) so a typo in `zfb.config.ts`
/// surfaces as a clear error naming the unknown field rather than silently
/// passing through with the feature disabled.
///
/// Mirrors `MarkdownFeaturesConfig` in `packages/zfb/src/config.ts`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MarkdownFeaturesConfig {
    /// GitHub-style alert blocks (`> [!NOTE]`, `> [!WARNING]`, etc.).
    #[serde(default)]
    pub github_alerts: Option<FeatureToggle>,

    /// Reading-time estimate injected into the document frontmatter.
    #[serde(default)]
    pub reading_time: Option<FeatureToggle>,

    /// GitHub-style `owner/repo#123` and `SHA` autolinks. Requires `repo`.
    #[serde(default)]
    pub github_autolinks: Option<GithubAutolinksConfig>,

    /// Code-block enrichment (copy button, language label, etc.).
    #[serde(default)]
    pub code_enrichment: Option<CodeEnrichmentConfig>,

    /// Grouped code blocks rendered as tabs.
    #[serde(default)]
    pub code_tabs: Option<FeatureToggle>,

    /// Ruby annotation support (`{base}^{ruby}` syntax).
    #[serde(default)]
    pub ruby: Option<FeatureToggle>,

    /// Export the page TOC as structured data (e.g. for sidebar rendering).
    #[serde(default)]
    pub toc_export: Option<TocExportConfig>,

    /// Auto-detect and inject `width`/`height` on `<img>` elements.
    #[serde(default)]
    pub image_dimensions: Option<ImageDimensionsConfig>,

    /// Validate internal and external links at build time.
    #[serde(default)]
    pub link_validation: Option<LinkValidationConfig>,

    /// Transclusion of other MDX files (`![[path]]` syntax).
    #[serde(default)]
    pub transclude: Option<TranscludeConfig>,

    /// Framework admonitions preset (maps `:::note` etc. to components).
    #[serde(default)]
    pub admonitions_preset: Option<FeatureToggle>,

    /// Mermaid diagram rendering.
    #[serde(default)]
    pub mermaid: Option<FeatureToggle>,

    /// Lightbox / image-enlarge on click.
    #[serde(default)]
    pub image_enlarge: Option<FeatureToggle>,

    /// Inline heading-marker TOC. Accepts either a `bool` shorthand
    /// (`true` = enable with defaults, `false` = disable) or a full
    /// [`TocConfig`] options object — see [`HeadingMarkerTocFeature`].
    #[serde(default)]
    pub heading_marker_toc: Option<HeadingMarkerTocFeature>,
}
