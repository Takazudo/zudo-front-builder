//! Tests for `zfb_content::mdx_to_jsx_module`.
//!
//! Coverage:
//! 1. Each mdast node category in the sub-issue's coverage list emits a
//!    JSX shape that mentions the right tag/identifier.
//! 2. Error path: malformed MDX returns `PipelineError::Parse` with
//!    line/column info and never panics.
//! 3. Smoke test: emitter output is a valid TSX module accepted by
//!    `zfb_render::SwcPipeline`.
//! 4. Real-world fixtures from zudo-doc round-trip through both stages.

use std::fs;
use std::path::PathBuf;

use zfb_content::frontmatter;
use zfb_content::pipeline::PipelineError;
use zfb_content::{mdx_to_jsx_module, MdxJsxOptions};
use zfb_render::{CompileOptions, SwcPipeline};

fn emit(src: &str) -> String {
    mdx_to_jsx_module(src, MdxJsxOptions::default()).expect("emit ok")
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("mdx-real")
}

// ───────────────────────────────────────────────────────────────────
// Per-node coverage tests
// ───────────────────────────────────────────────────────────────────

#[test]
fn module_skeleton_is_present() {
    let out = emit("hello");
    // The two pieces every module must have.
    assert!(out.contains("function _createMdxContent("));
    assert!(out.contains("export default function MDXContent(props = {})"));
    // The default `_components` map and the spread of overrides.
    assert!(out.contains("const _components = {"));
    assert!(out.contains("...components,"));
    // Always wraps the body in a React Fragment.
    assert!(out.contains("import { Fragment as _Fragment } from \"react/jsx-runtime\""));
    assert!(out.contains("<_Fragment>"));
    assert!(out.contains("</_Fragment>"));
}

#[test]
fn headings_h1_through_h6_emit_components() {
    for depth in 1..=6 {
        let src = format!("{} h{}\n", "#".repeat(depth), depth);
        let out = emit(&src);
        let tag = format!("h{depth}");
        assert!(
            out.contains(&format!("{tag}: \"{tag}\",")),
            "depth {depth}: missing default-tag entry; got:\n{out}"
        );
        assert!(
            out.contains(&format!("<_components.{tag}>")),
            "depth {depth}: missing open tag; got:\n{out}"
        );
        assert!(
            out.contains(&format!("</_components.{tag}>")),
            "depth {depth}: missing close tag; got:\n{out}"
        );
    }
}

#[test]
fn paragraph_emits_p_with_text() {
    let out = emit("hello world");
    assert!(out.contains("<_components.p>{\"hello world\"}</_components.p>"));
}

#[test]
fn emphasis_strong_inline_code() {
    let out = emit("a *em* b **strong** c `code` d");
    assert!(out.contains("<_components.em>"));
    assert!(out.contains("<_components.strong>"));
    assert!(out.contains("<_components.code>{\"code\"}</_components.code>"));
}

#[test]
fn fenced_code_preserves_lang_and_meta() {
    let out = emit("```rust title=\"main.rs\"\nfn main() {}\n```\n");
    assert!(out.contains("<_components.pre>"));
    assert!(out.contains("<_components.code"));
    assert!(out.contains("className=\"language-rust\""));
    assert!(out.contains("data-lang=\"rust\""));
    // `meta` is preserved — including embedded quote escapes.
    assert!(
        out.contains("data-meta=\"title=&quot;main.rs&quot;\""),
        "meta attr missing or unescaped:\n{out}"
    );
    // The body is in a JS string literal, so the source is escaped.
    assert!(out.contains("\"fn main() {}\""));
}

#[test]
fn link_with_and_without_title() {
    let out = emit("[a](https://example.com)");
    assert!(out.contains("<_components.a href=\"https://example.com\">{\"a\"}</_components.a>"));

    let out = emit("[a](https://example.com \"hi\")");
    assert!(out.contains("href=\"https://example.com\""));
    assert!(out.contains("title=\"hi\""));
}

#[test]
fn image_self_closes_with_attrs() {
    let out = emit("![alt text](pic.png)");
    assert!(out.contains("<_components.img src=\"pic.png\" alt=\"alt text\" />"));

    let out = emit("![alt](pic.png \"caption\")");
    assert!(out.contains("title=\"caption\""));
}

