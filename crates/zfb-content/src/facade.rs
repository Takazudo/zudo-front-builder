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

use serde::Deserialize;
use zfb_md_extras::MarkdownFeaturesConfig;

use crate::mdx_jsx_emit::{mdx_to_jsx_module_with_pipeline, MdxJsxOptions};
use crate::pipeline::{Pipeline, PipelineError, ResolvedGfmConstructs};
use crate::serializer;

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
    /// GFM autolink literal (bare URLs). Default: `false`.
    pub autolink_literal: bool,
    /// GFM task list items (`- [x]` / `- [ ]`). Default: `false`.
    pub task_list_item: bool,
    /// GFM footnote definitions (`[^ref]: …`). Default: `false`.
    pub footnote_definition: bool,
}

impl Default for GfmOptions {
    /// Mirrors [`ResolvedGfmConstructs::CONSERVATIVE`]: strikethrough +
    /// table on, every other GFM construct off.
    fn default() -> Self {
        Self {
            strikethrough: true,
            table: true,
            autolink_literal: false,
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
///     "autolinkLiteral": false,
///     "taskListItem": false,
///     "footnoteDefinition": false
///   },
///   "cjkFriendly": true,
///   "hardBreaks": false,
///   "features": {
///     "githubAlerts": true,
///     "readingTime": { "wpm": 200 },
///     "githubAutolinks": { "repo": "owner/repo" },
///     "codeEnrichment": { "diffMarkers": true, "lineHighlight": true },
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
    /// Syntect highlight theme name. `None` keeps the built-in default
    /// (`base16-ocean.dark`). Mirrors
    /// [`Pipeline::with_defaults_and_full_config`]'s `theme` parameter.
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
            features: MarkdownFeaturesConfig::default(),
        }
    }
}

/// Errors surfaced by the config-JSON-driven facade entry points.
#[derive(Debug, thiserror::Error)]
pub enum FacadeError {
    /// `serde_json` rejected the options document — malformed JSON, an
    /// unknown field (`deny_unknown_fields`), or a value that doesn't
    /// match the expected shape (e.g. `githubAutolinks: {}` missing
    /// `repo`, an invalid `headingIds.strategy`).
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
/// # Errors
/// Returns [`FacadeError::Highlight`] when `options.theme` names a theme
/// that does not exist in the built-in syntect theme set —
/// `with_defaults_and_full_config` validates theme names at build start
/// (zfb#1067 / zfb#1070). A wasm host must surface this as a structured
/// diagnostic, never a panic (zfb#1576): the theme name comes straight
/// from user-supplied options JSON, and on `wasm32-unknown-unknown` a
/// panic traps and poisons the instance.
pub fn build_pipeline(options: &PipelineOptions) -> Result<Pipeline, FacadeError> {
    let resolved_gfm: ResolvedGfmConstructs = options.gfm.into();
    Ok(Pipeline::with_defaults_and_full_config(
        options.theme.as_deref(),
        resolved_gfm,
        None,
        options.cjk_friendly,
        options.hard_breaks,
        Some(&options.features),
    )?)
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
