//! `zfb-md-wasm` — zfb's md/mdx conversion pipeline as a WebAssembly module
//! (epic zfb#1572, sub-issue zfb#1576).
//!
//! Two API tiers over one cdylib, both JSON-in/JSON-out strings so the
//! wasm boundary stays trivially serializable:
//!
//! 1. [`compile`] — full mdx → JSX → SWC → ES-module JS. Returns
//!    `{ code, frontmatter, diagnostics }`.
//! 2. [`render_html`] — md → mdast → pipeline visitors → hast → HTML
//!    string, no SWC at runtime. Returns `{ html, frontmatter,
//!    diagnostics }`.
//!
//! Plus [`version`] for host-side compatibility checks.
//!
//! ## Options JSON (shared by both entry points)
//!
//! ```json
//! {
//!   "filename": "posts/hello.mdx",
//!   "jsxRuntime": "preact",
//!   "development": false,
//!   "pipeline": { "theme": null, "gfm": {}, "cjkFriendly": true,
//!                 "hardBreaks": false, "features": {} }
//! }
//! ```
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
//! `null` when the underlying error carries no location.
//!
//! ## Error / trap contract (correctness-critical — see README.md)
//!
//! Expected failures (parse errors, malformed options) come back as
//! structured diagnostics — these functions do not intentionally panic
//! on any input. A *bug*-level panic on `wasm32-unknown-unknown` traps
//! and poisons the instance; the host wrapper must re-instantiate (the
//! API is stateless per call, so re-init loses nothing). Full contract
//! in this crate's README.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use wasm_bindgen::prelude::wasm_bindgen;
use zfb_content::facade::{self, PipelineOptions};
use zfb_content::frontmatter::{extract_from_filename, FrontmatterError};
use zfb_content::pipeline::{Pipeline, PipelineError};
use zfb_render::swc_pipeline::{CompileOptions, JsxRuntime, SwcPipeline};

/// `jsxRuntime` option values, mirroring
/// [`zfb_render::swc_pipeline::JsxRuntime`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum JsxRuntimeOption {
    #[default]
    Preact,
    React,
}

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
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
struct WasmOptions {
    filename: Option<String>,
    jsx_runtime: JsxRuntimeOption,
    development: bool,
    pipeline: PipelineOptions,
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

    fn at(mut self, line: Option<u64>, column: Option<u64>) -> Self {
        self.line = line;
        self.column = column;
        self
    }
}

/// Result document of [`compile`].
#[derive(Debug, Serialize)]
struct CompileResult {
    code: Option<String>,
    frontmatter: JsonValue,
    diagnostics: Vec<Diagnostic>,
}

/// Result document of [`render_html`].
#[derive(Debug, Serialize)]
struct RenderHtmlResult {
    html: Option<String>,
    frontmatter: JsonValue,
    diagnostics: Vec<Diagnostic>,
}

/// Everything the two tiers share once options + frontmatter + pipeline
/// are resolved.
struct Prepared {
    frontmatter: JsonValue,
    body: String,
    /// Number of source lines consumed before the body starts (the
    /// frontmatter block) — added back onto body-relative parse
    /// positions so diagnostics point into the original source.
    prefix_lines: u64,
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
    let prefix_lines = source
        .get(..body_offset)
        .map(|prefix| prefix.matches('\n').count() as u64)
        .unwrap_or(0);

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
        prefix_lines,
        pipeline,
        filename,
        jsx_runtime: opts.jsx_runtime.into(),
        development: opts.development,
    })
}

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

/// Convert a [`PipelineError`] into a `"markdown"` diagnostic, shifting
/// body-relative positions back into original-source coordinates.
fn markdown_diagnostic(err: &PipelineError, filename: &str, prefix_lines: u64) -> Diagnostic {
    let PipelineError::Parse(raw) = err;
    // The MDX emit path prefixes the message with `{filename}: ` (see
    // zfb-content's mdx_jsx_emit); the HTML path does not.
    let prefix = format!("{filename}: ");
    let stripped = raw.strip_prefix(&prefix).unwrap_or(raw);
    let (place, rest) = split_place(stripped);
    match place {
        Some((line, column)) => {
            Diagnostic::error("markdown", rest).at(Some(line + prefix_lines), Some(column))
        }
        None => Diagnostic::error("markdown", stripped),
    }
}

fn compile_impl(source: &str, options_json: &str) -> CompileResult {
    let prepared = match prepare(source, options_json, "<anonymous>.mdx") {
        Ok(p) => p,
        Err(boxed) => {
            let (frontmatter, diag) = *boxed;
            return CompileResult {
                code: None,
                frontmatter,
                diagnostics: vec![diag],
            }
        }
    };
    let Prepared {
        frontmatter,
        body,
        prefix_lines,
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
                diagnostics: vec![markdown_diagnostic(&e, &filename, prefix_lines)],
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

fn render_html_impl(source: &str, options_json: &str) -> RenderHtmlResult {
    let prepared = match prepare(source, options_json, "<anonymous>.md") {
        Ok(p) => p,
        Err(boxed) => {
            let (frontmatter, diag) = *boxed;
            return RenderHtmlResult {
                html: None,
                frontmatter,
                diagnostics: vec![diag],
            }
        }
    };
    let Prepared {
        frontmatter,
        body,
        prefix_lines,
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
            diagnostics: vec![markdown_diagnostic(&e, &filename, prefix_lines)],
        },
    }
}

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

/// Compile MDX source into ES-module JavaScript.
///
/// Returns a JSON string: `{ "code": string|null, "frontmatter": json,
/// "diagnostics": Diagnostic[] }` — see the crate docs for the options
/// and diagnostics shapes.
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
#[wasm_bindgen(js_name = renderHtml)]
#[must_use]
pub fn render_html(source: &str, options_json: &str) -> String {
    to_json(&render_html_impl(source, options_json))
}

/// This crate's version (`CARGO_PKG_VERSION`), for host-side
/// compatibility checks.
#[wasm_bindgen]
#[must_use]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests {
    use super::split_place;

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
}
