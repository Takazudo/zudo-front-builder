//! Native (rlib) integration tests for the `zfb-md-wasm` JSON API
//! (zfb#1576, epic zfb#1572).
//!
//! Test plan (declared per project testing discipline):
//! - **Level**: 1 (unit/logic). The crate is a pure `&str → String`
//!   transform; every assertion here is a JSON/string comparison against
//!   in-memory output on the host triple. No DOM, no build artifact, no
//!   process/browser.
//! - **Why level 1**: the API contract is "source + options JSON in,
//!   result JSON out" — a pure function boundary shared byte-for-byte
//!   between the rlib (tested here) and the cdylib (compiled for
//!   wasm32-unknown-unknown, verified at build level by `cargo
//!   check`/`clippy --target wasm32-unknown-unknown`).
//! - **What's covered**: both API tiers on fixtures with YAML
//!   frontmatter (values asserted, not just shape); the diagnostics
//!   contract for malformed MDX, malformed options JSON, unknown syntect
//!   theme names, YAML frontmatter errors, and non-markdown filenames;
//!   frontmatter-line-offset arithmetic on parse positions; the
//!   `jsxRuntime` switch; `version()`'s dev-build manifest fallback.
//! - **Blind spots** (explicitly NOT covered here): executing the
//!   compiled wasm artifact (Node/browser instantiation is the npm
//!   package's scope, zfb#1577); panic-freedom under adversarial fuzzed
//!   input (follow-up noted in README); runtime behaviour of the emitted
//!   JS (zfb-render's own tests cover the SWC pipeline).

use std::collections::BTreeMap;

use serde_json::Value;

#[cfg(feature = "pipeline")]
#[derive(Debug, serde::Deserialize)]
struct DiagnosticFixtureManifest {
    fixtures: Vec<DiagnosticFixture>,
}

#[cfg(feature = "pipeline")]
#[derive(Debug, serde::Deserialize)]
struct DiagnosticFixture {
    slug: String,
    source: String,
    options: Value,
    line: u64,
    column: u64,
}

#[cfg(feature = "pipeline")]
fn diagnostic_fixtures() -> Vec<DiagnosticFixture> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/parse-to-ast/diagnostics.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str::<DiagnosticFixtureManifest>(&text)
        .unwrap_or_else(|error| panic!("parsing {}: {error}", path.display()))
        .fixtures
}
use zfb_content::syntect_highlight::Highlighter;

fn parse(result: String) -> Value {
    serde_json::from_str(&result).expect("API output is always a JSON document")
}

// `compile`/`render_html` and everything only they exercise (below) are
// gated on `pipeline` (default-on, see crate docs) -- the highlight-only
// artifact drops them (zfb#1849, epic zfb#1845). `cargo test -p
// zfb-md-wasm --no-default-features` therefore runs only the
// `highlight_code`/`version` tests further down.
#[cfg(feature = "pipeline")]
const MDX_FIXTURE: &str = "---\n\
title: Hello WASM\n\
tags:\n\
\x20 - rust\n\
\x20 - wasm\n\
draft: false\n\
---\n\
\n\
> [!NOTE]\n\
> Stay on the happy path.\n\
\n\
<Card label=\"pinned\" />\n\
\n\
The answer is {6 * 7}.\n";

#[cfg(feature = "pipeline")]
const MDX_OPTIONS: &str =
    r#"{"filename":"post.mdx","pipeline":{"features":{"githubAlerts":true}}}"#;

// ── compile tier ────────────────────────────────────────────────────────────

