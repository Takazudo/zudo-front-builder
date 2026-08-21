//! Focused pins for the independently buildable wasm capabilities.
//!
//! Each singleton invocation runs these assertions without relying on
//! workspace feature unification. The default `pipeline` alias additionally
//! proves the historical four-call composition remains available.

#[cfg(any(feature = "render", feature = "parse"))]
use serde_json::json;
#[cfg(any(feature = "render", feature = "parse", feature = "highlight"))]
use serde_json::Value;

#[cfg(any(feature = "render", feature = "parse", feature = "highlight"))]
fn parse(result: String) -> Value {
    serde_json::from_str(&result).expect("capability output is valid JSON")
}

#[cfg(feature = "render")]
#[test]
fn render_singleton_pins_result_diagnostic_and_shared_options() {
    let success = parse(zfb_md_wasm::render_html(
        "---\ntitle: Slim\n---\n# Hello\n",
        r#"{"jsxRuntime":"react","development":true}"#,
    ));
    assert_eq!(
        success,
        json!({
            "html": "<h1>Hello</h1>",
            "frontmatter": { "title": "Slim" },
            "diagnostics": [],
        })
    );

    let failure = parse(zfb_md_wasm::render_html("# Hello\n", "not json"));
    assert_eq!(failure["html"], Value::Null);
    assert_eq!(failure["frontmatter"], Value::Null);
    assert_eq!(failure["diagnostics"][0]["severity"], "error");
    assert_eq!(failure["diagnostics"][0]["source"], "options");
    assert_eq!(failure["diagnostics"][0]["line"], 1);
}

#[cfg(feature = "render")]
#[test]
fn render_html_preserves_raw_html_by_unsanitized_contract() {
    // Trust intent: renderHtml is deliberately not a sanitizer. Raw author
    // HTML stays in the result for parity; callers must sanitize separately
    // when rendering untrusted content.
    let source = r#"<script type="application/x-zfb-security">globalThis.__zfbSlimSecurityProbe = 1</script>
"#;
    let out = parse(zfb_md_wasm::render_html(source, r#"{"filename":"raw.md"}"#));
    assert_eq!(out["diagnostics"], json!([]));
    let html = out["html"].as_str().expect("raw HTML renders successfully");
    assert!(
        html.contains("<script"),
        "raw HTML must not be sanitized: {html}"
    );
    assert!(
        html.contains("globalThis.__zfbSlimSecurityProbe = 1"),
        "raw script text must remain present: {html}"
    );
}

#[cfg(feature = "parse")]
#[test]
fn parse_singleton_pins_result_and_structured_diagnostic() {
    let success = parse(zfb_md_wasm::parse_to_ast(
        "---\ntitle: Slim\n---\n# Hello\n",
        r#"{"filename":"post.md"}"#,
    ));
    assert_eq!(success["frontmatter"], json!({ "title": "Slim" }));
    assert_eq!(success["diagnostics"], json!([]));
    assert_eq!(success["ast"]["type"], "root");
    assert_eq!(success["ast"]["children"][0]["type"], "heading");
    assert_eq!(
        success["ast"]["children"][0]["children"][0]["value"],
        "Hello"
    );

    let failure = parse(zfb_md_wasm::parse_to_ast("# Hello\n", "not json"));
    assert_eq!(failure["ast"], Value::Null);
    assert_eq!(failure["frontmatter"], Value::Null);
    assert_eq!(failure["diagnostics"][0]["severity"], "error");
    assert_eq!(failure["diagnostics"][0]["source"], "options");
    assert_eq!(failure["diagnostics"][0]["line"], 1);
}

#[cfg(feature = "parse")]
#[test]
fn parse_mdx_author_syntax_is_inert_data() {
    // Trust intent: parse returns MDX syntax as data. No slim parse path may
    // compile, import, evaluate, or execute author-supplied JavaScript.
    let source = concat!(
        "<Widget value={globalThis.__zfbSlimSecurityProbe = 1} />\n\n",
        "{globalThis.__zfbSlimSecurityProbe = 2}\n\n",
        "export const dangerous = globalThis.__zfbSlimSecurityProbe = 3\n"
    );
    let out = parse(zfb_md_wasm::parse_to_ast(
        source,
        r#"{"filename":"inert.mdx"}"#,
    ));
    assert_eq!(out["diagnostics"], json!([]));
    let ast = out["ast"].to_string();
    assert!(
        ast.contains("mdxJsxFlowElement"),
        "JSX must remain data: {ast}"
    );
    assert!(
        ast.contains("mdxFlowExpression"),
        "expression must remain data: {ast}"
    );
    assert!(
        ast.contains("globalThis.__zfbSlimSecurityProbe"),
        "author source must remain inert text: {ast}"
    );
    // markdown-rs intentionally keeps top-level ESM source as inert text at
    // this boundary; it must never become an executable compile surface.
    assert!(
        ast.contains("export const dangerous"),
        "ESM text must survive: {ast}"
    );
}

#[cfg(feature = "highlight")]
#[test]
fn highlight_singleton_pins_result_and_fallback_diagnostic() {
    let result = parse(zfb_md_wasm::highlight_code(
        "<tag>&",
        r#"{"language":"unknown-capability-language"}"#,
    ));
    assert_eq!(
        result["html"],
        r#"<pre class="hi-root"><code><span class="line">&lt;tag&gt;&amp;</span></code></pre>"#
    );
    assert_eq!(result["diagnostics"][0]["severity"], "warning");
    assert_eq!(result["diagnostics"][0]["source"], "highlight");
}

#[cfg(feature = "compile")]
#[test]
fn compile_capability_composes_with_render() {
    assert!(parse(zfb_md_wasm::compile("# Hello\n", "{}"))["code"].is_string());
    assert_eq!(
        parse(zfb_md_wasm::render_html("# Hello\n", "{}"))["html"],
        "<h1>Hello</h1>"
    );
}

#[cfg(feature = "pipeline")]
#[test]
fn pipeline_alias_composes_all_four_calls() {
    assert!(parse(zfb_md_wasm::compile("# Hello\n", "{}"))["code"].is_string());
    assert!(parse(zfb_md_wasm::render_html("# Hello\n", "{}"))["html"].is_string());
    assert!(parse(zfb_md_wasm::parse_to_ast("# Hello\n", "{}"))["ast"].is_object());
    assert!(parse(zfb_md_wasm::highlight_code(
        "const answer = 42;",
        r#"{"language":"javascript"}"#,
    ))["html"]
        .is_string());
}
