//! Wasm-safe pipeline facade (zfb#1574 / epic zfb#1572).
//!
//! A future `zfb-md-wasm` crate needs to build a fully-wired [`Pipeline`]
//! and run the mdx→JSX-module emit or the md→HTML render **without any
//! filesystem coupling** — no `zfb.config.ts` evaluation (that needs V8
//! and stays build-side), no `.tmTheme` directory loading, no
//! transclude/image-probe/link-check disk reads. This module is the one
//! entry point that guarantees that shape:
//!
//! 1. [`PipelineOptions`] is a `serde`-`Deserialize` mirror of the knobs
//!    [`Pipeline::with_defaults_and_full_config`] accepts, minus the
//!    filesystem-only ones (`themes_dir`) — see [`build_pipeline_from_json`].
//!    [`PipelineOptions::code_highlight`] additionally routes to
//!    [`Pipeline::with_defaults_and_full_config_class`] when
//!    `codeHighlight.mode` is `"class"` (Highlight Tokens epic zfb#1528,
//!    wasm routing sub zfb#1852) — see [`build_pipeline`].
//! 2. [`render_html`] promotes the `pipeline.run(input)` →
//!    [`crate::serializer::serialize`] composition that today only exists
//!    inlined in test helpers (e.g. `tests/integration_pipeline.rs`) into a
//!    public function.
//! 3. [`render_mdx_jsx_module`] is a thin wrapper over
//!    [`mdx_to_jsx_module_with_pipeline`].
//!
//! ## Why the fs-bound plugins stay inert
//!
//! `transclude`, `imageDimensions`, and `linkValidation` are registered
//! into the pipeline whenever their `features.*` key is present (same as
//! every other production pipeline — see `register_features` in
//! `crate::pipeline`), but they only do filesystem work when driven
//! through [`Pipeline::run_with_context`] with armed
//! [`Pipeline::set_build_context_roots`]. This facade never calls either:
//! [`build_pipeline`] never arms context roots, and both [`render_html`]
//! and [`render_mdx_jsx_module`] drive the pipeline via the context-free
//! path ([`Pipeline::run`] / [`mdx_to_jsx_module_with_pipeline`] without a
//! `source_path`). Each plugin's context-free `visit` is a documented
//! no-op (see `zfb_md_extras::transclude`, `::image_dimensions`,
//! `::link_validation`) — this is exactly the invariant the existing MDX
//! loader path relies on (`crates/zfb-render/src/loader.rs`, around the
//! `with_strip_md_ext_and_gfm_and_cjk_and_features` constructor).

use std::collections::BTreeMap;

use serde::Deserialize;
use zfb_md_extras::MarkdownFeaturesConfig;

use crate::mdx_jsx_emit::{mdx_to_jsx_module_with_pipeline, MdxJsxOptions};
use crate::pipeline::{Pipeline, PipelineError, ResolvedGfmConstructs};
use crate::serializer;
use crate::syntect_highlight::{validate_class_highlight_classes, DEFAULT_CLASS_HIGHLIGHT_PREFIX};

/// Resolved GFM construct toggles, `Deserialize`-compatible mirror of
/// [`ResolvedGfmConstructs`].
///
/// `ResolvedGfmConstructs` itself carries no `serde` derive (it lives in
/// `crate::pipeline`, owned by a sibling sub-issue in this epic — see
/// zfb#1574), so this facade defines its own JSON-shaped twin and converts
/// field-by-field via [`From`].
///
/// Every field is a plain `bool` — this type carries the **already
/// resolved** GFM construct set (the output of
/// `zfb::config::MarkdownConfig::resolve_constructs`), not the raw
/// `zfb.config.ts` `markdown.gfm` union (bool / preset name / per-construct
/// object with tri-state `Option<bool>` deltas) — that richer resolution
/// needs `zfb.config.ts` evaluation and stays build-side, per the epic's
/// "config arrives as resolved JSON" decision.
///
/// JSON field names are camelCase. Defaults match
/// [`ResolvedGfmConstructs::CONSERVATIVE`] — the conservative default used
/// by every other production pipeline (dev loader, bundler, snapshot
/// walker) when a knob is left unset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct GfmOptions {
    /// GFM strikethrough (`~~text~~`). Default: `true`.
    pub strikethrough: bool,
    /// GFM pipe-style tables. Default: `true`.
    pub table: bool,
    /// GFM autolink literal (bare URLs). Default: `true`.
    pub autolink_literal: bool,
    /// GFM task list items (`- [x]` / `- [ ]`). Default: `false`.
    pub task_list_item: bool,
    /// GFM footnote definitions (`[^ref]: …`). Default: `false`.
    pub footnote_definition: bool,
}

