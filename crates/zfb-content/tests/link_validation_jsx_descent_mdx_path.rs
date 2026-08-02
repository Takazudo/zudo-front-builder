//! Path-A (MDX compile path) coverage for the zfb#2184 JSX-nested link
//! descent: `link_validation_jsx_directive_descent.rs` (zfb-md-extras)
//! pins only the `Pipeline::run_with_context` HTML path. This file
//! proves the same broken nested link reaches the pipeline's
//! markdown-diagnostics channel through the REAL JSX-emit path —
//! `mdx_to_jsx_module_with_pipeline` with armed build-context roots, the
//! same route a real build drains via
//! `Pipeline::take_markdown_diagnostics` (the idiom of
//! `directives_emit_diagnostics.rs`). On this path the collector fires
//! inside `apply_mdast_visitors_with_context` (after the resolve_links
//! application) and the hast-phase `LinkValidationPlugin` drains it in
//! `apply_hast_visitors_with_context`.

use std::collections::HashMap;

use zfb_content::mdx_jsx_emit::{mdx_to_jsx_module_with_pipeline, MdxJsxOptions};
use zfb_content::pipeline::Pipeline;
use zfb_md_ast::{
    diagnostics::MarkdownDiagnostic, DirectiveSpec, LinkValidationConfig, MarkdownFeaturesConfig,
};

fn link_validation_pipeline_with_note_directive() -> Pipeline {
    let mut directives = HashMap::new();
    directives.insert("note".to_string(), DirectiveSpec::Short("Note".to_string()));
    let features = MarkdownFeaturesConfig {
        link_validation: Some(LinkValidationConfig::default()),
        directives: Some(directives),
        ..Default::default()
    };
    let mut p = Pipeline::with_defaults_and_features(&features);
    // Arm context threading — real builds always do (zfb#944); this is
    // what routes visitor diagnostics into the pipeline's counters.
    p.set_build_context_roots("/proj".into(), "/proj/public".into());
    p
}

/// A broken bare-fragment link nested inside `<Note>…</Note>` must reach
/// the pipeline's markdown-diagnostics channel on the MDX compile path —
/// exactly one `BrokenLink`, same shape as the run_with_context path.
#[test]
fn mdx_compile_path_reports_broken_link_nested_in_mdx_jsx() {
    let mut p = link_validation_pipeline_with_note_directive();
    let opts = MdxJsxOptions::default()
        .with_filename("content/a.mdx".to_string())
        .with_source_path("/proj/content/a.mdx");
    let _jsx = mdx_to_jsx_module_with_pipeline(
        "<Note>\n\n[link](#missing-note-mdx-path)\n\n</Note>\n",
        opts,
        &mut p,
    )
    .expect("compile must succeed");

    let diags = p.take_markdown_diagnostics();
    assert_eq!(
        diags.len(),
        1,
        "the nested broken link must reach the pipeline counters: {diags:#?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "#missing-note-mdx-path"),
        "diagnostic url must be the raw href: {diags:?}"
    );
}
