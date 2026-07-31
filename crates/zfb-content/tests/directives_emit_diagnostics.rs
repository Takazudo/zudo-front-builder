//! End-to-end proof for zfb#2206's diagnostics acceptance: directive
//! diagnostics reach the pipeline's markdown-diagnostics counters through
//! the REAL JSX-emit path (`mdx_to_jsx_module_with_pipeline` with armed
//! build-context roots — the same path a real build drains via
//! `Pipeline::take_markdown_diagnostics`), not just the registry's own
//! buffer.
//!
//! Companion positive control: the #2206 repro forms compile through the
//! same path with NO diagnostics and no literal `:::` in the emitted JSX.

use std::collections::HashMap;

use zfb_content::mdx_jsx_emit::{mdx_to_jsx_module_with_pipeline, MdxJsxOptions};
use zfb_content::pipeline::Pipeline;
use zfb_md_ast::{
    diagnostics::{DiagnosticSeverity, MarkdownDiagnostic},
    DirectiveSpec, MarkdownFeaturesConfig,
};

fn directives_pipeline() -> Pipeline {
    let mut directives = HashMap::new();
    for (name, component) in [("note", "Note"), ("warning", "Warning")] {
        directives.insert(
            name.to_string(),
            DirectiveSpec::Short(component.to_string()),
        );
    }
    let features = MarkdownFeaturesConfig {
        directives: Some(directives),
        ..Default::default()
    };
    let mut p = Pipeline::with_defaults_and_features(&features);
    // Arm context threading — real builds always do (zfb#944); this is
    // what routes visitor diagnostics into the pipeline's counters.
    p.set_build_context_roots("/proj".into(), "/proj/public".into());
    p
}

fn compile(p: &mut Pipeline, input: &str) -> String {
    let opts = MdxJsxOptions::default()
        .with_filename("content/a.mdx".to_string())
        .with_source_path("/proj/content/a.mdx");
    mdx_to_jsx_module_with_pipeline(input, opts, p).expect("compile must succeed")
}

#[test]
fn unclosed_container_reaches_pipeline_markdown_diagnostics() {
    let mut p = directives_pipeline();
    let jsx = compile(&mut p, ":::warning\nnever closed\n\nplain paragraph\n");
    // Graceful fallback: the source stays literal in the output.
    assert!(
        jsx.contains(":::warning"),
        "unclosed opener stays literal in the emitted JSX: {jsx}"
    );
    let diags = p.take_markdown_diagnostics();
    assert_eq!(
        diags.len(),
        1,
        "the unclosed diagnostic must reach the pipeline counters, got {diags:#?}"
    );
    let MarkdownDiagnostic::Generic {
        severity,
        message,
        location,
    } = &diags[0]
    else {
        unreachable!(
            "directive diagnostics surface as Generic, got {:?}",
            diags[0]
        );
    };
    assert_eq!(
        *severity,
        DiagnosticSeverity::Warning,
        "warning-only: the build prints it but never aborts"
    );
    assert!(
        message.contains("unclosed") && message.contains("warning"),
        "message names the unclosed directive, got {message:?}"
    );
    let loc = location.as_ref().expect("location attached");
    assert_eq!(
        loc.path.as_deref(),
        Some(std::path::Path::new("/proj/content/a.mdx"))
    );
    assert_eq!(loc.line, Some(1), "opener line attached");
}

