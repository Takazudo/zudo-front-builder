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
