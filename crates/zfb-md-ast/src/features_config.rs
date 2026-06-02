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

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::directives::{DirectiveDef, DirectiveKind};

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

/// Options for the `imageDimensions` feature.
///
/// Injecting `width`/`height` on `<img>` elements referencing local image
/// files via `BuildContext.public_dir`. Ported in Wave 6 (#579).
///
/// Mirrors `ImageDimensionsConfig` in `packages/zfb/src/config.ts`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageDimensionsConfig {
    /// When `true` (the default), `http://` and `https://` image sources are
    /// silently skipped and not probed for dimensions.
    ///
    /// Set to `false` only for testing or unusual setups — remote images
    /// require network access at build time and slow the pipeline.
    #[serde(default)]
    pub skip_remote: Option<bool>,
}

/// Options for the `linkValidation` feature.
///
/// Validates internal `[text](file.md#anchor)` and `[text](#anchor)` links at
/// build time using the cross-file heading-ID registry built by
/// `HeadingLinksPlugin` (wave 6, #568). External URLs (`http://`, `https://`,
/// `mailto:`, etc.) are skipped by default.
///
/// Mirrors `LinkValidationConfig` in `packages/zfb/src/config.ts`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkValidationConfig {
    /// When `true`, broken links are emitted as `Error` diagnostics instead
    /// of `Warning`, which the orchestrator can use to fail the build. Default:
    /// `false` (warn-only; build continues).
    #[serde(default)]
    pub fail_on_broken: Option<bool>,
    /// When `true` (default), external URLs (`http://`, `https://`, `mailto:`,
    /// etc.) are silently skipped. Set to `false` to validate external links
    /// too (useful in CI link-checking pipelines).
    #[serde(default)]
    pub allow_external: Option<bool>,
}

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

/// Directive kind for user-defined admonition directives.
///
/// Maps to [`crate::directives::DirectiveKind`]. A separate serde DTO is
/// needed because `DirectiveKind` carries no serde derives.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AdmonitionDirectiveKind {
    /// Block-level container directive (default).
    Container,
    /// Self-closing leaf directive.
    Leaf,
    /// Inline text directive.
    Text,
}

impl From<AdmonitionDirectiveKind> for DirectiveKind {
    fn from(k: AdmonitionDirectiveKind) -> Self {
        match k {
            AdmonitionDirectiveKind::Container => DirectiveKind::Container,
            AdmonitionDirectiveKind::Leaf => DirectiveKind::Leaf,
            AdmonitionDirectiveKind::Text => DirectiveKind::Text,
        }
    }
}

/// Full object form for a user-defined admonition directive.
///
/// `deny_unknown_fields` is critical: without it a typo'd field inside the
/// untagged `AdmonitionDirectiveSpec` enum would silently fall back to
/// `Short(String)` instead of surfacing the error.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdmonitionDirectiveFullSpec {
    /// JSX component identifier (e.g. `"Spoiler"`, `"Kbd"`).
    pub component: String,
    /// Container/leaf/text shape. Defaults to `Container` when absent.
    #[serde(default)]
    pub kind: Option<AdmonitionDirectiveKind>,
    /// Whether the bracketed `[label]` is promoted to a `title="…"` attribute.
    /// Defaults to `true` when absent.
    #[serde(default)]
    pub title_from_label: Option<bool>,
}

/// Spec for one user-defined admonition directive: either a bare component
/// name string or a full options object.
///
/// `serde(untagged)` with `Full` listed first ensures serde tries the object
/// form before the string — same ordering discipline as [`FeatureToggle`].
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum AdmonitionDirectiveSpec {
    /// Full options object — kind, title_from_label, component.
    Full(AdmonitionDirectiveFullSpec),
    /// Bare component name shorthand (`"Spoiler"`, `"Kbd"`, etc.).
    Short(String),
}