#[test]
fn ordered_and_unordered_lists() {
    let out = emit("- a\n- b\n");
    assert!(out.contains("<_components.ul>"));
    assert!(out.contains("<_components.li>"));

    let out = emit("1. one\n2. two\n");
    assert!(out.contains("<_components.ol>"));

    // Custom start.
    let out = emit("3. three\n4. four\n");
    assert!(
        out.contains("start={3}"),
        "expected start={{3}} attr in:\n{out}"
    );
}

#[test]
fn blockquote_wraps_children() {
    let out = emit("> hi\n");
    assert!(out.contains("<_components.blockquote>"));
}

#[test]
fn thematic_break_and_break_are_void() {
    let out = emit("---\n");
    assert!(out.contains("<_components.hr />"));
    // A trailing two spaces produces a hard break inside a paragraph.
    let out = emit("a  \nb\n");
    assert!(out.contains("<_components.br />"));
}

#[test]
fn delete_strikethrough_via_synthetic_node() {
    // GFM strikethrough is not enabled by `ParseOptions::mdx()`, but
    // `Delete` is a real mdast node. Round-trip a paragraph that
    // contains a manually wrapped `<del>` MDX-JSX element to confirm
    // the lowercase JSX-element path still routes through `_components`.
    let out = emit("text with <del>gone</del> here");
    assert!(out.contains("<_components.del>"));
    assert!(out.contains("</_components.del>"));
}

#[test]
fn html_literal_path_is_exercised_by_unit_tests() {
    // Under MDX parse options, raw HTML is disabled in favour of JSX
    // (HTML comments are even a parse error — markdown-rs suggests
    // `{/* text */}` instead). So `MdastNode::Html` never appears
    // through the public `mdx_to_jsx_module(str)` entry point.
    //
    // The dangerouslySetInnerHTML emission path is covered by the
    // in-crate unit test `html_node_emits_dangerously_set_inner_html`
    // in `src/mdx_jsx_emit.rs`, which feeds a synthetic `Html` node
    // straight into the emitter. This test documents the rationale
    // here so future readers don't try to add a string-input case
    // that markdown-rs will reject.
    let res = mdx_to_jsx_module("<!-- a -->\n", MdxJsxOptions::default());
    assert!(
        res.is_err(),
        "MDX rejects HTML comments — keep this assertion as the contract pin"
    );
}

#[test]
fn mdx_jsx_pascal_case_components_require_prop() {
    let out = emit("<Note title=\"hi\">body</Note>\n");
    // Pascal-case identifier preamble.
    assert!(out.contains("const Note = _components.Note ?? components.Note;"));
    assert!(out.contains(
        "if (!Note) throw new Error(\"MDX requires `Note` to be passed via the `components` prop\");",
    ));
    // Used in body with original attrs.
    assert!(out.contains("<Note title=\"hi\">"));
    assert!(out.contains("</Note>"));
}

#[test]
fn mdx_jsx_self_closing() {
    let out = emit("<Hr />\n");
    assert!(out.contains("const Hr = _components.Hr"));
    assert!(out.contains("<Hr />"));
}

#[test]
fn mdx_jsx_lowercase_routes_through_components() {
    // Lowercase MDX JSX (e.g. <span>) goes through _components.span,
    // matching the override-anything contract of the canonical shape.
    let out = emit("<span class=\"x\">hi</span>\n");
    assert!(out.contains("<_components.span class=\"x\">"));
    assert!(out.contains("</_components.span>"));
}

#[test]
fn mdx_attr_expression_survives_verbatim() {
    let out = emit("<Note count={1 + 2}>x</Note>\n");
    assert!(out.contains("count={1 + 2}"));
}

#[test]
fn mdx_spread_attribute_preserved() {
    let out = emit("<Note {...rest}>x</Note>\n");
    assert!(
        out.contains("{...rest}"),
        "expected spread attr verbatim:\n{out}"
    );
}

#[test]
fn mdx_text_expression_passes_through() {
    let out = emit("hello {1 + 1} world\n");
    // `{1 + 1}` should be visible verbatim in the output.
    assert!(
        out.contains("{1 + 1}"),
        "expected text expression verbatim:\n{out}"
    );
}

#[test]
fn mdx_flow_expression_passes_through() {
    // A flow expression sits at block level (its own line, no
    // surrounding paragraph).
    let out = emit("{1 + 1}\n");
    assert!(out.contains("{1 + 1}"));
}

