//! Path-A (MDX compile path) coverage for the zfb#2249 JSX-nested
//! external-links pass — `JsxNestedExternalLinks`, implemented in
//! `crates/zfb-content/src/plugins/external_links.rs`.
//!
//! Mirrors `link_validation_jsx_descent_mdx_path.rs`'s template: proves
//! the mdast-phase visitor's effect reaches the REAL JSX-emit path
//! (`mdx_to_jsx_module_with_pipeline`), not just an isolated mdast tree —
//! including its interaction with directive expansion, `resolve_links`
//! ordering, link validation, and the footnote-definition partition. The
//! plugin's pure Level-1 unit coverage lives beside the visitor itself,
//! in `external_links.rs`'s own `#[cfg(test)]` module.

use std::collections::HashMap;
use std::path::PathBuf;

use zfb_content::mdx_jsx_emit::{mdx_to_jsx_module_with_pipeline, MdxJsxOptions};
use zfb_content::pipeline::{Pipeline, ResolvedGfmConstructs};
use zfb_content::ExternalLinksConfig;
use zfb_md_ast::diagnostics::MarkdownDiagnostic;
use zfb_md_ast::{DirectiveSpec, LinkValidationConfig, MarkdownFeaturesConfig};

fn pipeline_with_external_links(directives: bool, link_validation: bool) -> Pipeline {
    let mut directive_map = HashMap::new();
    if directives {
        directive_map.insert("note".to_string(), DirectiveSpec::Short("Note".to_string()));
    }
    let features = MarkdownFeaturesConfig {
        directives: if directives {
            Some(directive_map)
        } else {
            None
        },
        link_validation: if link_validation {
            Some(LinkValidationConfig::default())
        } else {
            None
        },
        ..Default::default()
    };
    let mut p = Pipeline::with_defaults_and_features(&features);
    p.add_external_links(ExternalLinksConfig::default(), None);
    // Arm context threading — real builds always do (zfb#944); this is
    // what routes visitor diagnostics into the pipeline's counters and
    // is required for `LinkValidationPlugin` to fire at all.
    p.set_build_context_roots("/proj".into(), "/proj/public".into());
    p
}

/// Spec item 1 — A JSX-nested external link inside a literal MDX-JSX element
/// (`<Note>…</Note>`) gets `target`/`rel` in the compiled module, in the
/// exact attribute order the spec pins: `href`, `target`, `rel` (no
/// `title` here — see test 5 in the unit suite for the `title` case).
#[test]
fn mdx_jsx_element_nested_external_link_is_treated() {
    let mut p = pipeline_with_external_links(false, false);
    let opts = MdxJsxOptions::default()
        .with_filename("content/a.mdx".to_string())
        .with_source_path("/proj/content/a.mdx");
    let jsx = mdx_to_jsx_module_with_pipeline(
        "<Note>\n\n[ext](https://other.com)\n\n</Note>\n",
        opts,
        &mut p,
    )
    .expect("compile must succeed");

    assert!(
        jsx.contains(
            r#"<_components.a href="https://other.com" target="_blank" rel="noopener noreferrer">"#
        ),
        "expected the treated nested external link's exact attribute order: {jsx}"
    );
}

/// Spec item 2 — Same coverage through a `:::note` container directive — directive
/// expansion (`DirectiveRegistry`) produces an ordinary `MdxJsxFlowElement`
/// in the mdast phase, before this pass runs, so it is indistinguishable
/// from a literal `<Note>` from this visitor's perspective.
#[test]
fn directive_expanded_note_nested_external_link_is_treated() {
    let mut p = pipeline_with_external_links(true, false);
    let opts = MdxJsxOptions::default()
        .with_filename("content/a.mdx".to_string())
        .with_source_path("/proj/content/a.mdx");
    let jsx = mdx_to_jsx_module_with_pipeline(
        ":::note\n\n[ext](https://other.com)\n\n:::\n",
        opts,
        &mut p,
    )
    .expect("compile must succeed");

    assert!(
        jsx.contains(
            r#"<_components.a href="https://other.com" target="_blank" rel="noopener noreferrer">"#
        ),
        "a directive-expanded :::note body must treat its nested external link \
         identically to a literal <Note>: {jsx}"
    );
}

