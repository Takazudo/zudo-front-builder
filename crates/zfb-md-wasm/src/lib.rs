//! `zfb-md-wasm` — zfb's md/mdx conversion pipeline as a WebAssembly module
//! (epic zfb#1572, sub-issue zfb#1576).
//!
//! Four API tiers over one cdylib, all JSON-in/JSON-out strings so the
//! wasm boundary stays trivially serializable:
//!
//! 1. [`compile`] — full mdx → JSX → SWC → ES-module JS. Returns
//!    `{ code, frontmatter, diagnostics }`.
//! 2. [`render_html`] — md → mdast → pipeline visitors → hast → HTML
//!    string, no SWC at runtime. Returns `{ html, frontmatter,
//!    diagnostics }`.
//! 3. [`highlight_code`] — arbitrary source → semantic class-highlighted HTML
//!    (without a Markdown fence). Returns `{ html, diagnostics }`.
//! 4. [`parse_to_ast`] (`parseToAst`, zfb#1857, epic zfb#1854) — md/mdx →
//!    raw markdown-rs mdast (pre-visitors), serialized as a unist tree with
//!    `position.offset`/`position.column` in UTF-16 code units (the
//!    remark/unist convention). Returns `{ ast, frontmatter, diagnostics }`.
//!
//! Plus [`version`] for host-side compatibility checks.
//!
//! Tiers 1–2 above (`compile`/`render_html`, and everything only they use --
//! `WasmOptions`, `Prepared`, `prepare`, `CompileResult`/`RenderHtmlResult`)
//! live behind the `pipeline` feature (default-on). `highlight_code` and
//! `version` are unconditional. `npm/scripts/build.mjs` builds a SECOND,
//! `--no-default-features` artifact under the package's `./highlight`
//! export subpath (zfb#1849, epic zfb#1845) -- the md/MDX/JSX pipeline
//! dominates the shipped wasm bytes (Wave-1 measurement: 51.8% raw / 44.5%
//! gzip), so dropping it is the size knob for consumers that only need
//! syntax highlighting.
//!
//! ## Compile/render options JSON
//!
//! ```json
//! {
//!   "filename": "posts/hello.mdx",
//!   "jsxRuntime": "preact",
//!   "development": false,
//!   "pipeline": { "theme": null, "gfm": {}, "cjkFriendly": true,
//!                 "hardBreaks": false,
//!                 "codeHighlight": { "mode": "class", "classPrefix": "hi-",
//!                                     "roleClasses": {} },
//!                 "features": {} }
//! }
//! ```
//!
//! `pipeline.theme`, absent or explicit `null`, keeps the built-in default
//! theme (`base16-ocean.dark`) — fenced code is always highlighted through
//! this field, there is no "disable highlighting" value. `pipeline.codeHighlight`
//! (Highlight Tokens epic zfb#1528, wasm routing sub zfb#1852), when
//! present, switches `mode` between `"inline"` (default, per-token
//! `style="color:#…"`, the pre-#1852 behaviour) and `"class"` (per-token
//! semantic role classes, no inline color); `classPrefix`/`roleClasses`
//! are only meaningful in `"class"` mode. `mode: "class"` combined with a
//! top-level `theme` is rejected — themes don't affect class emission.
//!
//! Every field is optional (`{}` selects all defaults). `filename` drives
//! frontmatter dispatch (`.md`/`.mdx`) and diagnostics; it defaults to
//! `<anonymous>.mdx` for [`compile`] and `<anonymous>.md` for
//! [`render_html`]. `jsxRuntime` (`"preact"` | `"react"`) and
//! `development` are consumed only by [`compile`]; [`render_html`]
//! accepts and ignores them so one options document can serve both
//! tiers. `pipeline` is [`zfb_content::facade::PipelineOptions`]
//! verbatim — see that type's rustdoc for the authoritative shape.
//! Unknown fields are rejected at both levels (`deny_unknown_fields`).
//! [`parse_to_ast`] instead accepts a distinct closed document containing
//! `filename`, `dialect` (`"markdown"` or `"mdx"`), and `pipeline.gfm`.
//! Its absent filename defaults to `<anonymous>.mdx`; absent dialect is
//! inferred as Markdown for `.md` and MDX for `.mdx`, while an explicit
//! dialect overrides either valid extension.
//!
//! ## Result JSON
//!
//! `code` / `html` is a string on success, `null` on failure.
//! `frontmatter` is the parsed YAML frontmatter as JSON (`null` when the
//! source has none, or when extraction itself failed). `diagnostics` is
//! an array (empty on success) of:
//!
//! ```json
//! { "severity": "error", "source": "markdown", "message": "…",
//!   "line": 4, "column": 2 }
//! ```
//!
//! `source` ∈ `"options"` (bad options JSON, unknown theme, bad
//! filename), `"frontmatter"` (YAML errors, unterminated block),
//! `"markdown"` (markdown-rs parse errors), `"compile"` (SWC failures on
//! the emitted JSX). `line`/`column` are 1-based and refer to the
//! *markdown source* for `"frontmatter"`/`"markdown"` (frontmatter lines
//! included — positions reported against the stripped body are shifted
//! back), and to the *options JSON document* for `"options"`. They are
//! `null` when the underlying error carries no location. Markdown diagnostic
//! columns use JavaScript UTF-16 code units, matching successful AST positions
//! and `String.prototype.slice` (not UTF-8 bytes or grapheme clusters).
//!
//! ## Error / trap contract (correctness-critical — see README.md)
//!
//! Expected failures (parse errors, malformed options, dependency-enforced
//! input limits such as serde_yaml's recursion cap) come back as
//! structured diagnostics — these functions do not intentionally panic
//! on supported error paths. A *bug*-level panic on
//! `wasm32-unknown-unknown` traps and poisons the instance; the host
//! wrapper must re-instantiate (the API is stateless per call, so re-init
//! loses nothing). Full contract in this crate's README.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use wasm_bindgen::prelude::wasm_bindgen;
#[cfg(feature = "pipeline")]
use zfb_content::facade::{self, GfmOptions, ParseDialect, ParseMdastOptions, PipelineOptions};
#[cfg(feature = "pipeline")]
use zfb_content::frontmatter::{extract_from_filename, FrontmatterError};
#[cfg(feature = "pipeline")]
use zfb_content::pipeline::{Pipeline, PipelineError};
use zfb_content::syntect_highlight::{
    ClassHighlightFallbackReason, ClassHighlightRenderError, Highlighter,
    DEFAULT_CLASS_HIGHLIGHT_PREFIX,
};
#[cfg(feature = "pipeline")]
use zfb_render::swc_pipeline::{CompileOptions, JsxRuntime, SwcPipeline};