#[test]
fn text_escapes_quotes_and_backslashes() {
    let out = emit(r#"a "b" \ c"#);
    // Inside the JS string literal: " → \", \ → \\
    assert!(out.contains(r#""a \"b\" \\ c""#));
}

#[test]
fn default_components_are_alphabetised_and_unique() {
    // Use a repeated tag (p) to make sure we don't emit duplicates.
    let out = emit("# H1\n\npara\n\npara2\n\n## H2\n");
    let body_start = out.find("const _components = {").unwrap();
    let body_end = out[body_start..].find("...components,").unwrap();
    let block = &out[body_start..body_start + body_end];

    let lines: Vec<&str> = block.lines().filter(|l| l.contains(": \"")).collect();
    let names: Vec<&str> = lines
        .iter()
        .map(|l| l.trim().split(':').next().unwrap())
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "tags must be alphabetised in:\n{block}");
    // No duplicates.
    let mut dedup = sorted.clone();
    dedup.dedup();
    assert_eq!(sorted, dedup, "tags must be unique in:\n{block}");
}

// ───────────────────────────────────────────────────────────────────
// ESM import stripping (B-14-5 regression tests)
//
// markdown-rs requires `mdx_esm_parse` to be Some for the MdxjsEsm
// construct to activate. Without it, import statements fall through
// to Paragraph parsing: `import { Foo } from "pkg"` becomes a
// paragraph where `{ Foo }` is an MdxTextExpression that evaluates to
// the `Foo` identifier (undefined at runtime → empty string in the
// rendered output, leaving the double-space skeleton visible).
// ───────────────────────────────────────────────────────────────────

/// Default import (`import X from 'pkg'`) must be stripped from output
/// and must not appear inside the rendered JSX body.
#[test]
fn esm_default_import_is_stripped() {
    let out = emit("import PresetGenerator from '@/components/preset-generator';\n\nhello\n");
    // The import keyword and module specifier must NOT leak into the body.
    assert!(
        !out.contains("import PresetGenerator"),
        "default import leaked into JSX output:\n{out}"
    );
    assert!(
        !out.contains("components/preset-generator"),
        "module path leaked into JSX output:\n{out}"
    );
    // The rest of the document still renders.
    assert!(out.contains("\"hello\""), "body text missing:\n{out}");
}

/// Named import (`import { X } from 'pkg'`) must be stripped, not rendered
/// as a paragraph. Before the fix, `{ X }` was parsed as an MdxTextExpression
/// inside a Paragraph, producing `import  from "pkg"` (double-space) in the
/// rendered output.
#[test]
fn esm_named_import_is_stripped() {
    let out = emit("import { Island } from \"@takazudo/zfb\";\n\nhello\n");
    // The source import must not appear in the JSX body. Check for the
    // module specifier (not just "import" which also appears in the React
    // preamble `import { Fragment as _Fragment } from "react/jsx-runtime"`).
    assert!(
        !out.contains("@takazudo/zfb"),
        "module specifier leaked into JSX output:\n{out}"
    );
    // The "import " text (with trailing space as it appears in the source
    // paragraph form) must not appear in the rendered body.
    assert!(
        !out.contains("\"import "),
        "import text leaked as JS string literal:\n{out}"
    );
    // Body still renders.
    assert!(out.contains("\"hello\""), "body text missing:\n{out}");
}

/// Two consecutive imports (default then named) must both be stripped.
/// This is the exact pattern from /docs/getting-started/setup-preset-generator.
#[test]
fn esm_two_consecutive_imports_are_stripped() {
    let src = "import PresetGenerator from '@/components/preset-generator';\nimport { Island } from \"@takazudo/zfb\";\n\nhello\n";
    let out = emit(src);
    // Neither module specifier must appear in the output body.
    assert!(
        !out.contains("preset-generator"),
        "preset-generator module path leaked:\n{out}"
    );
    assert!(
        !out.contains("@takazudo/zfb"),
        "@takazudo/zfb module specifier leaked:\n{out}"
    );
    // The import text must not appear as a JS string literal (which would
    // indicate it was emitted inside a paragraph).
    assert!(
        !out.contains("\"import "),
        "import text leaked as JS string literal:\n{out}"
    );
    // Body still renders.
    assert!(out.contains("\"hello\""), "body text missing:\n{out}");
}

/// ESM export statements must also be stripped (export const, export default, etc.).
#[test]
fn esm_export_statement_is_stripped() {
    let out = emit("export const version = '1.0.0';\n\nhello\n");
    assert!(
        !out.contains("export const version"),
        "export statement leaked:\n{out}"
    );
    assert!(out.contains("\"hello\""), "body text missing:\n{out}");
}

// ───────────────────────────────────────────────────────────────────
// Error-path tests
// ───────────────────────────────────────────────────────────────────

#[test]
fn unterminated_jsx_returns_parse_error_not_panic() {
    // An MDX flow JSX element with an unterminated expression
    // attribute is the canonical "unrecoverable parse" case for
    // markdown-rs.
    let res = mdx_to_jsx_module(
        "<Note attr={\n",
        MdxJsxOptions::default().with_filename("bad.mdx"),
    );
    let err = res.expect_err("expected parse error");
    let PipelineError::Parse(msg) = err;
    // Filename surfaces.
    assert!(msg.contains("bad.mdx"), "filename missing in: {msg}");
    // markdown-rs's Display includes the line:col-line:col span up
    // front, so we expect a digit somewhere in the message.
    assert!(
        msg.chars().any(|c| c.is_ascii_digit()),
        "expected line/column info in: {msg}"
    );
}

#[test]
fn unbalanced_brace_expression_returns_parse_error() {
    // `{` without matching `}` is invalid MDX.
    let res = mdx_to_jsx_module("text {unbalanced\n", MdxJsxOptions::default());
    let err = res.expect_err("expected parse error");
    let PipelineError::Parse(msg) = err;
    assert!(!msg.is_empty(), "error message must not be empty");
}

// ───────────────────────────────────────────────────────────────────
// Smoke test — emitter output compiles via SWC
// ───────────────────────────────────────────────────────────────────

/// Compile `jsx_module` through SWC's TSX pass and assert the output
/// is non-empty JS that contains the canonical `MDXContent` export.
fn assert_swc_accepts(jsx_module: &str, label: &str) {
    let pipeline = SwcPipeline::new();
    let opts = CompileOptions::default().with_filename(format!("{label}.tsx"));
    let compiled = pipeline.compile(jsx_module, &opts).unwrap_or_else(|e| {
        panic!("SWC rejected emitter output for {label}: {e}\n--- src ---\n{jsx_module}")
    });
    assert!(
        !compiled.code.is_empty(),
        "SWC produced empty code for {label}"
    );
    // The default export survives the JSX transform.
    assert!(
        compiled.code.contains("MDXContent"),
        "compiled output missing MDXContent for {label}:\n{}",
        compiled.code
    );
    // JSX is fully desugared (no leftover open-tag syntax).
    assert!(
        !compiled.code.contains("<_Fragment>"),
        "JSX leaked through SWC for {label}:\n{}",
        compiled.code
    );
}

#[test]
fn smoke_simple_markdown_compiles_via_swc() {
    let jsx = emit("# Hello\n\nFoo *bar* baz.\n");
    assert_swc_accepts(&jsx, "simple");
}

#[test]
fn smoke_with_jsx_components_compiles_via_swc() {
    let jsx = emit("# Hello\n\n<Note title=\"hi\">body</Note>\n");
    assert_swc_accepts(&jsx, "with-jsx");
}

#[test]
fn smoke_self_closing_jsx_compiles_via_swc() {
    let jsx = emit("<Hr />\n\nafter\n");
    assert_swc_accepts(&jsx, "self-closing");
}

// ───────────────────────────────────────────────────────────────────
// Real-world fixtures from zudo-doc
// ───────────────────────────────────────────────────────────────────

fn fixture_files() -> Vec<PathBuf> {
    let dir = fixture_dir();
    let mut out: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read fixture dir {dir:?}: {e}"))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "mdx"))
        .collect();
    // Stable order across platforms.
    out.sort();
    assert!(
        out.len() >= 2,
        "expected at least 2 real-world MDX fixtures in {dir:?}, found {}",
        out.len()
    );
    out
}

#[test]
fn real_world_fixtures_round_trip_through_emitter() {
    for path in fixture_files() {
        let raw = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
        // Strip frontmatter — the emitter expects body-only input.
        let uf = frontmatter::extract(&path, &raw)
            .unwrap_or_else(|e| panic!("frontmatter parse {path:?}: {e}"));
        let body = uf
            .body
            .unwrap_or_else(|| panic!("md/mdx fixture must have a body: {path:?}"));
        let label = path.file_name().unwrap().to_string_lossy().into_owned();
        let jsx = mdx_to_jsx_module(
            &body,
            MdxJsxOptions::default().with_filename(label.clone()),
        )
        .unwrap_or_else(|e| panic!("emit {label}: {e}"));
        assert!(
            jsx.contains("export default function MDXContent"),
            "{label}: missing default export"
        );
        // Each fixture should also pass the SWC smoke test.
        assert_swc_accepts(&jsx, &label);
    }
}