/// Spec item 3 — Behavioral post-`resolve_links` ordering pin: a JSX-nested
/// INTERNAL link is left untouched by `JsxNestedExternalLinks` (no
/// `target`/`rel`) but IS resolve_links-rewritten — the emitted `href`
/// reflects the resolved site URL, not the raw `./page.md` spelling.
/// This only holds because the pipeline applies `JsxNestedExternalLinks`
/// AFTER `resolve_links` (the #2247 slot ordering) — see the field docs
/// on `Pipeline::jsx_nested_external_links`.
#[test]
fn jsx_nested_internal_link_is_resolve_links_rewritten_and_untouched_by_this_pass() {
    let mut p = pipeline_with_external_links(false, false);
    let mut source_map = HashMap::new();
    source_map.insert(PathBuf::from("./page.md"), "/page/".to_string());
    p.add_resolve_links(source_map);

    let opts = MdxJsxOptions::default()
        .with_filename("content/a.mdx".to_string())
        .with_source_path("/proj/content/a.mdx");
    let jsx =
        mdx_to_jsx_module_with_pipeline("<Note>\n\n[x](./page.md)\n\n</Note>\n", opts, &mut p)
            .expect("compile must succeed");

    assert!(
        jsx.contains(r#"href="/page/""#),
        "the nested internal link must carry resolve_links' rewritten href: {jsx}"
    );
    assert!(
        !jsx.contains("target=\"_blank\""),
        "an internal (resolved) nested link must not be treated as external: {jsx}"
    );
}

/// Spec item 6 — Link-validation non-regression — the behavioral post-collector
/// ordering pin: a broken nested internal link BESIDE a treated nested
/// external link still produces exactly its own diagnostic (the
/// external link is skipped by `validate_link`, and mutating it into an
/// `MdxJsxTextElement` afterward does not disturb the collector, which
/// already ran and collected both candidates before this pass mutates
/// the tree — see `Pipeline::apply_mdast_visitors_with_context`'s
/// ordering).
#[test]
fn broken_nested_internal_link_beside_treated_external_link_still_diagnoses() {
    let mut p = pipeline_with_external_links(false, true);
    let opts = MdxJsxOptions::default()
        .with_filename("content/a.mdx".to_string())
        .with_source_path("/proj/content/a.mdx");
    let jsx = mdx_to_jsx_module_with_pipeline(
        "<Note>\n\n[broken](#missing-nested-ext-sibling)\n\n[ext](https://other.com)\n\n</Note>\n",
        opts,
        &mut p,
    )
    .expect("compile must succeed");

    // The external link was still treated (proves the two passes
    // coexist without either one suppressing the other).
    assert!(
        jsx.contains("target=\"_blank\""),
        "the sibling external link must still be treated: {jsx}"
    );

    let diags = p.take_markdown_diagnostics();
    assert_eq!(
        diags.len(),
        1,
        "exactly the broken internal link must diagnose, not the external one: {diags:#?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "#missing-nested-ext-sibling"),
        "diagnostic must name the broken internal link's raw href: {diags:?}"
    );
}

/// Spec item 7 — Partition pin: an external link inside a footnote-definition body
/// authored under a JSX ancestor is treated EXACTLY ONCE — by the
/// hast-phase `ExternalLinksPlugin`, in the document-level rendered
/// footnote section (structurally hast-visible regardless of where the
/// definition was authored) — not by `JsxNestedExternalLinks`, which
/// resets JSX-nested tracking at the `FootnoteDefinition` boundary
/// (`rewrite_jsx_nested`'s documented contract). A regression that made
/// BOTH passes treat it would double the `target="_blank"` occurrence
/// count; a regression that made NEITHER treat it would drop it to zero.
#[test]
fn footnote_definition_external_link_under_jsx_is_treated_exactly_once() {
    let mut p = Pipeline::with_resolved_gfm_constructs(ResolvedGfmConstructs::ALL_ON);
    p.add_external_links(ExternalLinksConfig::default(), None);

    let jsx = mdx_to_jsx_module_with_pipeline(
        "<Note>\n\nRef[^a] end.\n\n[^a]: [ext](https://other.com)\n\n</Note>\n",
        MdxJsxOptions::default(),
        &mut p,
    )
    .expect("compile must succeed");

    let treated_count = jsx.matches("target=\"_blank\"").count();
    assert_eq!(
        treated_count, 1,
        "a footnote-definition-nested external link under JSX must be treated \
         exactly once (by the rendered footnote section's hast plugin), not \
         zero (dropped) or two (double-treated): {jsx}"
    );
    assert!(
        jsx.contains(r#"href="https://other.com""#),
        "the treated link's href must survive: {jsx}"
    );
}

/// Regression pin (codex review of #2249): a JSX-nested HEADING whose
/// text is (or contains) an external link must keep its plain-text
/// projection — `headings[i].text`/`.slug` in the module export AND the
/// rendered `id`/hash-link anchor on the `<hN>` itself — after
/// `JsxNestedExternalLinks` replaces the nested `Link` with an `<a>`
/// `MdxJsxTextElement`. Both `collect_headings` (via `mdast_inline_text`)
/// and the JSX-nested heading's own renderer share that same text
/// projection (`mdx_jsx_emit.rs`), so this one pin covers both call
/// sites: before the fix, the synthesized JSX anchor fell through the
/// generic "JSX contributes nothing to TOCs" catch-all and silently
/// emptied the heading's text/slug/id/anchor.
#[test]
fn jsx_nested_heading_containing_a_treated_external_link_keeps_its_text_and_anchor() {
    let mut p = pipeline_with_external_links(false, false);
    let opts = MdxJsxOptions::default()
        .with_filename("content/a.mdx".to_string())
        .with_source_path("/proj/content/a.mdx");
    let jsx = mdx_to_jsx_module_with_pipeline(
        "<Note>\n\n## [Guide](https://example.org)\n\n</Note>\n",
        opts,
        &mut p,
    )
    .expect("compile must succeed");

    assert!(
        jsx.contains(r#"text: "Guide""#),
        "the headings export must keep the flattened link text: {jsx}"
    );
    assert!(
        jsx.contains(r#"slug: "guide""#),
        "the headings export must keep a non-empty slug derived from that text: {jsx}"
    );
    assert!(
        jsx.contains(r#"id="guide""#),
        "the rendered heading must keep its stamped id (and, by extension, its \
         hash-link anchor, which is derived from the same slug): {jsx}"
    );
    assert!(
        jsx.contains("target=\"_blank\""),
        "the nested link itself must still be treated as external: {jsx}"
    );
}