#[cfg(feature = "pipeline")]
#[test]
fn compile_full_fixture_emits_mdx_content_module() {
    let out = parse(zfb_md_wasm::compile(MDX_FIXTURE, MDX_OPTIONS));

    let code = out["code"].as_str().expect("code is a string on success");
    assert!(
        code.contains("export default function MDXContent"),
        "compiled module must default-export MDXContent, got:\n{code}"
    );
    assert!(
        code.contains("preact/jsx-runtime"),
        "default jsxRuntime is preact, got:\n{code}"
    );
    // The PascalCase component comes from the caller's `components` prop,
    // guarded by an explicit throw.
    assert!(
        code.contains("MDX requires `Card` to be passed via the `components` prop"),
        "PascalCase component guard missing:\n{code}"
    );
    // githubAlerts rewrites `> [!NOTE]` into a `<Note>` component instead
    // of a plain blockquote.
    assert!(
        code.contains("MDX requires `Note` to be passed via the `components` prop"),
        "githubAlerts must turn the alert into a Note component:\n{code}"
    );
    assert!(
        !code.contains("blockquote"),
        "alert blockquote must be fully rewritten:\n{code}"
    );
    // The MDX expression is emitted verbatim (no constant folding).
    assert!(
        code.contains("6 * 7"),
        "expression must survive into the JS:\n{code}"
    );
    // No leftover JSX syntax.
    assert!(!code.contains("<Card"), "JSX must be desugared:\n{code}");

    // Frontmatter VALUES (not just shape).
    assert_eq!(out["frontmatter"]["title"], "Hello WASM");
    assert_eq!(out["frontmatter"]["tags"][0], "rust");
    assert_eq!(out["frontmatter"]["tags"][1], "wasm");
    assert_eq!(out["frontmatter"]["draft"], false);

    assert_eq!(out["diagnostics"].as_array().map(Vec::len), Some(0));
}