/// `jsxRuntime` option values, mirroring
/// [`zfb_render::swc_pipeline::JsxRuntime`].
#[cfg(feature = "pipeline")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum JsxRuntimeOption {
    #[default]
    Preact,
    React,
}

#[cfg(feature = "pipeline")]
impl From<JsxRuntimeOption> for JsxRuntime {
    fn from(o: JsxRuntimeOption) -> Self {
        match o {
            JsxRuntimeOption::Preact => JsxRuntime::Preact,
            JsxRuntimeOption::React => JsxRuntime::React,
        }
    }
}

/// The wasm-boundary options document — see the crate docs for the JSON
/// shape. Wraps the facade's [`PipelineOptions`] under `pipeline` and
/// adds the SWC-tier knobs the facade deliberately does not know about.
#[cfg(feature = "pipeline")]
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
struct WasmOptions {
    filename: Option<String>,
    jsx_runtime: JsxRuntimeOption,
    development: bool,
    pipeline: PipelineOptions,
}

/// Raw-parser-only pipeline options. Keeping this closed and limited to GFM
/// prevents visitor/serializer knobs from being silently accepted by
/// `parseToAst` even though that entry point cannot apply them.
#[cfg(feature = "pipeline")]
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
struct ParsePipelineOptions {
    gfm: GfmOptions,
}

/// The distinct, closed options document consumed only by [`parse_to_ast`].
///
/// Directive and frontmatter-policy keys intentionally remain unknown until
/// their owning implementations can honor them; accepting either as a no-op
/// would make this boundary behaviorally incoherent.
#[cfg(feature = "pipeline")]
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
struct ParseToAstOptions {
    #[serde(default, deserialize_with = "deserialize_present_non_null")]
    filename: Option<String>,
    #[serde(default, deserialize_with = "deserialize_present_non_null")]
    dialect: Option<ParseDialect>,
    pipeline: ParsePipelineOptions,
}

/// Deserialize an optional field while rejecting an explicitly present
/// `null`. Serde calls this only when the key exists; omission still uses the
/// field default (`None`).
#[cfg(feature = "pipeline")]
fn deserialize_present_non_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// The only direct-code output mode currently supported by the public API.
/// Keeping this a closed enum means a future mode must be deliberately
/// designed instead of being silently accepted and ignored.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum HighlightCodeMode {
    #[default]
    Class,
}

/// Options for [`highlight_code`]. This is intentionally not the Markdown
/// pipeline's options document: arbitrary code has no filename/frontmatter or
/// theme configuration, and always emits semantic classes.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HighlightCodeOptions {
    /// Required so a missing language is distinct from an unknown non-empty
    /// language, which is an escaped successful fallback.
    language: String,
    #[serde(default)]
    mode: HighlightCodeMode,
    #[serde(default = "default_class_highlight_prefix")]
    class_prefix: String,
    #[serde(default)]
    role_classes: BTreeMap<String, String>,
}

fn default_class_highlight_prefix() -> String {
    DEFAULT_CLASS_HIGHLIGHT_PREFIX.to_string()
}

/// One diagnostic entry — see the crate docs for field semantics.
#[derive(Debug, Serialize)]
struct Diagnostic {
    severity: &'static str,
    source: &'static str,
    message: String,
    line: Option<u64>,
    column: Option<u64>,
}

impl Diagnostic {
    fn error(source: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: "error",
            source,
            message: message.into(),
            line: None,
            column: None,
        }
    }

    fn warning(source: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: "warning",
            source,
            message: message.into(),
            line: None,
            column: None,
        }
    }

    fn at(mut self, line: Option<u64>, column: Option<u64>) -> Self {
        self.line = line;
        self.column = column;
        self
    }
}

/// Result document of [`compile`].
#[cfg(feature = "pipeline")]
#[derive(Debug, Serialize)]
struct CompileResult {
    code: Option<String>,
    frontmatter: JsonValue,
    diagnostics: Vec<Diagnostic>,
}

/// Result document of [`render_html`].
#[cfg(feature = "pipeline")]
#[derive(Debug, Serialize)]
struct RenderHtmlResult {
    html: Option<String>,
    frontmatter: JsonValue,
    diagnostics: Vec<Diagnostic>,
}

/// Result document of [`parse_to_ast`].
///
/// `ast` is the RAW markdown-rs mdast tree serialized via markdown-rs's own
/// `serde` feature (unist-shaped: `type` tag, camelCase, `position` omitted
/// when absent), post-frontmatter-strip, PRE-zfb-visitors — with every
/// `position` shifted back into original-source coordinates (lines by the
/// frontmatter's line count, offsets/columns in UTF-16 code units — see
/// [`parse_to_ast`]'s docs for the full contract).
#[cfg(feature = "pipeline")]
#[derive(Debug, Serialize)]
struct ParseToAstResult {
    ast: Option<markdown::mdast::Node>,
    frontmatter: JsonValue,
    diagnostics: Vec<Diagnostic>,
}

/// Result document of [`highlight_code`]. Unlike the Markdown APIs, direct
/// highlighting has no frontmatter and can succeed with an escaped fallback
/// plus a warning diagnostic.
#[derive(Debug, Serialize)]
struct HighlightCodeResult {
    html: Option<String>,
    diagnostics: Vec<Diagnostic>,
}

