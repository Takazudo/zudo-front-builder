#![cfg(not(feature = "compiler"))]

use std::path::Path;

use markdown::mdast::Node;
use zfb_content::facade::{
    build_pipeline, parse_mdast, render_html, render_mdx_jsx_module, ParseMdastOptions,
    PipelineOptions,
};
use zfb_content::frontmatter::{self, FrontmatterError};
use zfb_content::tsx_frontmatter::TsxFrontmatterError;

#[test]
fn markdown_render_raw_parse_frontmatter_and_diagnostics_remain_available() {
    let source = "---\ntitle: Compiler-free\n---\n# Hello\n\n```rust\nfn main() {}\n```\n";
    let extracted = frontmatter::extract(Path::new("page.mdx"), source)
        .expect("MDX YAML frontmatter remains available");
    assert_eq!(extracted.value["title"], "Compiler-free");

    let mut pipeline = build_pipeline(&PipelineOptions::default()).expect("pipeline builds");
    let html = render_html(
        &mut pipeline,
        extracted.body.as_deref().expect("markdown body is present"),
    )
    .expect("HTML rendering remains available");
    assert!(html.contains("<h1"));
    assert!(html.contains("Hello"));
    assert!(
        html.contains("<pre"),
        "syntect-backed code rendering remains active"
    );

    let ast = parse_mdast(ParseMdastOptions::default(), "# Hello\n")
        .expect("raw mdast parsing remains available");
    assert!(matches!(ast, Node::Root(_)));

    let invalid = "<Card {...\\bad}>body</Card>\n";
    let emitted = render_mdx_jsx_module(&mut pipeline, invalid, "invalid-spread.mdx")
        .expect("invalid spread is recovered, not fatal");
    assert!(!emitted.contains("\\bad"));
    let diagnostics = pipeline.take_markdown_diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert!(format!("{:?}", diagnostics[0]).contains("dropping invalid spread attribute"));
}

#[test]
fn conservative_expression_validator_preserves_regex_and_recovers_bare_escape() {
    let mut pipeline = build_pipeline(&PipelineOptions::default()).expect("pipeline builds");
    let emitted = render_mdx_jsx_module(
        &mut pipeline,
        "<Card ok={/[{\\d}]/.test(value)} bad={\\d} />\n",
        "expressions.mdx",
    )
    .expect("MDX expression emission remains available");
    assert!(emitted.contains("ok={/[{\\d}]/.test(value)}"));
    assert!(emitted.contains("bad={\"\\\\d\"}"));
}

#[test]
fn conservative_spread_validator_rejects_unverified_template_substitutions() {
    let mut pipeline = build_pipeline(&PipelineOptions::default()).expect("pipeline builds");
    let emitted = render_mdx_jsx_module(
        &mut pipeline,
        "<Card {...`${value}`} />\n",
        "template-spread.mdx",
    )
    .expect("the invalid spread is dropped without failing MDX emission");
    assert!(!emitted.contains("`${value}`"));
    let diagnostics = pipeline.take_markdown_diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert!(format!("{:?}", diagnostics[0]).contains("dropping invalid spread attribute"));
}

#[test]
fn tsx_frontmatter_has_an_explicit_stable_capability_error() {
    let error = frontmatter::extract(
        Path::new("page.tsx"),
        "export const frontmatter = { title: 'Unavailable' };",
    )
    .expect_err("TSX extraction must be rejected without compiler support");
    assert!(matches!(
        error,
        FrontmatterError::Tsx(TsxFrontmatterError::CompilerUnavailable { ref file })
            if file == "page.tsx"
    ));
    assert_eq!(
        error.to_string(),
        "TSX frontmatter error: page.tsx: TSX frontmatter extraction requires the `zfb-content/compiler` capability"
    );
}