impl Default for GfmOptions {
    /// Mirrors [`ResolvedGfmConstructs::CONSERVATIVE`]: strikethrough,
    /// table, and autolink literal on; task-list-item and
    /// footnote-definition off.
    fn default() -> Self {
        Self {
            strikethrough: true,
            table: true,
            autolink_literal: true,
            task_list_item: false,
            footnote_definition: false,
        }
    }
}

impl From<GfmOptions> for ResolvedGfmConstructs {
    fn from(o: GfmOptions) -> Self {
        Self {
            strikethrough: o.strikethrough,
            table: o.table,
            autolink_literal: o.autolink_literal,
            task_list_item: o.task_list_item,
            footnote_definition: o.footnote_definition,
        }
    }
}

/// `codeHighlight.mode` — mirrors the `"inline"` / `"class"` string-literal
/// union accepted by native `zfb::config::CodeHighlightConfig::mode`
/// (`crates/zfb/src/config.rs`).
///
/// A facade-local mirror rather than a reuse of
/// [`crate::pipeline_spec::CodeHighlightMode`] (also re-exported at the
/// crate root as `CodeHighlightMode`, so importing both under one name
/// would collide): that type has no `Deserialize` impl and is constructed
/// from already-resolved `zfb.config.ts` JSON through a completely
/// different path (`pipeline_spec_from_config` in
/// `crates/zfb/src/commands/bundler_input.rs`). Giving this wasm-facing
/// `deny_unknown_fields` boundary its own tiny enum keeps it self-contained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum CodeHighlightMode {
    /// Per-token inline colors (pre-epic behaviour). Default.
    #[default]
    Inline,
    /// Per-token semantic role classes; colors resolved via CSS instead of
    /// inline styles. Routes [`build_pipeline`] to
    /// [`Pipeline::with_defaults_and_full_config_class`].
    Class,
}

/// `codeHighlight` sub-object of [`PipelineOptions`] (Highlight Tokens epic
/// zfb#1528, wasm routing sub zfb#1852).
///
/// Mirrors the subset of native `zfb::config::CodeHighlightConfig`
/// (`crates/zfb/src/config.rs`) this facade exposes: `mode`, `classPrefix`,
/// `roleClasses`. `theme` (native's `theme`/`themeLight`/`themeDark`/
/// `themesDir` quartet, minus the filesystem-only `themesDir` this facade
/// never exposes) stays at the TOP LEVEL of [`PipelineOptions`], outside
/// this sub-object — a deliberate partial congruence with the native
/// shape, not an oversight: nesting `theme` here too would be a breaking
/// change to the pre-existing `PipelineOptions::theme` field, and this
/// facade has no dual-theme knob to nest alongside it either.
///
/// `mode: "class"` combined with a top-level [`PipelineOptions::theme`] is
/// rejected by [`build_pipeline`] — themes don't affect class emission, so
/// silently ignoring the theme would surprise callers (same precedent as
/// native config.rs's `codeHighlight.mode "class"` exclusion checks).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct CodeHighlightOptions {
    /// Output mode for fenced-code highlighting. Default: `"inline"` —
    /// reproduces the pre-existing per-token inline-color behaviour
    /// byte-for-byte.
    pub mode: CodeHighlightMode,
    /// Class-name prefix for class-mode role classes (e.g. the default
    /// `"hi-"` yields `hi-kw`, `hi-str`, ...). Only meaningful when
    /// [`Self::mode`] is [`CodeHighlightMode::Class`].
    ///
    /// Defaults to [`DEFAULT_CLASS_HIGHLIGHT_PREFIX`] — shared with native
    /// config and the direct arbitrary-code highlighter (see that
    /// constant's docs) so wasm/native defaults cannot diverge.
    #[serde(default = "default_class_highlight_prefix")]
    pub class_prefix: String,
    /// Per-role class overrides for class mode, e.g.
    /// `{ "keyword": "text-violet-600 dark:text-violet-400" }`. Keys must
    /// be one of the fixed role names in
    /// [`crate::hi_roles::HiRole::FULL_NAMES`]; `None` (the default) uses
    /// `{classPrefix}{role}` for every role. Only meaningful when
    /// [`Self::mode`] is [`CodeHighlightMode::Class`]. Validated by
    /// [`validate_class_highlight_classes`] in [`build_pipeline`] — the
    /// same validator native config uses.
    pub role_classes: Option<BTreeMap<String, String>>,
}