/// Everything the two tiers share once options + frontmatter + pipeline
/// are resolved.
#[cfg(feature = "pipeline")]
struct Prepared {
    frontmatter: JsonValue,
    body: String,
    /// Byte offset of `body` in the original source. Diagnostics use this
    /// together with the body text to reconstruct markdown-rs's byte-based
    /// place before converting it to original-source UTF-16 coordinates.
    body_offset: usize,
    pipeline: Pipeline,
    filename: String,
    jsx_runtime: JsxRuntime,
    development: bool,
}

/// Shared front half: options JSON → frontmatter extraction → pipeline
/// construction. The error branch carries the best-effort frontmatter
/// (already-extracted values are still returned when a later stage
/// fails). The `Err` is boxed: the `(JsonValue, Diagnostic)` tuple is
/// ~160 bytes on 64-bit hosts (`clippy::result_large_err` under the native
/// `cargo clippy --workspace`), though it stays under the threshold on the
/// shipped wasm32 target — boxing keeps it lint-clean on every target.
#[cfg(feature = "pipeline")]
fn prepare(
    source: &str,
    options_json: &str,
    default_filename: &str,
) -> Result<Prepared, Box<(JsonValue, Diagnostic)>> {
    let opts: WasmOptions = match serde_json::from_str(options_json) {
        Ok(o) => o,
        Err(e) => {
            let diag = Diagnostic::error("options", format!("invalid options JSON: {e}"))
                .at(Some(e.line() as u64), Some(e.column() as u64));
            return Err(Box::new((JsonValue::Null, diag)));
        }
    };
    let filename = opts
        .filename
        .unwrap_or_else(|| default_filename.to_string());
    let extension = std::path::Path::new(&filename)
        .extension()
        .and_then(|e| e.to_str());
    if !matches!(extension, Some("md" | "mdx")) {
        let diag = Diagnostic::error(
            "options",
            format!("filename `{filename}` must end in `.md` or `.mdx` for this entry point"),
        );
        return Err(Box::new((JsonValue::Null, diag)));
    }

    let extracted = match extract_from_filename(&filename, source) {
        Ok(uf) => uf,
        Err(e) => return Err(Box::new((JsonValue::Null, frontmatter_diagnostic(&e)))),
    };
    let (Some(body), Some(body_offset)) = (extracted.body, extracted.body_offset) else {
        // Unreachable after the extension gate above (`.md`/`.mdx` always
        // yields a body), kept as a diagnostic rather than a panic per the
        // wasm trap contract.
        let diag = Diagnostic::error(
            "options",
            format!("filename `{filename}` did not resolve to a markdown body"),
        );
        return Err(Box::new((JsonValue::Null, diag)));
    };
    let frontmatter = extracted.value;
    let pipeline = match facade::build_pipeline(&opts.pipeline) {
        Ok(p) => p,
        Err(e) => {
            let diag = Diagnostic::error("options", e.to_string());
            return Err(Box::new((frontmatter, diag)));
        }
    };

    Ok(Prepared {
        frontmatter,
        body,
        body_offset,
        pipeline,
        filename,
        jsx_runtime: opts.jsx_runtime.into(),
        development: opts.development,
    })
}

#[cfg(feature = "pipeline")]
fn frontmatter_diagnostic(err: &FrontmatterError) -> Diagnostic {
    match err {
        FrontmatterError::Yaml(e) => {
            let loc = e.location();
            Diagnostic::error("frontmatter", format!("invalid YAML in frontmatter: {e}")).at(
                // serde_yaml locations are 1-based within the YAML text,
                // which begins on source line 2 (after the opening `---`).
                loc.as_ref().map(|l| l.line() as u64 + 1),
                loc.as_ref().map(|l| l.column() as u64),
            )
        }
        FrontmatterError::Unterminated => Diagnostic::error("frontmatter", err.to_string()),
        FrontmatterError::Tsx(_) => Diagnostic::error("frontmatter", err.to_string()),
        FrontmatterError::UnsupportedExtension(_) | FrontmatterError::MissingExtension => {
            Diagnostic::error("options", err.to_string())
        }
    }
}

/// Split a leading markdown-rs place prefix (`L:C: rest` or
/// `L1:C1-L2:C2: rest`) off an error message. Returns the parsed start
/// position and the remainder, or `None` + the untouched message when no
/// well-formed place prefix exists.
///
/// `cfg`'d on `test` too: the unit tests below exercise this directly, and
/// without `pipeline` (its only production caller, via `markdown_diagnostic`)
/// it would otherwise be dead code under `--no-default-features`.
#[cfg(any(feature = "pipeline", test))]
fn split_place(msg: &str) -> (Option<(u64, u64)>, &str) {
    let Some((head, rest)) = msg.split_once(": ") else {
        return (None, msg);
    };
    let start = head.split('-').next().unwrap_or(head);
    let Some((line_s, col_s)) = start.split_once(':') else {
        return (None, msg);
    };
    match (line_s.parse::<u64>(), col_s.parse::<u64>()) {
        (Ok(line), Ok(column)) => (Some((line, column)), rest),
        _ => (None, msg),
    }
}

/// Resolve markdown-rs's 1-based body-relative line and byte-based column to
/// a byte offset in `body`. Tabs follow markdown-rs's four-column tab stops.
/// Locations in the middle of a UTF-8 scalar are rejected: they cannot be
/// represented safely in the original Rust string or in JavaScript UTF-16.
#[cfg(any(feature = "pipeline", test))]
fn markdown_place_to_body_offset(body: &str, line: u64, column: u64) -> Option<usize> {
    let target_line = usize::try_from(line).ok()?;
    let target_column = usize::try_from(column).ok()?;
    if target_line == 0 || target_column == 0 {
        return None;
    }

    let bytes = body.as_bytes();
    let mut line_start = 0usize;
    let mut current_line = 1usize;
    while current_line < target_line {
        let byte = *bytes.get(line_start)?;
        if byte == b'\r' {
            line_start += usize::from(bytes.get(line_start + 1) == Some(&b'\n')) + 1;
            current_line += 1;
        } else if byte == b'\n' {
            line_start += 1;
            current_line += 1;
        } else {
            line_start += 1;
        }
    }

    let mut offset = line_start;
    let mut current_column = 1usize;
    loop {
        if current_column == target_column {
            return body.is_char_boundary(offset).then_some(offset);
        }
        let byte = *bytes.get(offset)?;
        if matches!(byte, b'\r' | b'\n') {
            return None;
        }
        if byte == b'\t' {
            let remainder = current_column % 4;
            let virtual_spaces = if remainder == 0 { 0 } else { 4 - remainder };
            let next_column = current_column.checked_add(1 + virtual_spaces)?;
            if target_column < next_column {
                return body.is_char_boundary(offset).then_some(offset);
            }
            current_column = next_column;
        } else {
            current_column = current_column.checked_add(1)?;
        }
        offset += 1;
    }
}

