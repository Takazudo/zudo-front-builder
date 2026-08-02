//! Live reproduction of the four JSX-nested anchor classes (issue #2244,
//! epic #2243) on the REAL MDX compile path — `mdx_to_jsx_module_with_pipeline`
//! with armed build-context roots, the `link_validation_jsx_descent_mdx_path.rs`
//! idiom. Each class is exercised nested inside BOTH a literal MDX JSX
//! element (`<Note>…</Note>`) and a container directive (`:::note … :::`,
//! which `DirectiveRegistry` expands into the same `MdxJsxFlowElement`
//! shape in the mdast phase).
//!
//! Executed verdicts (2026-08-02, this file's first commit):
//!
//! 1. same-file link → JSX-nested heading: **bridged** (no false positive).
//!    `collect_headings` (`mdx_jsx_emit.rs`) pre-seeds the per-compile
//!    `HeadingRegistry` with every heading, including JSX-nested ones.
//! 2. cross-file link → JSX-nested heading: **bridged**. In-compile
//!    validation degrades to existence-only and records a
//!    `CrossFileLinkCandidate`; the per-file `FileHeadings` side channel
//!    (seeded from the same `collect_headings` walk) carries the nested
//!    heading id, so the post-compile check (`zfb-build` bundler, #980)
//!    settles the candidate as valid.
//! 3. link → JSX-nested explicit anchor (`<div id>` / `<a name>`):
//!    **broken** — false `BrokenLink`. The whole JSX subtree collapses to
//!    one opaque `HastNode::JsxRaw`, so `HeadingLinksPlugin::visit_node`'s
//!    `insert_anchor_id` arm (hast-phase, `Root|Element`-only recursion)
//!    never sees the anchor element.
//! 4. footnote back-reference whose reference occurrence sits inside JSX:
//!    **broken** — false `BrokenLink` on the structured footnote section's
//!    backref `<a href="#user-content-fnref-…">`. The occurrence marker
//!    (carrying the `id`) is rendered inside the JsxRaw string by
//!    `jsx_footnote_reference_marker`, invisible to the hast walk.
//!
//! FLIP CONTRACT (#2246, Wave 2): the tests below whose names end in
//! `_broken_today` assert the CURRENT broken behavior (exactly one false
//! `BrokenLink`). The #2246 implementation registers the missing anchor
//! ids and must FLIP those assertions to "no diagnostics" (delete the
//! `_broken_today` suffix and the false-positive assertion; keep the
//! fixture). They are deliberately committed green-asserting-broken
//! rather than `#[ignore = "pending-feature: …"]`-red so the branch's
//! `cargo test -p zfb-content` stays green throughout the epic. The
//! bridged-class tests (classes 1 and 2) are permanent regression tests
//! and must never be weakened.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use zfb_content::mdx_jsx_emit::{mdx_to_jsx_module_with_pipeline, MdxJsxOptions};
use zfb_content::pipeline::{Pipeline, ResolvedGfmConstructs};
use zfb_md_ast::diagnostics::MarkdownDiagnostic;
use zfb_md_ast::{DirectiveSpec, LinkValidationConfig, MarkdownFeaturesConfig};

/// Full-config pipeline: linkValidation + the `note` → `Note` directive
/// mapping + GFM ALL_ON (footnotes are OFF in the CONSERVATIVE default,
/// and class 4 needs `footnote_definition`). Build-context roots armed —
/// real builds always arm them (zfb#944); that is what routes visitor
/// diagnostics into the pipeline's counters.
fn pipeline(project_root: &Path) -> Pipeline {
    let mut directives = HashMap::new();
    directives.insert("note".to_string(), DirectiveSpec::Short("Note".to_string()));
    let features = MarkdownFeaturesConfig {
        link_validation: Some(LinkValidationConfig::default()),
        directives: Some(directives),
        ..Default::default()
    };
    let mut p = Pipeline::with_defaults_and_full_config(
        None,
        ResolvedGfmConstructs::ALL_ON,
        None,
        false,
        false,
        Some(&features),
    )
    .expect("pipeline construction must succeed");
    p.set_build_context_roots(project_root.to_path_buf(), project_root.join("public"));
    p
}

/// Compile one in-memory document rooted at `/proj` (no on-disk files —
/// fine for every class except the cross-file one, which needs real
/// files for the existence probe) and return the drained diagnostics.
fn compile_and_take_diags(source: &str) -> Vec<MarkdownDiagnostic> {
    let mut p = pipeline(Path::new("/proj"));
    let opts = MdxJsxOptions::default()
        .with_filename("content/page.mdx".to_string())
        .with_source_path("/proj/content/page.mdx");
    let _jsx = mdx_to_jsx_module_with_pipeline(source, opts, &mut p).expect("compile must succeed");
    p.take_markdown_diagnostics()
}