impl Default for CodeHighlightOptions {
    fn default() -> Self {
        Self {
            mode: CodeHighlightMode::default(),
            class_prefix: default_class_highlight_prefix(),
            role_classes: None,
        }
    }
}

fn default_class_highlight_prefix() -> String {
    DEFAULT_CLASS_HIGHLIGHT_PREFIX.to_string()
}

/// The wasm-safe pipeline knob set — the JSON shape a `zfb-md-wasm` host
/// binds `compile(source, optionsJson)` / `renderHtml(source, optionsJson)`
/// to.
///
/// Deserialized with `deny_unknown_fields`: a typo'd key fails fast with a
/// named-field error rather than being silently ignored — same discipline
/// as [`MarkdownFeaturesConfig`] and the rest of the `zfb.config.ts`
/// mirror types.
///
/// Every field is optional in the JSON document; absent fields fall back
/// to [`PipelineOptions::default`], which reproduces the same effective
/// pipeline shape as [`Pipeline::with_defaults_and_full_config`] called
/// with `theme: None`, `themes_dir: None`, `features: None` — the
/// production "nothing configured" pipeline.
///
/// # JSON shape
///
/// ```json
/// {
///   "theme": "InspiredGitHub",
///   "gfm": {
///     "strikethrough": true,
///     "table": true,
///     "autolinkLiteral": true,
///     "taskListItem": false,
///     "footnoteDefinition": false
///   },
///   "cjkFriendly": true,
///   "hardBreaks": false,
///   "codeHighlight": {
///     "mode": "class",
///     "classPrefix": "hi-",
///     "roleClasses": { "keyword": "text-violet-600 dark:text-violet-400" }
///   },
///   "features": {
///     "githubAlerts": true,
///     "readingTime": { "wpm": 200 },
///     "codeEnrichment": {
///       "diffMarkers": true,
///       "lineHighlight": true,
///       "wordHighlight": true
///     },
///     "codeTabs": true,
///     "ruby": true,
///     "tocExport": { "maxDepth": 3 },
///     "imageDimensions": { "skipRemote": true },
///     "linkValidation": { "failOnBroken": false },
///     "transclude": { "maxDepth": 5 },
///     "directives": { "note": "Note" },
///     "mermaid": true,
///     "headingMarkerToc": { "heading": "TOC", "maxDepth": 2 },
///     "headingIds": { "strategy": "flat" }
///   }
/// }
/// ```
///
/// Every sub-shape under `features` is [`MarkdownFeaturesConfig`] verbatim
/// — see that type's field docs (`crates/zfb-md-ast/src/features_config.rs`)
/// for the full per-feature contract (bool-shorthand vs. options-object
/// forms, which keys are required, etc.). `theme` is a SYNTECT theme name
/// (e.g. `"InspiredGitHub"`, `"base16-ocean.dark"`), NOT a Shiki name.
///
/// There is deliberately **no `themesDir` key**: loading `.tmTheme` files
/// from a directory is filesystem I/O, which this facade never performs —
/// see the module docs.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct PipelineOptions {
    /// Syntect highlight theme name. Absent, or explicit JSON `null`, keep
    /// the built-in default theme (`base16-ocean.dark`) — fenced code is
    /// ALWAYS highlighted through this field; there is no "disable
    /// highlighting" value (matches native `zfb::config::CodeHighlightConfig`
    /// — `crates/zfb/src/config.rs` — where an absent `theme` carries the
    /// identical "defaults to `base16-ocean.dark`" contract). An earlier
    /// draft of the TS-side docs (`crates/zfb-md-wasm/npm/src/types.ts`)
    /// claimed `null` meant "no syntax highlighting"; that was never true of
    /// this deserializer and has been corrected there (zfb#1852) rather
    /// than changed here, to keep this knob's semantics identical to
    /// native config's `theme`. Mirrors
    /// [`Pipeline::with_defaults_and_full_config`]'s `theme` parameter.
    ///
    /// Mutually exclusive with [`Self::code_highlight`]'s `mode: "class"`
    /// — see that field's docs.
    pub theme: Option<String>,
    /// Resolved GFM construct set. Default: conservative (strikethrough +
    /// table on).
    pub gfm: GfmOptions,
    /// Whether to run `CjkFriendlyPlugin` (CJK-aware emphasis/strong
    /// retokenisation). Default: `true`.
    pub cjk_friendly: bool,
    /// Whether to run `HardBreaksPlugin` (single newlines become `<br>`).
    /// Default: `false`.
    pub hard_breaks: bool,
    /// Output mode + class-mode knobs for fenced-code highlighting
    /// (Highlight Tokens epic zfb#1528). Absent, `null`, or
    /// `{ "mode": "inline" }` reproduce the pre-existing per-token
    /// inline-color behaviour byte-for-byte — see [`CodeHighlightOptions`].
    pub code_highlight: Option<CodeHighlightOptions>,
    /// Opt-in feature set. Default: every feature disabled (the post-epic
    /// opt-in default — see [`MarkdownFeaturesConfig`]'s own docs).
    pub features: MarkdownFeaturesConfig,
}