/// Convert a validated byte offset in the original source to its 1-based line
/// and UTF-16-code-unit column. CRLF is one line ending; lone CR and LF are
/// also supported so malformed upstream places can never trigger subtraction
/// or indexing panics.
#[cfg(any(feature = "pipeline", test))]
fn source_utf16_place(source: &str, offset: usize) -> Option<(u64, u64)> {
    if offset > source.len() || !source.is_char_boundary(offset) {
        return None;
    }
    let bytes = source.as_bytes();
    let mut index = 0usize;
    let mut line = 1u64;
    let mut line_start = 0usize;
    while index < offset {
        if bytes[index] == b'\r' {
            if index + 1 < offset && bytes.get(index + 1) == Some(&b'\n') {
                index += 2;
            } else {
                index += 1;
            }
            line = line.checked_add(1)?;
            line_start = index;
        } else if bytes[index] == b'\n' {
            index += 1;
            line = line.checked_add(1)?;
            line_start = index;
        } else {
            index += 1;
        }
    }
    let column = u64::try_from(source.get(line_start..offset)?.encode_utf16().count())
        .ok()?
        .checked_add(1)?;
    Some((line, column))
}

/// Transform a markdown-rs body-relative place into the public diagnostic
/// coordinate space: the full original source, in JavaScript UTF-16 units.
/// Every relationship is validated so malformed/out-of-range upstream places
/// degrade to a location-less diagnostic instead of trapping wasm.
#[cfg(any(feature = "pipeline", test))]
fn markdown_place_in_source(
    source: &str,
    body: &str,
    body_offset: usize,
    line: u64,
    column: u64,
) -> Option<(u64, u64)> {
    if source.get(body_offset..)? != body {
        return None;
    }
    let relative = markdown_place_to_body_offset(body, line, column)?;
    let absolute = body_offset.checked_add(relative)?;
    source_utf16_place(source, absolute)
}

/// Convert a [`PipelineError`] into a `"markdown"` diagnostic, mapping
/// body-relative byte positions back into original-source UTF-16 coordinates.
#[cfg(feature = "pipeline")]
fn markdown_diagnostic(
    err: &PipelineError,
    filename: &str,
    source: &str,
    body: &str,
    body_offset: usize,
) -> Diagnostic {
    let PipelineError::Parse(raw) = err;
    // The MDX emit path prefixes the message with `{filename}: ` (see
    // zfb-content's mdx_jsx_emit); the HTML path does not.
    let prefix = format!("{filename}: ");
    let stripped = raw.strip_prefix(&prefix).unwrap_or(raw);
    let (place, rest) = split_place(stripped);
    match place.and_then(|(line, column)| {
        markdown_place_in_source(source, body, body_offset, line, column)
    }) {
        Some((line, column)) => Diagnostic::error("markdown", rest).at(Some(line), Some(column)),
        None if place.is_some() => Diagnostic::error("markdown", rest),
        None => Diagnostic::error("markdown", stripped),
    }
}

#[cfg(feature = "pipeline")]
fn compile_impl(source: &str, options_json: &str) -> CompileResult {
    let prepared = match prepare(source, options_json, "<anonymous>.mdx") {
        Ok(p) => p,
        Err(boxed) => {
            let (frontmatter, diag) = *boxed;
            return CompileResult {
                code: None,
                frontmatter,
                diagnostics: vec![diag],
            };
        }
    };
    let Prepared {
        frontmatter,
        body,
        body_offset,
        mut pipeline,
        filename,
        jsx_runtime,
        development,
    } = prepared;

    let jsx = match facade::render_mdx_jsx_module(&mut pipeline, &body, &filename) {
        Ok(jsx) => jsx,
        Err(e) => {
            return CompileResult {
                code: None,
                frontmatter,
                diagnostics: vec![markdown_diagnostic(
                    &e,
                    &filename,
                    source,
                    &body,
                    body_offset,
                )],
            }
        }
    };

    let swc_opts = CompileOptions {
        filename,
        jsx_runtime,
        development,
    };
    match SwcPipeline::new().compile(&jsx, &swc_opts) {
        Ok(module) => CompileResult {
            code: Some(module.code),
            frontmatter,
            diagnostics: vec![],
        },
        Err(e) => CompileResult {
            code: None,
            frontmatter,
            diagnostics: vec![Diagnostic::error("compile", e.to_string())],
        },
    }
}

#[cfg(feature = "pipeline")]
fn render_html_impl(source: &str, options_json: &str) -> RenderHtmlResult {
    let prepared = match prepare(source, options_json, "<anonymous>.md") {
        Ok(p) => p,
        Err(boxed) => {
            let (frontmatter, diag) = *boxed;
            return RenderHtmlResult {
                html: None,
                frontmatter,
                diagnostics: vec![diag],
            };
        }
    };
    let Prepared {
        frontmatter,
        body,
        body_offset,
        mut pipeline,
        filename,
        ..
    } = prepared;

    match facade::render_html(&mut pipeline, &body) {
        Ok(html) => RenderHtmlResult {
            html: Some(html),
            frontmatter,
            diagnostics: vec![],
        },
        Err(e) => RenderHtmlResult {
            html: None,
            frontmatter,
            diagnostics: vec![markdown_diagnostic(
                &e,
                &filename,
                source,
                &body,
                body_offset,
            )],
        },
    }
}