fn assert_no_diags(source: &str) {
    let diags = compile_and_take_diags(source);
    assert!(
        diags.is_empty(),
        "expected no diagnostics for:\n{source}\ngot: {diags:#?}"
    );
}

/// Assert the compile emits exactly one false `BrokenLink` for `url` —
/// the CURRENT (broken) behavior. #2246 flips call sites of this helper
/// to `assert_no_diags`.
fn assert_single_false_broken_link_today(source: &str, url: &str) {
    let diags = compile_and_take_diags(source);
    assert_eq!(
        diags.len(),
        1,
        "expected exactly the one false BrokenLink for:\n{source}\ngot: {diags:#?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url: u, .. } if u == url),
        "diagnostic url must be {url:?}: {diags:?}"
    );
}

// ── Class 1: same-file link → JSX-nested heading — BRIDGED ───────────────────

/// Both the heading and the link sit inside `<Note>`: the nested link is
/// collected by `JsxNestedLinkCollector` and validated against the
/// per-compile registry, which `collect_headings` seeded with the nested
/// heading's slug. Permanent regression test.
#[test]
fn same_file_link_to_jsx_nested_heading_passes() {
    assert_no_diags("<Note>\n\n## Nested Target\n\n[jump](#nested-target)\n\n</Note>\n");
}

/// Same class through the container-directive spelling.
#[test]
fn same_file_link_to_directive_nested_heading_passes() {
    assert_no_diags(":::note\n\n## Nested Target\n\n[jump](#nested-target)\n\n:::\n");
}

/// Top-level link → JSX-nested heading (the two-sided variant): the
/// structured hast walk validates the top-level `<a>`; the target slug
/// still comes from the `collect_headings` seed. Permanent regression test.
#[test]
fn top_level_link_to_jsx_nested_heading_passes() {
    assert_no_diags("<Note>\n\n## Nested Target\n\n</Note>\n\n[jump](#nested-target)\n");
}

// ── Class 2: cross-file link → JSX-nested heading — BRIDGED ──────────────────

/// In-compile validation of `./b.mdx#nested-target` must NOT emit a
/// diagnostic (existence-only degrade) and must record a
/// `CrossFileLinkCandidate`; compiling the target must surface the
/// JSX-nested heading in the `FileHeadings` side channel under the SAME
/// normalized path key. Those two channels are exactly the inputs the
/// post-compile check joins on (`zfb-build` bundler step 2c-anchor,
/// #980 — covered by `bundler_anchor_check.rs`), so together they prove
/// the cross-file class is bridged end to end. Permanent regression test.
#[test]
fn cross_file_link_to_jsx_nested_heading_is_settled_by_the_channels() {
    cross_file_case(
        "<Note>\n\n## Nested Target\n\n</Note>\n",
        "<Note>\n\n[jump](./b.mdx#nested-target)\n\n</Note>\n",
    );
}

/// Same class through the container-directive spelling on both sides.
#[test]
fn cross_file_link_to_directive_nested_heading_is_settled_by_the_channels() {
    cross_file_case(
        ":::note\n\n## Nested Target\n\n:::\n",
        ":::note\n\n[jump](./b.mdx#nested-target)\n\n:::\n",
    );
}

fn cross_file_case(target_source: &str, linking_source: &str) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let content = root.join("content");
    fs::create_dir_all(&content).expect("mkdir content");
    let a_path: PathBuf = content.join("a.mdx");
    let b_path: PathBuf = content.join("b.mdx");
    fs::write(&a_path, linking_source).expect("write a.mdx");
    fs::write(&b_path, target_source).expect("write b.mdx");

    let mut p = pipeline(&root);

    // Compile the TARGET file: its FileHeadings record must carry the
    // JSX-nested heading id.
    let opts_b = MdxJsxOptions::default()
        .with_filename("content/b.mdx".to_string())
        .with_source_path(&b_path);
    let _jsx = mdx_to_jsx_module_with_pipeline(target_source, opts_b, &mut p)
        .expect("target compile must succeed");
    let diags_b = p.take_markdown_diagnostics();
    assert!(
        diags_b.is_empty(),
        "target file must compile clean: {diags_b:#?}"
    );
    let headings = p.take_file_headings();
    assert_eq!(headings.len(), 1, "one FileHeadings record: {headings:#?}");
    assert!(
        headings[0].headings.iter().any(|h| h.id == "nested-target"),
        "the JSX-nested heading must reach the FileHeadings side channel: {headings:#?}"
    );

    // Compile the LINKING file: no in-compile diagnostic; one candidate
    // whose target key equals the FileHeadings record's key.
    let opts_a = MdxJsxOptions::default()
        .with_filename("content/a.mdx".to_string())
        .with_source_path(&a_path);
    let _jsx = mdx_to_jsx_module_with_pipeline(linking_source, opts_a, &mut p)
        .expect("linking compile must succeed");
    let diags_a = p.take_markdown_diagnostics();
    assert!(
        diags_a.is_empty(),
        "cross-file fragment must degrade to existence-only in-compile, \
         not emit a BrokenLink: {diags_a:#?}"
    );
    let candidates = p.take_cross_file_link_candidates();
    assert_eq!(candidates.len(), 1, "one candidate: {candidates:#?}");
    assert_eq!(candidates[0].fragment, "nested-target");
    assert_eq!(
        candidates[0].target_path, headings[0].source_path,
        "the candidate's target key and the FileHeadings key must join \
         (the post-compile check's lookup contract, #980)"
    );
}