impl Default for PipelineOptions {
    fn default() -> Self {
        Self {
            theme: None,
            gfm: GfmOptions::default(),
            cjk_friendly: true,
            hard_breaks: false,
            code_highlight: None,
            features: MarkdownFeaturesConfig::default(),
        }
    }
}

/// Errors surfaced by the config-JSON-driven facade entry points.
#[derive(Debug, thiserror::Error)]
pub enum FacadeError {
    /// `serde_json` rejected the options document — malformed JSON, an
    /// unknown field (`deny_unknown_fields`), or a value that doesn't
    /// match the expected shape (e.g. an unknown field inside
    /// `codeEnrichment`, an invalid `headingIds.strategy`).
    #[error("invalid pipeline options JSON: {0}")]
    InvalidConfig(#[from] serde_json::Error),
    /// Pipeline construction rejected the resolved options. With
    /// `themes_dir` never set by this facade (see the module docs), the
    /// only reachable case is
    /// [`HighlightError::UnknownTheme`](crate::syntect_highlight::HighlightError::UnknownTheme):
    /// `with_defaults_and_full_config` validates the configured theme
    /// *name* against the loaded theme set at build start (zfb#1067 /
    /// zfb#1070), so a misspelled `theme` fails loudly instead of
    /// silently rendering unhighlighted code blocks.
    #[error("invalid pipeline options: {0}")]
    Highlight(#[from] crate::syntect_highlight::HighlightError),
    /// `options.code_highlight.mode` is `"class"` while `options.theme` is
    /// also set. Themes don't affect class emission, so silently ignoring
    /// the theme would surprise callers — mirrors native config.rs's
    /// `codeHighlight.mode "class" is mutually exclusive with
    /// codeHighlight.theme` precedent (`crates/zfb/src/config.rs:2194-2218`).
    #[error(
        "invalid pipeline options: codeHighlight.mode \"class\" is mutually exclusive with theme"
    )]
    ClassModeExcludesTheme,
    /// `options.code_highlight.classPrefix` / `roleClasses` failed the
    /// shared [`validate_class_highlight_classes`] check — the same
    /// validator native config uses (prefixed with `codeHighlight.` there
    /// too).
    #[error("invalid pipeline options: codeHighlight.{0}")]
    ClassHighlight(#[from] crate::syntect_highlight::ClassHighlightValidationError),
    /// The pipeline itself failed to run against the supplied source.
    #[error(transparent)]
    Pipeline(#[from] PipelineError),
}

/// Parse a JSON options document into [`PipelineOptions`].
///
/// # Errors
/// Returns [`FacadeError::InvalidConfig`] when `config_json` is not valid
/// JSON, or does not match the [`PipelineOptions`] shape (unknown field,
/// wrong type, or a per-feature validation the `Deserialize` impl
/// enforces).
pub fn parse_pipeline_options(config_json: &str) -> Result<PipelineOptions, FacadeError> {
    Ok(serde_json::from_str(config_json)?)
}

/// Build a fully-wired [`Pipeline`] from already-resolved [`PipelineOptions`].
///
/// Mirrors [`Pipeline::with_defaults_and_full_config`] with `themes_dir`
/// hard-coded to `None` and `build_context_roots` never armed — see the
/// module docs for why that keeps `transclude` / `imageDimensions` /
/// `linkValidation` inert.
///
/// When [`PipelineOptions::code_highlight`]'s `mode` is
/// [`CodeHighlightMode::Class`], routes to
/// [`Pipeline::with_defaults_and_full_config_class`] instead (Highlight
/// Tokens epic zfb#1528, wasm routing sub zfb#1852) — every other knob
/// wires through identically.
///
/// # Errors
/// Returns [`FacadeError::Highlight`] when `options.theme` names a theme
/// that does not exist in the built-in syntect theme set —
/// `with_defaults_and_full_config` validates theme names at build start
/// (zfb#1067 / zfb#1070). A wasm host must surface this as a structured
/// diagnostic, never a panic (zfb#1576): the theme name comes straight
/// from user-supplied options JSON, and on `wasm32-unknown-unknown` a
/// panic traps and poisons the instance.
///
/// Returns [`FacadeError::ClassModeExcludesTheme`] when `code_highlight.mode`
/// is `"class"` and `theme` is also set, and
/// [`FacadeError::ClassHighlight`] when `code_highlight.classPrefix` /
/// `roleClasses` fails validation — checked whenever `code_highlight` is
/// present, regardless of `mode` (native parity, zfb#1865 / zfb#1978): the
/// fields are inert in `"inline"` mode but must still reject malformed
/// input exactly as native `config.rs` does.
pub fn build_pipeline(options: &PipelineOptions) -> Result<Pipeline, FacadeError> {
    let resolved_gfm: ResolvedGfmConstructs = options.gfm.into();
    // Native config.rs (crates/zfb/src/config.rs:2184-2236) validates
    // classPrefix/roleClasses for ANY code_highlight present, not just
    // mode "class" — those fields are inert in "inline" mode but still
    // get parsed and must reject the same malformed input native does
    // (zfb#1865 / zfb#1978). In class mode, native runs the class/theme
    // mutual-exclusion check FIRST (config.rs:2194-2218), then
    // classPrefix/roleClasses validation (config.rs:2230-2236) — preserve
    // that precedence rather than validating classPrefix/roleClasses
    // unconditionally up front, or a config with both a theme AND a
    // malformed classPrefix would report the wrong error.
    match &options.code_highlight {
        Some(ch) if ch.mode == CodeHighlightMode::Class => {
            if options.theme.is_some() {
                return Err(FacadeError::ClassModeExcludesTheme);
            }
            let role_classes = ch.role_classes.clone().unwrap_or_default();
            validate_class_highlight_classes(&ch.class_prefix, &role_classes)?;
            Ok(Pipeline::with_defaults_and_full_config_class(
                resolved_gfm,
                None,
                options.cjk_friendly,
                options.hard_breaks,
                Some(&options.features),
                &ch.class_prefix,
                &role_classes,
            )?)
        }
        Some(ch) => {
            // Non-class mode (e.g. "inline"): classPrefix/roleClasses are
            // inert here, but native still validates them whenever
            // code_highlight is present — see the module-level note above.
            let role_classes = ch.role_classes.clone().unwrap_or_default();
            validate_class_highlight_classes(&ch.class_prefix, &role_classes)?;
            Ok(Pipeline::with_defaults_and_full_config(
                options.theme.as_deref(),
                resolved_gfm,
                None,
                options.cjk_friendly,
                options.hard_breaks,
                Some(&options.features),
            )?)
        }
        None => Ok(Pipeline::with_defaults_and_full_config(
            options.theme.as_deref(),
            resolved_gfm,
            None,
            options.cjk_friendly,
            options.hard_breaks,
            Some(&options.features),
        )?),
    }
}

/// Parse `config_json` and build a fully-wired [`Pipeline`] in one step.
///
/// # Errors
/// Returns [`FacadeError::InvalidConfig`] on malformed/invalid config JSON
/// (see [`parse_pipeline_options`]), or [`FacadeError::Highlight`] when
/// the config names an unknown syntect theme (see [`build_pipeline`]).
pub fn build_pipeline_from_json(config_json: &str) -> Result<Pipeline, FacadeError> {
    let options = parse_pipeline_options(config_json)?;
    build_pipeline(&options)
}

/// Render `input` to an HTML fragment string through `pipeline`.
///
/// This is the `mdast → hast → HTML` composition — `pipeline.run(input)`
/// (which internally builds hast via
/// [`crate::pipeline::mdast_to_hast_with`] under the
/// `JsxEmitStrategy::HtmlPath` strategy) followed by
/// [`crate::serializer::serialize`] — promoted out of test-only helpers
/// (e.g. `render_fixture_with` in `tests/integration_pipeline.rs`) into a
/// public function, per zfb#1574's acceptance criteria.
///
/// Drives the pipeline via [`Pipeline::run`] (the context-free path) —
/// never [`Pipeline::run_with_context`] — so the fs-bound feature plugins
/// stay inert regardless of what `features.*` the pipeline was built with.
///
/// Reusing one `Pipeline` across multiple calls is safe for stateless
/// plugins; call [`Pipeline::reset_per_entry`] between documents if the
/// configured feature set includes stateful ones (e.g. duplicate-heading
/// slug counters) — see that method's docs.
///
/// # Errors
/// Returns [`PipelineError::Parse`] if markdown-rs rejects `input`.
pub fn render_html(pipeline: &mut Pipeline, input: &str) -> Result<String, PipelineError> {
    let hast = pipeline.run(input)?;
    Ok(serializer::serialize(&hast))
}

/// Compile `input` (MDX source) into a JSX module string through
/// `pipeline`.
///
/// Thin wrapper over [`mdx_to_jsx_module_with_pipeline`] that fills in
/// [`MdxJsxOptions`] from just a display `filename` (used in parse-error
/// messages) — no `source_path` is threaded through, since this facade
/// never arms build-context roots (see the module docs), which is the
/// only thing `source_path` would be used for.
///
/// The returned string is JSX **source text** — feed it through a JSX/TSX
/// compiler (e.g. `zfb-render::SwcPipeline`, or `swc_core` directly in a
/// wasm host) to get executable ES module JS.
///
/// # Errors
/// Returns [`PipelineError::Parse`] if markdown-rs rejects `input`.
pub fn render_mdx_jsx_module(
    pipeline: &mut Pipeline,
    input: &str,
    filename: &str,
) -> Result<String, PipelineError> {
    let opts = MdxJsxOptions::default().with_filename(filename);
    mdx_to_jsx_module_with_pipeline(input, opts, pipeline)
}

/// Base syntax used by [`parse_mdast`].
///
/// GFM is deliberately not a third dialect: its five constructs are applied
/// independently through [`ParseMdastOptions::gfm`] on top of either base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParseDialect {
    /// CommonMark, including HTML/comments, angle autolinks, indented code,
    /// and literal braces.
    Markdown,
    /// MDX, including JSX and expressions and markdown-rs's corresponding
    /// conflicting-CommonMark construct exclusions.
    #[default]
    Mdx,
}

/// Options for the raw mdast parser facade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ParseMdastOptions {
    /// CommonMark or MDX base construct set. Defaults to MDX to preserve the
    /// raw facade's pre-dialect behavior for direct Rust callers.
    pub dialect: ParseDialect,
    /// Five independently resolved GFM construct switches.
    pub gfm: GfmOptions,
    /// Recognize markdown-rs YAML frontmatter and emit a `yaml` node.
    /// Disabled by default; zfb's wasm boundary enables it only after its
    /// stricter YAML-only recognizer has accepted the opening/closing fences.
    pub frontmatter: bool,
}