#[cfg(feature = "pipeline")]
#[test]
fn compile_react_runtime_switches_import_source() {
    let out = parse(zfb_md_wasm::compile("hello\n", r#"{"jsxRuntime":"react"}"#));
    let code = out["code"].as_str().expect("code present");
    assert!(
        code.contains("import { jsx as _jsx } from \"react/jsx-runtime\""),
        "react runtime must synthesize react/jsx-runtime imports:\n{code}"
    );
    assert_eq!(out["frontmatter"], Value::Null, "no frontmatter → null");
}

#[cfg(feature = "pipeline")]
#[test]
fn compile_malformed_mdx_returns_structured_markdown_diagnostic() {
    // Frontmatter occupies file lines 1–3; the unclosed `<Card>` element
    // makes markdown-rs fail at end of file — body line 4, file line 7.
    let src = "---\ntitle: x\n---\n<Card>\n\nsome text\n";
    let out = parse(zfb_md_wasm::compile(src, r#"{"filename":"bad.mdx"}"#));

    assert_eq!(out["code"], Value::Null);
    // Frontmatter extracted before the parse failure is still returned.
    assert_eq!(out["frontmatter"]["title"], "x");
    let diag = &out["diagnostics"][0];
    assert_eq!(diag["severity"], "error");
    assert_eq!(diag["source"], "markdown");
    assert!(
        diag["message"]
            .as_str()
            .is_some_and(|m| m.contains("closing tag")),
        "message must describe the failure: {diag}"
    );
    // Positions are shifted back into original-source coordinates
    // (markdown-rs reported body line 4; the 3 frontmatter lines are
    // added back).
    assert_eq!(diag["line"], 7);
    assert_eq!(diag["column"], 1);
}

#[cfg(feature = "pipeline")]
#[test]
fn compile_unclosed_expression_reports_shifted_position() {
    let src = "---\ntitle: x\n---\n\nvalue is {1 +\n";
    let out = parse(zfb_md_wasm::compile(src, "{}"));
    let diag = &out["diagnostics"][0];
    assert_eq!(diag["source"], "markdown");
    // Body line 2 + 3 frontmatter lines = file line 5.
    assert_eq!(diag["line"], 5);
    assert_eq!(diag["column"], 14);
}

#[cfg(feature = "pipeline")]
#[test]
fn markdown_diagnostics_use_original_source_utf16_coordinates_for_every_entrypoint() {
    for fixture in diagnostic_fixtures() {
        let options = serde_json::to_string(&fixture.options).expect("fixture options serialize");
        let outputs = [
            (
                "compile",
                parse(zfb_md_wasm::compile(&fixture.source, &options)),
            ),
            (
                "renderHtml",
                parse(zfb_md_wasm::render_html(&fixture.source, &options)),
            ),
            (
                "parseToAst",
                parse(zfb_md_wasm::parse_to_ast(&fixture.source, &options)),
            ),
        ];
        for (entrypoint, out) in outputs {
            let diag = &out["diagnostics"][0];
            assert_eq!(
                diag["source"], "markdown",
                "{} through {entrypoint}: {out}",
                fixture.slug
            );
            assert_eq!(
                diag["line"], fixture.line,
                "{} through {entrypoint}: {diag}",
                fixture.slug
            );
            assert_eq!(
                diag["column"], fixture.column,
                "{} through {entrypoint}: {diag}",
                fixture.slug
            );
        }
    }
}

// ── renderHtml tier ─────────────────────────────────────────────────────────

#[cfg(feature = "pipeline")]
#[test]
fn render_html_full_fixture_with_frontmatter() {
    let src = "---\ntitle: Doc\ncount: 3\n---\n\n# Heading\n\nSome **bold** text.\n";
    let out = parse(zfb_md_wasm::render_html(src, "{}"));

    assert_eq!(
        out["html"], "<h1>Heading</h1><p>Some <strong>bold</strong> text.</p>",
        "expected the exact serialized HTML fragment"
    );
    // Frontmatter VALUES (not just shape).
    assert_eq!(out["frontmatter"]["title"], "Doc");
    assert_eq!(out["frontmatter"]["count"], 3);
    assert_eq!(out["diagnostics"].as_array().map(Vec::len), Some(0));
}

#[cfg(feature = "pipeline")]
#[test]
fn render_html_without_frontmatter_yields_null_frontmatter() {
    let out = parse(zfb_md_wasm::render_html("# Just a heading\n", "{}"));
    assert_eq!(out["html"], "<h1>Just a heading</h1>");
    assert_eq!(out["frontmatter"], Value::Null);
}

#[cfg(feature = "pipeline")]
#[test]
fn render_html_accepts_compile_tier_options_document() {
    // One options document must be shareable across both tiers:
    // `jsxRuntime` / `development` are accepted (and ignored) here.
    let out = parse(zfb_md_wasm::render_html(
        "*hi*\n",
        r#"{"jsxRuntime":"react","development":true}"#,
    ));
    assert_eq!(out["html"], "<p><em>hi</em></p>");
    assert_eq!(out["diagnostics"].as_array().map(Vec::len), Some(0));
}

// ── options / frontmatter diagnostics ───────────────────────────────────────

#[cfg(feature = "pipeline")]
#[test]
fn non_json_options_return_options_diagnostic() {
    let out = parse(zfb_md_wasm::render_html("# hi\n", "not json"));
    assert_eq!(out["html"], Value::Null);
    let diag = &out["diagnostics"][0];
    assert_eq!(diag["source"], "options");
    assert_eq!(diag["severity"], "error");
    // For `options` diagnostics line/column refer to the options JSON.
    assert_eq!(diag["line"], 1);
}

#[cfg(feature = "pipeline")]
#[test]
fn unknown_top_level_option_is_rejected() {
    let out = parse(zfb_md_wasm::compile("# hi\n", r#"{"bogus":1}"#));
    assert_eq!(out["code"], Value::Null);
    let diag = &out["diagnostics"][0];
    assert_eq!(diag["source"], "options");
    assert!(
        diag["message"]
            .as_str()
            .is_some_and(|m| m.contains("bogus")),
        "message must name the unknown field: {diag}"
    );
}

#[cfg(feature = "pipeline")]
#[test]
fn unknown_pipeline_option_is_rejected() {
    let out = parse(zfb_md_wasm::compile(
        "# hi\n",
        r#"{"pipeline":{"bogus":1}}"#,
    ));
    let diag = &out["diagnostics"][0];
    assert_eq!(diag["source"], "options");
    assert!(
        diag["message"]
            .as_str()
            .is_some_and(|m| m.contains("bogus")),
        "message must name the unknown field: {diag}"
    );
}

#[cfg(feature = "pipeline")]
#[test]
fn unknown_theme_name_is_a_diagnostic_not_a_panic() {
    // Theme names are validated at pipeline construction (zfb#1067 /
    // zfb#1070); before zfb#1576 this path panicked through the facade's
    // `.expect` — on wasm32 that would trap and poison the instance.
    let out = parse(zfb_md_wasm::render_html(
        "# hi\n",
        r#"{"pipeline":{"theme":"NoSuchTheme"}}"#,
    ));
    assert_eq!(out["html"], Value::Null);
    let diag = &out["diagnostics"][0];
    assert_eq!(diag["source"], "options");
    assert!(
        diag["message"]
            .as_str()
            .is_some_and(|m| m.contains("NoSuchTheme")),
        "message must name the unknown theme: {diag}"
    );
}

#[cfg(feature = "pipeline")]
#[test]
fn invalid_yaml_frontmatter_returns_frontmatter_diagnostic() {
    let out = parse(zfb_md_wasm::render_html(
        "---\ntitle: [oops\n---\nbody\n",
        "{}",
    ));
    assert_eq!(out["html"], Value::Null);
    assert_eq!(out["frontmatter"], Value::Null);
    let diag = &out["diagnostics"][0];
    assert_eq!(diag["source"], "frontmatter");
    assert!(
        diag["message"]
            .as_str()
            .is_some_and(|m| m.contains("invalid YAML")),
        "message must describe the YAML failure: {diag}"
    );
    // serde_yaml reports the interruption one line past the flow sequence
    // (YAML line 2) → +1 for the opening `---` = source line 3.
    assert_eq!(diag["line"], 3);
    assert_eq!(diag["column"], 1);
}

#[cfg(feature = "pipeline")]
#[test]
fn unterminated_frontmatter_returns_frontmatter_diagnostic() {
    let out = parse(zfb_md_wasm::render_html(
        "---\ntitle: forever\nno closing\n",
        "{}",
    ));
    assert_eq!(out["html"], Value::Null);
    let diag = &out["diagnostics"][0];
    assert_eq!(diag["source"], "frontmatter");
    assert!(
        diag["message"]
            .as_str()
            .is_some_and(|m| m.contains("missing closing")),
        "message must describe the failure: {diag}"
    );
}

#[cfg(feature = "pipeline")]
#[test]
fn non_markdown_filename_is_an_options_diagnostic() {
    let out = parse(zfb_md_wasm::compile("# hi\n", r#"{"filename":"page.tsx"}"#));
    assert_eq!(out["code"], Value::Null);
    let diag = &out["diagnostics"][0];
    assert_eq!(diag["source"], "options");
    assert!(
        diag["message"]
            .as_str()
            .is_some_and(|m| m.contains(".md") && m.contains("page.tsx")),
        "message must name the offending filename and the accepted extensions: {diag}"
    );
}

// ── parseToAst tier ─────────────────────────────────────────────────────────
// The whole-tree position/UTF-16/custom-node proofs live in
// tests/parse_to_ast.rs; these cases cover the same diagnostics paths,
// shared-options-document, and non-markdown-filename gate the compile/
// renderHtml tiers above are covered for, so parseToAst's cross-cutting
// behavior isn't only proven by its own dedicated file.

#[cfg(feature = "pipeline")]
#[test]
fn parse_to_ast_malformed_mdx_returns_structured_markdown_diagnostic() {
    // Same fixture/shift arithmetic as `compile_malformed_mdx_returns_structured_markdown_diagnostic`.
    let src = "---\ntitle: x\n---\n<Card>\n\nsome text\n";
    let out = parse(zfb_md_wasm::parse_to_ast(src, r#"{"filename":"bad.mdx"}"#));

    assert_eq!(out["ast"], Value::Null);
    assert_eq!(out["frontmatter"]["title"], "x");
    let diag = &out["diagnostics"][0];
    assert_eq!(diag["severity"], "error");
    assert_eq!(diag["source"], "markdown");
    assert!(
        diag["message"]
            .as_str()
            .is_some_and(|m| m.contains("closing tag")),
        "message must describe the failure: {diag}"
    );
    assert_eq!(diag["line"], 7);
    assert_eq!(diag["column"], 1);
}

#[cfg(feature = "pipeline")]
#[test]
fn parse_to_ast_non_json_options_return_options_diagnostic() {
    let out = parse(zfb_md_wasm::parse_to_ast("# hi\n", "not json"));
    assert_eq!(out["ast"], Value::Null);
    let diag = &out["diagnostics"][0];
    assert_eq!(diag["source"], "options");
    assert_eq!(diag["severity"], "error");
    assert_eq!(diag["line"], 1);
}

#[cfg(feature = "pipeline")]
#[test]
fn parse_to_ast_unknown_top_level_option_is_rejected() {
    let out = parse(zfb_md_wasm::parse_to_ast("# hi\n", r#"{"bogus":1}"#));
    assert_eq!(out["ast"], Value::Null);
    let diag = &out["diagnostics"][0];
    assert_eq!(diag["source"], "options");
    assert!(
        diag["message"]
            .as_str()
            .is_some_and(|m| m.contains("bogus")),
        "message must name the unknown field: {diag}"
    );
}

#[cfg(feature = "pipeline")]
#[test]
fn parse_to_ast_invalid_yaml_frontmatter_returns_frontmatter_diagnostic() {
    let out = parse(zfb_md_wasm::parse_to_ast(
        "---\ntitle: [oops\n---\nbody\n",
        "{}",
    ));
    assert_eq!(out["ast"], Value::Null);
    assert_eq!(out["frontmatter"], Value::Null);
    let diag = &out["diagnostics"][0];
    assert_eq!(diag["source"], "frontmatter");
    assert!(
        diag["message"]
            .as_str()
            .is_some_and(|m| m.contains("invalid YAML")),
        "message must describe the YAML failure: {diag}"
    );
}

#[cfg(feature = "pipeline")]
#[test]
fn parse_to_ast_non_markdown_filename_is_an_options_diagnostic() {
    let out = parse(zfb_md_wasm::parse_to_ast(
        "# hi\n",
        r#"{"filename":"page.tsx"}"#,
    ));
    assert_eq!(out["ast"], Value::Null);
    let diag = &out["diagnostics"][0];
    assert_eq!(diag["source"], "options");
    assert!(
        diag["message"]
            .as_str()
            .is_some_and(|m| m.contains(".md") && m.contains("page.tsx")),
        "message must name the offending filename and the accepted extensions: {diag}"
    );
}

#[cfg(feature = "pipeline")]
#[test]
fn parse_to_ast_accepts_the_shared_options_document() {
    // One options document must be shareable across every tier: `jsxRuntime`/
    // `development` are compile-tier-only knobs that parseToAst accepts and
    // ignores, same contract as renderHtml.
    let out = parse(zfb_md_wasm::parse_to_ast(
        "# hi\n",
        r#"{"filename":"post.mdx","jsxRuntime":"react","development":true,"pipeline":{"gfm":{"table":true}}}"#,
    ));
    assert_eq!(out["ast"]["type"], "root");
    assert_eq!(out["diagnostics"].as_array().map(Vec::len), Some(0));
}

// ── direct highlightCode tier ──────────────────────────────────────────────

#[test]
fn highlight_code_matches_the_native_semantic_renderer_for_promised_languages() {
    for (language, code) in [
        ("html", "<main data-x=\"a & b\">hello</main>"),
        ("css", ".button { color: red; }"),
        ("javascript", "const value = '<tag>';"),
    ] {
        let actual = parse(zfb_md_wasm::highlight_code(
            code,
            &format!(r#"{{"language":{language:?}}}"#),
        ));
        let expected = Highlighter::new()
            .render_class_highlight(code, language, "hi-", &BTreeMap::new())
            .expect("promised language is a valid native direct render")
            .html;

        assert_eq!(
            actual["html"], expected,
            "{language} must be byte-compatible"
        );
        assert_eq!(actual["diagnostics"], Value::Array(vec![]));
        assert!(
            actual["html"]
                .as_str()
                .is_some_and(|html| html.starts_with("<pre class=\"hi-root\"><code>")),
            "direct code must receive the class renderer wrapper, never Markdown fencing: {actual}"
        );
    }
}

#[test]
fn highlight_code_applies_prefix_and_full_name_role_overrides() {
    let out = parse(zfb_md_wasm::highlight_code(
        "const answer = 42;",
        r#"{"language":"javascript","classPrefix":"token-","roleClasses":{"keyword":"text-violet-600 dark:text-violet-400"}}"#,
    ));

    let html = out["html"].as_str().expect("valid direct highlight html");
    assert!(html.starts_with("<pre class=\"token-root\"><code>"));
    assert!(html.contains("class=\"text-violet-600 dark:text-violet-400\">const</span>"));
    assert!(
        !html.contains("token-kw"),
        "the canonical full-name role override must replace the default class: {html}"
    );
    assert_eq!(out["diagnostics"], Value::Array(vec![]));
}

#[test]
fn highlight_code_unknown_language_is_escaped_markup_plus_warning() {
    let out = parse(zfb_md_wasm::highlight_code(
        "<tag>&",
        r#"{"language":"not-a-bundled-syntax"}"#,
    ));

    assert_eq!(
        out["html"],
        r#"<pre class="hi-root"><code><span class="line">&lt;tag&gt;&amp;</span></code></pre>"#
    );
    let diagnostic = &out["diagnostics"][0];
    assert_eq!(diagnostic["severity"], "warning");
    assert_eq!(diagnostic["source"], "highlight");
    assert!(
        diagnostic["message"].as_str().is_some_and(|message| message
            .contains("not-a-bundled-syntax")
            && message.contains("fallback")),
        "warning must name the fallback language and behavior: {diagnostic}"
    );
    assert_eq!(diagnostic["line"], Value::Null);
    assert_eq!(diagnostic["column"], Value::Null);
}

#[test]
fn highlight_code_accepts_empty_and_incomplete_source() {
    let empty = parse(zfb_md_wasm::highlight_code("", r#"{"language":"rust"}"#));
    assert_eq!(empty["html"], r#"<pre class="hi-root"><code></code></pre>"#);
    assert_eq!(empty["diagnostics"], Value::Array(vec![]));

    let incomplete = parse(zfb_md_wasm::highlight_code(
        "const answer =",
        r#"{"language":"javascript"}"#,
    ));
    assert!(incomplete["html"].is_string());
    assert_eq!(incomplete["diagnostics"], Value::Array(vec![]));
}

#[test]
fn highlight_code_invalid_options_are_structured_and_non_trapping() {
    for options in [
        "not json",
        r#"{}"#,
        r#"{"language":""}"#,
        r#"{"language":"rust","mode":"inline"}"#,
        r#"{"language":"rust","classPrefix":"1hi-"}"#,
        r#"{"language":"rust","roleClasses":{"kw":"text-blue-600"}}"#,
        r#"{"language":"rust","unexpected":true}"#,
    ] {
        let out = parse(zfb_md_wasm::highlight_code("let value = 1;", options));
        assert_eq!(out["html"], Value::Null, "options {options:?}");
        let diagnostic = &out["diagnostics"][0];
        assert_eq!(diagnostic["severity"], "error", "options {options:?}");
        assert_eq!(diagnostic["source"], "options", "options {options:?}");
    }

    let malformed = parse(zfb_md_wasm::highlight_code("x", "not json"));
    assert_eq!(malformed["diagnostics"][0]["line"], 1);
    assert!(malformed["diagnostics"][0]["column"].is_u64());
}

// ── version ─────────────────────────────────────────────────────────────────

#[test]
fn version_falls_back_to_cargo_pkg_version_in_dev_builds() {
    // Release builds stamp the package semver with ZFB_RELEASE_VERSION at
    // compile time. Native crate tests intentionally run with that env unset,
    // so they verify the manifest fallback without spawning a heavy rebuild.
    assert_eq!(zfb_md_wasm::version(), env!("CARGO_PKG_VERSION"));
}