/// The body-relative → original-source position transform: `line_delta`
/// frontmatter lines onto `line`, `offset_delta` body-start bytes onto
/// `offset`, and — ONLY for points on the body's first line —
/// `first_line_column_delta` onto `column`.
///
/// The column delta is 0 in the ordinary case (the stripped frontmatter
/// block is whole lines, so the body starts at column 1 of a fresh line)
/// but non-zero when the closing `---` sits at EOF with no trailing
/// newline: the (empty) body then starts right after the delimiter on the
/// SAME line, and an unshifted column would disagree with the shifted
/// offset beside it (self-review finding on zfb#1855). NOTE: all units here
/// are UTF-8 BYTES, straight from markdown-rs — this is an intermediate
/// step. [`Utf16Positions`] converts the result to the UTF-16 code-unit
/// contract [`parse_to_ast`] actually returns.
#[cfg(feature = "pipeline")]
#[derive(Clone, Copy)]
struct PositionShift {
    line_delta: usize,
    offset_delta: usize,
    first_line_column_delta: usize,
}

#[cfg(feature = "pipeline")]
impl PositionShift {
    /// Derive the shift from the source prefix preceding the body (the
    /// frontmatter block). Mirrors the `prefix_lines` arithmetic in
    /// [`prepare`] / [`markdown_diagnostic`], plus the mid-line body-start
    /// column correction those diagnostics paths do not model.
    fn for_body_at(source: &str, body_offset: usize) -> Self {
        let prefix = source.get(..body_offset).unwrap_or("");
        let line_start = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
        Self {
            line_delta: prefix.matches('\n').count(),
            offset_delta: body_offset,
            first_line_column_delta: prefix.len() - line_start,
        }
    }

    fn apply(&self, point: &mut markdown::unist::Point) {
        if point.line == 1 {
            point.column += self.first_line_column_delta;
        }
        point.line += self.line_delta;
        point.offset += self.offset_delta;
    }
}

/// Build a byte-offset → UTF-16-code-unit-offset prefix map over `source`:
/// `prefix[i]` is the number of UTF-16 code units in `source[..i]`. Valid at
/// every `i` markdown-rs positions can land on (UTF-8 char boundaries —
/// markdown-rs only ever parses, and points into, valid UTF-8 text), plus
/// `source.len()` itself for end-of-source points. One linear pass over
/// `source`'s chars, `char::len_utf16()` per character (2 code units for a
/// scalar value outside the Basic Multilingual Plane — e.g. most emoji —
/// which needs a surrogate pair; 1 otherwise).
#[cfg(feature = "pipeline")]
fn build_utf16_prefix(source: &str) -> Vec<u32> {
    let mut prefix = vec![0u32; source.len() + 1];
    let mut utf16_units = 0u32;
    let mut byte_index = 0usize;
    for ch in source.chars() {
        for _ in 0..ch.len_utf8() {
            prefix[byte_index] = utf16_units;
            byte_index += 1;
        }
        utf16_units += ch.len_utf16() as u32;
    }
    prefix[source.len()] = utf16_units;
    prefix
}

/// Converts already-original-source-shifted BYTE positions (the output of
/// [`PositionShift::apply`]) into UTF-16 code-unit positions — the
/// production contract for `position.offset`/`position.column`
/// (remark/unist convention: `line` is unit-agnostic and needs no
/// conversion). See [`parse_to_ast`]'s docs for why UTF-16, not bytes.
///
/// `prefix` is `None` on the ASCII fast path (a cheap `str::is_ascii` scan):
/// for a pure-ASCII source, byte offsets already equal UTF-16 offsets, so
/// [`apply`](Self::apply) is a no-op and the prefix map is never built —
/// this keeps ASCII-source call cost, and the ASCII benchmark numbers, the
/// same as before this conversion existed.
#[cfg(feature = "pipeline")]
struct Utf16Positions {
    prefix: Option<Vec<u32>>,
}

#[cfg(feature = "pipeline")]
impl Utf16Positions {
    fn for_source(source: &str) -> Self {
        Self {
            prefix: (!source.is_ascii()).then(|| build_utf16_prefix(source)),
        }
    }

    /// Convert one already-shifted point in place. `column` is recomputed as
    /// UTF-16 units from the point's own line start rather than tracking
    /// line-start byte offsets separately: `point.column - 1` already counts
    /// BYTES since that line start (that is what a byte-based column means),
    /// so `point.offset - (point.column - 1)` recovers the line start's byte
    /// offset directly.
    fn apply(&self, point: &mut markdown::unist::Point) {
        let Some(prefix) = &self.prefix else {
            return;
        };
        let line_start_byte = point.offset - (point.column - 1);
        let utf16_offset = prefix[point.offset];
        let utf16_line_start = prefix[line_start_byte];
        point.column = (utf16_offset - utf16_line_start) as usize + 1;
        point.offset = utf16_offset as usize;
    }
}

/// Shift markdown-rs `Stop` pairs (`(index_in_value, absolute_source_offset)`
/// — see `markdown::mdast::Stop`) by the body byte offset. The second tuple
/// element is an absolute source offset (markdown-rs uses it in
/// `Location::relative_to_point` for expression re-parsing), so it must move
/// with `position.*.offset` or the serialized `_markdownRsStops` field would
/// disagree with the shifted positions beside it.
///
/// Deliberately NOT UTF-16-converted: `_markdownRsStops` is
/// markdown-rs-internal re-parse bookkeeping (underscore-prefixed on
/// purpose), documented as internal/unstable/BYTE-based in
/// [`parse_to_ast`]'s docs and in the npm package's `types.ts` — never slice
/// a string with it.
#[cfg(feature = "pipeline")]
fn shift_stops(stops: &mut [markdown::mdast::Stop], offset_delta: usize) {
    for stop in stops {
        stop.1 += offset_delta;
    }
}