/// Recursive raw-mdast carrier used by interoperability boundaries which
/// need to compose node kinds that markdown-rs's closed [`markdown::mdast::Node`]
/// enum cannot represent (currently the three remark-directive node kinds).
///
/// This is an alias rather than a second structurally identical carrier so
/// the directive parser remains the single owner of carrier validation.
pub type InteropMdastNode = crate::directive_parser::DirectiveMdastNode;

/// Parse `input` into a RAW markdown-rs mdast tree — feeds `zfb-md-wasm`'s
/// `parseToAst` export (zfb#1857, epic zfb#1854).
///
/// Contract locked at epic planning: the returned tree is the raw parser
/// output — post-frontmatter-strip (the caller strips frontmatter before
/// calling this; see `zfb-md-wasm`'s `parseToAst`), PRE-zfb-visitors — so
/// every node carries real parser `position` info and no visitor-synthesized
/// `position: None` nodes exist. Markdown starts from
/// [`markdown::ParseOptions::default`], MDX starts from
/// [`markdown::ParseOptions::mdx`], and the same five resolved GFM switches
/// are then applied to either base. Math stays off.
///
/// `zfb-md-wasm` converts this function's UTF-8-byte positions to the
/// UTF-16 code-unit contract `parseToAst` actually returns — see that
/// crate's `Utf16Positions` for the conversion. Positions returned directly
/// from this function stay markdown-rs-native (UTF-8 bytes).
///
/// # Errors
/// Returns [`PipelineError::Parse`] if markdown-rs rejects `input`.
pub fn parse_mdast(
    options: ParseMdastOptions,
    input: &str,
) -> Result<markdown::mdast::Node, PipelineError> {
    let resolved_gfm: ResolvedGfmConstructs = options.gfm.into();
    let mut parse_options = match options.dialect {
        ParseDialect::Markdown => markdown::ParseOptions::default(),
        ParseDialect::Mdx => markdown::ParseOptions::mdx(),
    };
    parse_options.constructs.gfm_strikethrough = resolved_gfm.strikethrough;
    parse_options.constructs.gfm_table = resolved_gfm.table;
    parse_options.constructs.gfm_autolink_literal = resolved_gfm.autolink_literal;
    parse_options.constructs.gfm_task_list_item = resolved_gfm.task_list_item;
    // markdown-rs splits footnotes into block-definition and inline-label
    // constructs; the locked public flag controls both halves.
    parse_options.constructs.gfm_footnote_definition = resolved_gfm.footnote_definition;
    parse_options.constructs.gfm_label_start_footnote = resolved_gfm.footnote_definition;
    parse_options.constructs.frontmatter = options.frontmatter;
    markdown::to_mdast(input, &parse_options).map_err(|m| PipelineError::Parse(m.to_string()))
}