#[test]
fn unclosed_opener_glued_before_valid_directive_reaches_pipeline_markdown_diagnostics() {
    // zfb#2212: an unclosed opener glued (NO blank lines) directly above
    // a valid collapsed directive — one paragraph run. The collapsed-run
    // transform emits the literal warning paragraph followed by the
    // transformed note; pre-fix only the FINAL replacement node was
    // checked for an opener shape, so the leaked `:::warning` earned no
    // unclosed diagnostic. Diagnostic-only contract: the note still
    // transforms and the warning opener still leaks literally.
    let mut p = directives_pipeline();
    let jsx = compile(&mut p, ":::warning\nnever closed\n:::note\nbody\n:::\n");
    assert!(
        jsx.contains(":::warning"),
        "unclosed opener stays literal in the emitted JSX: {jsx}"
    );
    assert!(
        jsx.contains("<Note") && !jsx.contains(":::note"),
        "the trailing note must transform, not leak: {jsx}"
    );
    let diags = p.take_markdown_diagnostics();
    assert_eq!(
        diags.len(),
        1,
        "the unclosed diagnostic must reach the pipeline counters, got {diags:#?}"
    );
    let MarkdownDiagnostic::Generic {
        severity,
        message,
        location,
    } = &diags[0]
    else {
        unreachable!(
            "directive diagnostics surface as Generic, got {:?}",
            diags[0]
        );
    };
    assert_eq!(*severity, DiagnosticSeverity::Warning);
    assert!(
        message.contains("unclosed") && message.contains("warning"),
        "message names the leaked opener, got {message:?}"
    );
    let loc = location.as_ref().expect("location attached");
    assert_eq!(
        loc.path.as_deref(),
        Some(std::path::Path::new("/proj/content/a.mdx"))
    );
}

#[test]
fn buried_unclosed_opener_reaches_pipeline_markdown_diagnostics() {
    // zfb#2211: the buried shape — a non-opener first line with the
    // unclosed opener glued mid-paragraph — through the REAL emit path.
    // Pre-fix this leaked literal ::: with no diagnostic at all (only
    // paragraph-HEAD openers warned). Diagnostic-only: the source still
    // leaks literally.
    let mut p = directives_pipeline();
    let jsx = compile(
        &mut p,
        "intro prose\n:::warning\nnever closed\n\nplain paragraph\n",
    );
    assert!(
        jsx.contains(":::warning"),
        "buried opener stays literal in the emitted JSX: {jsx}"
    );
    let diags = p.take_markdown_diagnostics();
    assert_eq!(
        diags.len(),
        1,
        "the buried unclosed diagnostic must reach the pipeline counters, got {diags:#?}"
    );
    let MarkdownDiagnostic::Generic {
        severity,
        message,
        location,
    } = &diags[0]
    else {
        unreachable!(
            "directive diagnostics surface as Generic, got {:?}",
            diags[0]
        );
    };
    assert_eq!(
        *severity,
        DiagnosticSeverity::Warning,
        "warning-only: the build prints it but never aborts"
    );
    assert!(
        message.contains("unclosed") && message.contains("warning"),
        "message names the buried opener, got {message:?}"
    );
    let loc = location.as_ref().expect("location attached");
    assert_eq!(
        loc.path.as_deref(),
        Some(std::path::Path::new("/proj/content/a.mdx"))
    );
    assert_eq!(
        loc.line,
        Some(2),
        "the buried fence LINE, not the paragraph head"
    );
}

#[test]
fn repro_forms_compile_clean_through_emit_path() {
    // The three #2206 repro forms (A: blank-line body, B: backtick title,
    // C: plain-title control) through the real emit path: all transform,
    // no literal :::, no diagnostics.
    let mut p = directives_pipeline();
    let input = "\
:::warning
alpha one

alpha two
:::

:::warning[`Evidence:` in the title]
bravo body
:::

:::warning[plain title here]
charlie body
:::
";
    let jsx = compile(&mut p, input);
    assert!(
        !jsx.contains(":::"),
        "no literal ::: anywhere in the emitted JSX: {jsx}"
    );
    assert_eq!(
        jsx.matches("<Warning").count(),
        3,
        "all three repro forms transform: {jsx}"
    );
    for body in ["alpha one", "alpha two", "bravo body", "charlie body"] {
        assert!(jsx.contains(body), "body {body:?} must survive: {jsx}");
    }
    assert!(
        jsx.contains("Evidence: in the title"),
        "backtick title normalized to plain text: {jsx}"
    );
    assert!(
        p.take_markdown_diagnostics().is_empty(),
        "well-formed forms produce no diagnostics"
    );
}