/// Options object for the `admonitionsPreset` feature.
///
/// `deny_unknown_fields` ensures a typo like `extraDirectivez` is rejected
/// rather than silently activating the feature with default options.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdmonitionsPresetOptions {
    /// Additional `:::name` → component mappings to register. Registered
    /// after the standard set (when `extend_defaults` is true) so last-wins
    /// gives user override on collision.
    #[serde(default)]
    pub extra_directives: Option<HashMap<String, AdmonitionDirectiveSpec>>,
    /// Seed the standard seven directives before `extra_directives`.
    /// Defaults to `true` when absent.
    #[serde(default)]
    pub extend_defaults: Option<bool>,
}

/// `admonitionsPreset` feature value: either a `bool` shorthand or a full
/// [`AdmonitionsPresetOptions`] object.
///
/// `serde(untagged)` with `Options` listed first ensures serde tries the
/// object form before the bool — same pattern as [`HeadingMarkerTocFeature`].
///
/// Prefer `features.directives` for new configs — `admonitionsPreset` is
/// kept for back-compat and continues to work unchanged.
#[deprecated(note = "use features.directives instead; kept for back-compat")]
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum AdmonitionsPresetFeature {
    /// Full options object.
    Options(AdmonitionsPresetOptions),
    /// Shorthand: `true` = enabled with defaults, `false` = disabled.
    Bool(bool),
}

#[allow(deprecated)]
impl AdmonitionsPresetFeature {
    /// True when the feature should be wired into the pipeline. `Bool(false)`
    /// is treated as disabled even though it deserialises to `Some(...)`.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        match self {
            AdmonitionsPresetFeature::Bool(b) => *b,
            AdmonitionsPresetFeature::Options(_) => true,
        }
    }

    /// Resolve to a concrete [`AdmonitionsPresetOptions`]. `Bool(true)` and
    /// `Bool(false)` both fall back to defaults; the caller must gate on
    /// [`Self::is_enabled`] first.
    #[must_use]
    pub fn options(&self) -> AdmonitionsPresetOptions {
        match self {
            AdmonitionsPresetFeature::Options(opts) => opts.clone(),
            AdmonitionsPresetFeature::Bool(_) => AdmonitionsPresetOptions::default(),
        }
    }
}

/// Same as [`feature_enabled`] but for the admonitions-preset feature, which
/// accepts the rich `AdmonitionsPresetOptions` shape via
/// [`AdmonitionsPresetFeature`].
#[must_use]
#[allow(deprecated)]
pub fn admonitions_preset_enabled(toggle: &Option<AdmonitionsPresetFeature>) -> bool {
    toggle.as_ref().is_some_and(AdmonitionsPresetFeature::is_enabled)
}

/// Neutral public alias for [`AdmonitionDirectiveSpec`] — same type, without
/// "admonition" in the name. Use in `features.directives` contexts.
pub type DirectiveSpec = AdmonitionDirectiveSpec;

/// Helper for gating the generic `directives` feature in
/// `Pipeline::register_features`. Returns `true` iff the user supplied a
/// non-`None` map (even an empty one counts as "enabled").
#[must_use]
pub fn directives_enabled(toggle: &Option<HashMap<String, AdmonitionDirectiveSpec>>) -> bool {
    toggle.is_some()
}