/// Parse into the open recursive interoperability carrier.
///
/// When `directives` is false this follows the ordinary markdown-rs path and
/// converts markdown-rs's own serde output into the carrier. Crucially, it
/// does not invoke the directive scanner. When true it composes the generic
/// directive parser over the exact same selected dialect/GFM options.
pub fn parse_interop_mdast(
    options: ParseMdastOptions,
    input: &str,
    directives: bool,
) -> Result<InteropMdastNode, PipelineError> {
    if directives {
        return crate::directive_parser::parse_directive_mdast(options, input);
    }

    let base = parse_mdast(options, input)?;
    let serialized = serde_json::to_value(base)
        .map_err(|error| PipelineError::Parse(format!("serialize mdast: {error}")))?;
    serde_json::from_value(serialized)
        .map_err(|error| PipelineError::Parse(format!("validate interop mdast: {error}")))
}

/// One-shot convenience: parse `config_json`, build a [`Pipeline`], and
/// render `source` to an HTML fragment string.
///
/// Equivalent to [`build_pipeline_from_json`] + [`render_html`] — provided
/// because it maps 1:1 onto the wasm epic's `renderHtml(source,
/// optionsJson)` API tier (zfb#1572). Prefer the two-step form when
/// rendering multiple sources against the same config — it avoids
/// re-parsing the options JSON and re-wiring the plugin chain per call.
///
/// # Errors
/// Returns [`FacadeError::InvalidConfig`] on invalid config JSON,
/// [`FacadeError::Highlight`] on an unknown syntect theme name (see
/// [`build_pipeline`]), or [`FacadeError::Pipeline`] if markdown-rs
/// rejects `source`.
pub fn render_html_from_config(config_json: &str, source: &str) -> Result<String, FacadeError> {
    let mut pipeline = build_pipeline_from_json(config_json)?;
    Ok(render_html(&mut pipeline, source)?)
}

/// One-shot convenience: parse `config_json`, build a [`Pipeline`], and
/// compile `source` (MDX) into a JSX module string.
///
/// Equivalent to [`build_pipeline_from_json`] + [`render_mdx_jsx_module`] —
/// provided because it maps 1:1 onto the wasm epic's `compile(source,
/// optionsJson)` API tier (zfb#1572). Prefer the two-step form when
/// compiling multiple sources against the same config.
///
/// # Errors
/// Returns [`FacadeError::InvalidConfig`] on invalid config JSON,
/// [`FacadeError::Highlight`] on an unknown syntect theme name (see
/// [`build_pipeline`]), or [`FacadeError::Pipeline`] if markdown-rs
/// rejects `source`.
pub fn compile_mdx_jsx_from_config(
    config_json: &str,
    source: &str,
    filename: &str,
) -> Result<String, FacadeError> {
    let mut pipeline = build_pipeline_from_json(config_json)?;
    Ok(render_mdx_jsx_module(&mut pipeline, source, filename)?)
}