#[cfg(feature = "pipeline")]
fn shift_attribute_stops(
    attributes: &mut [markdown::mdast::AttributeContent],
    offset_delta: usize,
) {
    use markdown::mdast::{AttributeContent, AttributeValue};
    for attribute in attributes {
        match attribute {
            AttributeContent::Expression(expression) => {
                shift_stops(&mut expression.stops, offset_delta);
            }
            AttributeContent::Property(property) => {
                if let Some(AttributeValue::Expression(expression)) = &mut property.value {
                    shift_stops(&mut expression.stops, offset_delta);
                }
            }
        }
    }
}

/// Recursively transform every node's `position` (and every embedded stop
/// list) from body-relative markdown-rs output into the export's final
/// contract: original-source coordinates (via `shift`), UTF-16 code-unit
/// `offset`/`column` (via `utf16`, applied AFTER the shift so it converts
/// against ORIGINAL-source coordinates — see [`Utf16Positions`]).
/// `_markdownRsStops` is shifted but never UTF-16-converted (stays byte-based
/// by design). Recursion depth equals mdast nesting depth — the same bound
/// `Pipeline`'s own visitors already accept for this input.
#[cfg(feature = "pipeline")]
fn shift_mdast_positions(
    node: &mut markdown::mdast::Node,
    shift: PositionShift,
    utf16: &Utf16Positions,
) {
    use markdown::mdast::Node;
    if let Some(position) = node.position_mut() {
        shift.apply(&mut position.start);
        shift.apply(&mut position.end);
        utf16.apply(&mut position.start);
        utf16.apply(&mut position.end);
    }
    let offset_delta = shift.offset_delta;
    match node {
        Node::MdxjsEsm(x) => shift_stops(&mut x.stops, offset_delta),
        Node::MdxFlowExpression(x) => shift_stops(&mut x.stops, offset_delta),
        Node::MdxTextExpression(x) => shift_stops(&mut x.stops, offset_delta),
        Node::MdxJsxFlowElement(x) => shift_attribute_stops(&mut x.attributes, offset_delta),
        Node::MdxJsxTextElement(x) => shift_attribute_stops(&mut x.attributes, offset_delta),
        _ => {}
    }
    if let Some(children) = node.children_mut() {
        for child in children {
            shift_mdast_positions(child, shift, utf16);
        }
    }
}

/// Deliberately duplicates [`prepare`]'s front half (options JSON →
/// filename gate → frontmatter extraction) MINUS `facade::build_pipeline`:
/// building the full pipeline loads the syntect theme set this raw-parse
/// entry point never uses, which would both slow every call and distort the
/// benchmark numbers this export is held to (zfb#1828). Kept as a deliberate,
/// permanent duplication rather than unified with `prepare` — merging them
/// would force `prepare` to build (and pay for) a pipeline this tier never
/// runs, or would need a new shared abstraction; the duplication is small
/// and keeps each function's cost model obvious at a glance.
#[cfg(feature = "pipeline")]
fn parse_to_ast_impl(source: &str, options_json: &str) -> ParseToAstResult {
    let fail = |frontmatter: JsonValue, diag: Diagnostic| ParseToAstResult {
        ast: None,
        frontmatter,
        diagnostics: vec![diag],
    };

    let opts: ParseToAstOptions = match serde_json::from_str(options_json) {
        Ok(o) => o,
        Err(e) => {
            let diag =
                Diagnostic::error("options", format!("invalid parseToAst options JSON: {e}"))
                    .at(Some(e.line() as u64), Some(e.column() as u64));
            return fail(JsonValue::Null, diag);
        }
    };
    let filename = opts
        .filename
        .unwrap_or_else(|| "<anonymous>.mdx".to_string());
    let extension = std::path::Path::new(&filename)
        .extension()
        .and_then(|e| e.to_str());
    if !matches!(extension, Some("md" | "mdx")) {
        let diag = Diagnostic::error(
            "options",
            format!("filename `{filename}` must end in `.md` or `.mdx` for this entry point"),
        );
        return fail(JsonValue::Null, diag);
    }
    let inferred_dialect = match extension {
        Some("md") => ParseDialect::Markdown,
        Some("mdx") => ParseDialect::Mdx,
        // Defensive duplicate of the gate above: options failures must stay
        // structured even if this resolution logic is changed later.
        _ => {
            let diag = Diagnostic::error(
                "options",
                format!("filename `{filename}` must end in `.md` or `.mdx` for this entry point"),
            );
            return fail(JsonValue::Null, diag);
        }
    };
    let dialect = opts.dialect.unwrap_or(inferred_dialect);

    let extracted = match extract_from_filename(&filename, source) {
        Ok(uf) => uf,
        Err(e) => return fail(JsonValue::Null, frontmatter_diagnostic(&e)),
    };
    let (Some(body), Some(body_offset)) = (extracted.body, extracted.body_offset) else {
        // Unreachable after the extension gate above — kept as a diagnostic
        // rather than a panic per the wasm trap contract (same as `prepare`).
        let diag = Diagnostic::error(
            "options",
            format!("filename `{filename}` did not resolve to a markdown body"),
        );
        return fail(JsonValue::Null, diag);
    };
    let frontmatter = extracted.value;
    let shift = PositionShift::for_body_at(source, body_offset);

    let mut ast = match facade::parse_mdast(
        ParseMdastOptions {
            dialect,
            gfm: opts.pipeline.gfm,
        },
        &body,
    ) {
        Ok(ast) => ast,
        Err(e) => {
            let diag = markdown_diagnostic(&e, &filename, source, &body, body_offset);
            return fail(frontmatter, diag);
        }
    };
    // `utf16` is built against the FULL original source (not `body`): the
    // shift above already moved every position into original-source byte
    // coordinates, so the UTF-16 prefix map must be indexed the same way.
    let utf16 = Utf16Positions::for_source(source);
    shift_mdast_positions(&mut ast, shift, &utf16);

    ParseToAstResult {
        ast: Some(ast),
        frontmatter,
        diagnostics: vec![],
    }
}