/// Convert a user-supplied [`AdmonitionDirectiveSpec`] into a
/// [`DirectiveDef`] ready for registration.
///
/// - `Short(name)` → Container, `title_from_label = true`.
/// - `Full { ... }` → kind defaults to Container, `title_from_label`
///   defaults to `true`. `attrs` is empty in this phase (attrs-via-config
///   is deferred).
#[must_use]
pub fn into_directive_def(name: &str, spec: &AdmonitionDirectiveSpec) -> DirectiveDef {
    match spec {
        AdmonitionDirectiveSpec::Short(component) => DirectiveDef {
            name: name.to_string(),
            kind: DirectiveKind::Container,
            component_name: component.clone(),
            title_from_label: true,
            attrs: Vec::new(),
        },
        AdmonitionDirectiveSpec::Full(full) => DirectiveDef {
            name: name.to_string(),
            kind: full
                .kind
                .clone()
                .map(DirectiveKind::from)
                .unwrap_or(DirectiveKind::Container),
            component_name: full.component.clone(),
            title_from_label: full.title_from_label.unwrap_or(true),
            attrs: Vec::new(),
        },
    }
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
/// **`directives`**: generic `:::name` → `<Component>` map. Keys are
/// directive names; values are [`AdmonitionDirectiveSpec`] (a bare component
/// name string or a full `{ component, kind, titleFromLabel }` object). Zero
/// default directives are registered — only the names you supply are active.
/// Presence of the key (even `{}`) enables the feature. Use this in place of
/// `admonitionsPreset` for new configs.
///
/// Mirrors `MarkdownFeaturesConfig` in `packages/zfb/src/config.ts`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(deprecated)]
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

    /// Ruby annotation support. Primary syntax is caret `{base}^{ruby}`
    /// (e.g. `これは^{これ}`, also accepts a bare base `base^{ruby}`); pipe
    /// `{base|ruby}` is kept as a legacy alias. For the bare caret form the
    /// base must contain a non-ASCII char so ASCII superscripts like `2^{n}`
    /// stay untouched (see `zfb-md-extras/src/ruby.rs` for the pinned grammar).
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
    /// Accepts `true`/`false` or a full [`AdmonitionsPresetOptions`] object —
    /// see [`AdmonitionsPresetFeature`].
    ///
    /// Deprecated in favour of `directives`; kept for back-compat.
    #[serde(default)]
    #[allow(deprecated)]
    pub admonitions_preset: Option<AdmonitionsPresetFeature>,

    /// Generic `:::name` → `<Component>` directive map (zero default names).
    /// Keys are directive names; values are [`AdmonitionDirectiveSpec`].
    /// Presence of the field (even `{}`) enables the directives step.
    #[serde(default)]
    pub directives: Option<HashMap<String, AdmonitionDirectiveSpec>>,

    /// Mermaid diagram rendering.
    #[serde(default)]
    pub mermaid: Option<FeatureToggle>,

    /// Inline heading-marker TOC. Accepts either a `bool` shorthand
    /// (`true` = enable with defaults, `false` = disable) or a full
    /// [`TocConfig`] options object — see [`HeadingMarkerTocFeature`].
    #[serde(default)]
    pub heading_marker_toc: Option<HeadingMarkerTocFeature>,
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;

    // `directives` with a short-form entry (bare component name string).
    #[test]
    fn directives_short_form_round_trip() {
        let json = serde_json::json!({
            "directives": { "foo": "Foo" }
        });
        let cfg: MarkdownFeaturesConfig =
            serde_json::from_value(json).expect("directives short-form deserialises");
        let map = cfg.directives.expect("directives present");
        assert_eq!(
            map.get("foo"),
            Some(&AdmonitionDirectiveSpec::Short("Foo".to_string()))
        );
        // directives_enabled returns true when Some
        assert!(directives_enabled(&Some(map)));
    }

    // `directives` with a full-form entry ({ component, kind, titleFromLabel }).
    #[test]
    fn directives_full_form_round_trip() {
        let json = serde_json::json!({
            "directives": {
                "kbd": { "component": "Kbd", "kind": "text", "titleFromLabel": false }
            }
        });
        let cfg: MarkdownFeaturesConfig =
            serde_json::from_value(json).expect("directives full-form deserialises");
        let map = cfg.directives.expect("directives present");
        assert_eq!(
            map.get("kbd"),
            Some(&AdmonitionDirectiveSpec::Full(AdmonitionDirectiveFullSpec {
                component: "Kbd".to_string(),
                kind: Some(AdmonitionDirectiveKind::Text),
                title_from_label: Some(false),
            }))
        );
    }

    // `directives_enabled` returns false when the field is absent.
    #[test]
    fn directives_enabled_false_when_none() {
        assert!(!directives_enabled(&None));
    }

    // An empty `directives` map still counts as enabled.
    #[test]
    fn directives_enabled_true_for_empty_map() {
        let json = serde_json::json!({ "directives": {} });
        let cfg: MarkdownFeaturesConfig =
            serde_json::from_value(json).expect("empty directives deserialises");
        assert!(directives_enabled(&cfg.directives));
    }
}