// ── Class 3: link → JSX-nested explicit anchor — BROKEN today ────────────────
//
// #2246 flips each `_broken_today` test to `assert_no_diags` once the
// JSX-authored literal `id` / `<a name>` anchors are registered into
// `HeadingRegistry.anchor_ids` before validation.

/// `<div id>` anchor and the link both nested inside `<Note>`.
#[test]
fn link_to_jsx_nested_div_id_anchor_broken_today() {
    assert_single_false_broken_link_today(
        "<Note>\n\n<div id=\"nested-div-anchor\"></div>\n\n[jump](#nested-div-anchor)\n\n</Note>\n",
        "#nested-div-anchor",
    );
}

/// `<a name>` legacy named anchor nested inside `<Note>`.
#[test]
fn link_to_jsx_nested_a_name_anchor_broken_today() {
    assert_single_false_broken_link_today(
        "<Note>\n\n<a name=\"nested-name-anchor\"></a>\n\n[jump](#nested-name-anchor)\n\n</Note>\n",
        "#nested-name-anchor",
    );
}

/// Same class through the container-directive spelling.
#[test]
fn link_to_directive_nested_div_id_anchor_broken_today() {
    assert_single_false_broken_link_today(
        ":::note\n\n<div id=\"nested-div-anchor\"></div>\n\n[jump](#nested-div-anchor)\n\n:::\n",
        "#nested-div-anchor",
    );
}

/// CHARACTERIZATION (spec input for #2246, not one of the four classes):
/// on the MDX path a TOP-LEVEL `<div id>` is also an `MdxJsxFlowElement`
/// and also collapses to JsxRaw, so it is exactly as invisible to the
/// hast-phase `insert_anchor_id` as the nested one — the registration
/// walk must therefore collect MDX-JSX-authored anchors at EVERY depth,
/// not only under a JSX ancestor. #2246 flips this too.
#[test]
fn link_to_top_level_jsx_div_id_anchor_broken_today() {
    assert_single_false_broken_link_today(
        "<div id=\"top-div-anchor\"></div>\n\n[jump](#top-div-anchor)\n",
        "#top-div-anchor",
    );
}

// ── Class 4: footnote reference occurrence inside JSX — BROKEN today ─────────
//
// The structured footnote section's backreference link points at the
// occurrence marker's id, which was rendered inside the JsxRaw string
// (`jsx_footnote_reference_marker`) and never registered. #2246 flips
// each `_broken_today` test to `assert_no_diags` once the footnote
// occurrence ids (allocated by `FootnoteModel` — the footnotes module's
// own allocator, never re-derived) are registered before validation.

/// Reference occurrence inside `<Note>`, definition at top level.
#[test]
fn footnote_backref_to_jsx_nested_occurrence_broken_today() {
    assert_single_false_broken_link_today(
        "<Note>\n\nRef[^a] end.\n\n</Note>\n\n[^a]: Body.\n",
        "#user-content-fnref-a",
    );
}

/// Same class through the container-directive spelling.
#[test]
fn footnote_backref_to_directive_nested_occurrence_broken_today() {
    assert_single_false_broken_link_today(
        ":::note\n\nRef[^a] end.\n\n:::\n\n[^a]: Body.\n",
        "#user-content-fnref-a",
    );
}

/// CONTROL (permanent): a top-level occurrence renders as a structured
/// `<sup><a id="user-content-fnref-b">` hast element, so the hast-phase
/// `insert_anchor_id` arm records it and the backref validates clean —
/// the partition evidence that only JSX-nested occurrences are missing.
#[test]
fn footnote_backref_to_top_level_occurrence_passes() {
    assert_no_diags("Ref[^b] end.\n\n[^b]: Body.\n");
}