fn highlight_code_impl(code: &str, options_json: &str) -> HighlightCodeResult {
    let options: HighlightCodeOptions = match serde_json::from_str(options_json) {
        Ok(options) => options,
        Err(error) => {
            return HighlightCodeResult {
                html: None,
                diagnostics: vec![Diagnostic::error(
                    "options",
                    format!("invalid highlight options JSON: {error}"),
                )
                .at(Some(error.line() as u64), Some(error.column() as u64))],
            }
        }
    };

    // There is currently one intentionally closed direct-output mode. Binding
    // it in the pattern makes accepting another serialized variant a source
    // change at this boundary, rather than an accidental no-op.
    let HighlightCodeOptions {
        language,
        mode: HighlightCodeMode::Class,
        class_prefix,
        role_classes,
    } = options;

    match Highlighter::new().render_class_highlight(code, &language, &class_prefix, &role_classes) {
        Ok(outcome) => {
            let diagnostics = match outcome.fallback_reason {
                None => vec![],
                Some(ClassHighlightFallbackReason::UnknownLanguage) => vec![Diagnostic::warning(
                    "highlight",
                    format!(
                        "no bundled syntax matches language {language:?}; emitted escaped fallback markup"
                    ),
                )],
                Some(ClassHighlightFallbackReason::Tokenization) => vec![Diagnostic::warning(
                    "highlight",
                    format!(
                        "could not tokenize language {language:?}; emitted escaped fallback markup"
                    ),
                )],
            };
            HighlightCodeResult {
                html: Some(outcome.html),
                diagnostics,
            }
        }
        Err(ClassHighlightRenderError::Validation(error)) => HighlightCodeResult {
            html: None,
            diagnostics: vec![Diagnostic::error("options", error.to_string())],
        },
        Err(ClassHighlightRenderError::Highlight(error)) => HighlightCodeResult {
            html: None,
            diagnostics: vec![Diagnostic::error("internal", error.to_string())],
        },
    }
}

#[cfg(feature = "pipeline")]
fn to_json<T: Serialize>(value: &T) -> String {
    // Serialization of these result shapes cannot fail in practice, but a
    // panic here would trap the wasm instance — degrade to a hand-built
    // error document instead.
    serde_json::to_string(value).unwrap_or_else(|e| {
        format!(
            r#"{{"code":null,"html":null,"frontmatter":null,"diagnostics":[{{"severity":"error","source":"internal","message":{},"line":null,"column":null}}]}}"#,
            JsonValue::String(format!("result serialization failed: {e}"))
        )
    })
}

fn highlight_to_json(value: &HighlightCodeResult) -> String {
    // Keep this fallback on the direct API's exact result shape. A serde
    // serialization failure is an internal bug, but must still be a normal
    // JSON response rather than a wasm trap that poisons the cached instance.
    serde_json::to_string(value).unwrap_or_else(|error| {
        format!(
            r#"{{"html":null,"diagnostics":[{{"severity":"error","source":"internal","message":{},"line":null,"column":null}}]}}"#,
            JsonValue::String(format!("result serialization failed: {error}"))
        )
    })
}

/// Compile MDX source into ES-module JavaScript.
///
/// Returns a JSON string: `{ "code": string|null, "frontmatter": json,
/// "diagnostics": Diagnostic[] }` — see the crate docs for the options
/// and diagnostics shapes.
#[cfg(feature = "pipeline")]
#[wasm_bindgen]
#[must_use]
pub fn compile(source: &str, options_json: &str) -> String {
    to_json(&compile_impl(source, options_json))
}

/// Render markdown source to an HTML fragment (no SWC at runtime).
///
/// Returns a JSON string: `{ "html": string|null, "frontmatter": json,
/// "diagnostics": Diagnostic[] }` — see the crate docs for the options
/// and diagnostics shapes. Exported to JS as `renderHtml`.
#[cfg(feature = "pipeline")]
#[wasm_bindgen(js_name = renderHtml)]
#[must_use]
pub fn render_html(source: &str, options_json: &str) -> String {
    to_json(&render_html_impl(source, options_json))
}

/// Parse markdown/MDX source into a serialized RAW mdast tree — a
/// supported API tier (zfb#1857, decided GO by epic zfb#1854 / zfb#1828).
/// Exported to JS as `parseToAst`.
///
/// Returns a JSON string: `{ "ast": mdast|null, "frontmatter": json,
/// "diagnostics": Diagnostic[] }`. The tree is the raw markdown-rs parser
/// output (post-frontmatter-strip, PRE-zfb-visitors — mdast, not hast, and
/// unrecognized/custom constructs such as MDX JSX elements survive as typed
/// nodes rather than being dropped), serialized in the unist shape via
/// markdown-rs's `serde` feature, with every `position` shifted back into
/// original-source coordinates (see [`shift_mdast_positions`]). Its distinct
/// closed options document accepts only `filename`, `dialect`, and
/// `pipeline.gfm`; compile/visitor/serializer knobs are rejected rather than
/// silently ignored. A missing filename defaults to `<anonymous>.mdx`.
/// Otherwise `.md` infers Markdown and `.mdx` infers MDX; an explicit dialect
/// overrides either valid extension without waiving the extension gate.
///
/// ## Position contract: UTF-16 code units (decided zfb#1856)
///
/// `position.offset` and `position.column` are UTF-16 code-unit indices —
/// remark/unist convention, and what `String.prototype.slice`,
/// `mdast-util-to-hast`, and consumer mdast plugins already index by.
/// `position.line` is unit-agnostic and needs no conversion. This matters on
/// non-ASCII sources: a scalar value outside the Basic Multilingual Plane
/// (most emoji) is 1 UTF-16 code unit different from its UTF-8 byte width
/// (2 UTF-16 units vs. usually 4 bytes) — see [`Utf16Positions`] for the
/// conversion and `tests/parse_to_ast.rs`'s
/// `utf16_code_unit_semantics_are_pinned_on_non_ascii` for the pin. Pure-
/// ASCII sources take a fast path that skips the conversion entirely (byte
/// units already equal UTF-16 units there).
///
/// ## Documented divergences from remark-parse / remark-mdx
///
/// - `mdxJsxAttribute` records carry no `position` — markdown-rs does not
///   model attribute positions.
/// - Top-level `import`/`export` degrade to paragraphs (no `mdxjsEsm` nodes
///   — markdown-rs's `mdx_esm_parse` needs a JS ESM parser the wasm boundary
///   cannot host), and MDX expressions carry no estree data (the default
///   aggressive mode validates braces only, unlike remark-mdx's acorn pass).
///   Consumers needing remark-mdx-equivalent ESM/estree keep remark for
///   those documents.
/// - `_markdownRsStops` (on MDX expression/ESM nodes) is markdown-rs-
///   internal re-parse bookkeeping: internal, unstable, and BYTE-based (NOT
///   UTF-16 like `position`) — never slice a string with it.
#[cfg(feature = "pipeline")]
#[wasm_bindgen(js_name = parseToAst)]
#[must_use]
pub fn parse_to_ast(source: &str, options_json: &str) -> String {
    to_json(&parse_to_ast_impl(source, options_json))
}

/// Highlight arbitrary source into semantic class-mode HTML without requiring
/// a Markdown fence. Exported to JavaScript as `highlightCode`.
///
/// Returns `{ "html": string|null, "diagnostics": HighlightDiagnostic[] }`.
/// Invalid options are structured errors; an unknown non-empty language or a
/// tokenizer fallback returns escaped wrapper markup plus a warning.
#[wasm_bindgen(js_name = highlightCode)]
#[must_use]
pub fn highlight_code(code: &str, options_json: &str) -> String {
    highlight_to_json(&highlight_code_impl(code, options_json))
}

/// This package's release version, stamped by CI at compile time.
///
/// Development builds without `ZFB_RELEASE_VERSION` fall back to this crate's
/// manifest version (`CARGO_PKG_VERSION`).
#[wasm_bindgen]
#[must_use]
pub fn version() -> String {
    option_env!("ZFB_RELEASE_VERSION")
        .unwrap_or(env!("CARGO_PKG_VERSION"))
        .to_string()
}

/// Deliberately trap the current WebAssembly instance for wrapper recovery
/// tests. This is not part of the typed package API; the JavaScript wrapper
/// exposes it only through its internal test hook.
///
/// A Rust panic is the reliable cross-engine representation of the real
/// failure this hook needs to exercise. Calling a generated raw C-ABI export
/// with fabricated pointers could instead enter undefined Rust-level work and
/// hang Chromium before reaching the wasm trap boundary.
#[doc(hidden)]
#[wasm_bindgen(js_name = __forceTrapForTests)]
pub fn force_trap_for_tests() {
    panic!("zfb-md-wasm test-only forced trap");
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "pipeline")]
    use super::markdown_diagnostic;
    use super::{markdown_place_in_source, markdown_place_to_body_offset, split_place};
    #[cfg(feature = "pipeline")]
    use zfb_content::pipeline::PipelineError;

    #[test]
    fn split_place_parses_point_form() {
        let (place, rest) = split_place("3:5: unexpected end of file (markdown-rs:x)");
        assert_eq!(place, Some((3, 5)));
        assert_eq!(rest, "unexpected end of file (markdown-rs:x)");
    }

    #[test]
    fn split_place_parses_position_form() {
        let (place, rest) = split_place("1:2-3:4: something went wrong");
        assert_eq!(place, Some((1, 2)));
        assert_eq!(rest, "something went wrong");
    }

    #[test]
    fn split_place_passes_through_placeless_messages() {
        let msg = "expected ES module, got script";
        let (place, rest) = split_place(msg);
        assert_eq!(place, None);
        assert_eq!(rest, msg);
    }

    #[test]
    fn split_place_rejects_non_numeric_heads() {
        let msg = "weird: but not a place";
        let (place, rest) = split_place(msg);
        assert_eq!(place, None);
        assert_eq!(rest, msg);
    }

    #[test]
    fn diagnostic_place_transform_handles_mid_line_body_starts_and_empty_body() {
        assert_eq!(
            markdown_place_in_source("", "", 0, 1, 1),
            Some((1, 1)),
            "an empty source still has a valid EOF location"
        );

        let source = "\u{FEFF}---\ntitle: x\n---";
        let body_offset = source.len();
        assert_eq!(
            markdown_place_in_source(source, "", body_offset, 1, 1),
            Some((3, 4)),
            "the EOF body starts immediately after the closing `---`"
        );
    }

    #[test]
    fn diagnostic_place_transform_rejects_invalid_locations_without_panicking() {
        for (line, column) in [(0, 1), (1, 0), (2, 1), (1, 99), (u64::MAX, u64::MAX)] {
            assert_eq!(markdown_place_in_source("あ", "あ", 0, line, column), None);
        }
        assert_eq!(
            markdown_place_in_source("prefixbody", "body", 0, 1, 1),
            None,
            "a mismatched source/body relationship must be rejected"
        );
        assert_eq!(
            markdown_place_to_body_offset("あ", 1, 2),
            None,
            "a byte column inside a UTF-8 scalar is not safely representable"
        );
    }

    #[test]
    fn diagnostic_place_transform_accounts_for_markdown_tab_stops() {
        assert_eq!(markdown_place_to_body_offset("\tX", 1, 1), Some(0));
        assert_eq!(markdown_place_to_body_offset("\tX", 1, 4), Some(0));
        assert_eq!(markdown_place_to_body_offset("\tX", 1, 5), Some(1));
    }

    #[test]
    #[cfg(feature = "pipeline")]
    fn invalid_upstream_place_becomes_a_locationless_diagnostic() {
        let error = PipelineError::Parse("99:99: malformed location".to_string());
        let diagnostic = markdown_diagnostic(&error, "bad.mdx", "あ", "あ", 0);
        assert_eq!(diagnostic.message, "malformed location");
        assert_eq!(diagnostic.line, None);
        assert_eq!(diagnostic.column, None);
    }
}
