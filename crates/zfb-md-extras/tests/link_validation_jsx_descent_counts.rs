//! Exact-count, ordering, and reuse guarantees for the JSX-nested link
//! descent (zfb#2184; Link Validation Descent epic #2222, Wave 3 sub
//! #2225) — the unit-level companions to
//! `link_validation_jsx_directive_descent.rs`'s per-context flip set,
//! ahead of Wave 4's build-level sweep.
//!
//! What this file pins, beyond "a nested broken link warns":
//!
//! - **The partition invariant is exact.** A `Link`/`Image` mdast node
//!   either has an MDX-JSX ancestor or it does not; the hast walk
//!   validates exactly the no-ancestor set and the mdast collector
//!   exactly the complement. Wave 4 will assert exact diagnostic counts
//!   over whole builds, so a design that can double-report (or drop the
//!   JSX-in-JSX case) must fail HERE, not there.
//! - **The collector runs after `resolve_links`.** `ResolveLinksPlugin`
//!   descends into JSX children and rewrites nested `Link.url` in place;
//!   collecting pre-rewrite spellings would validate `./page.md` forms
//!   that resolve to (skipped) URL-space hrefs — false positives, the
//!   exact failure mode epic #2222 must not introduce.
//! - **A reused pipeline neither accumulates nor replays candidates.**
//!   Deliberately WITHOUT `reset_per_entry` between documents: the
//!   drain-on-take + clear-then-fill mechanism must hold on its own,
//!   not lean on the reset belt-and-braces layer.

use std::collections::HashMap;
use std::path::PathBuf;

use zfb_content::pipeline::{BuildContext, Pipeline};
use zfb_md_ast::{
    diagnostics::{CollectingSink, MarkdownDiagnostic},
    heading_registry::HeadingRegistry,
    DirectiveSpec, LinkValidationConfig, MarkdownFeaturesConfig,
};

// ── Helpers (mirroring the descent characterization file's harness) ──────────

/// Build a pipeline with `linkValidation` enabled AND a registered
/// `"note"` container directive (`:::note` → `<Note>`) — the same single
/// configuration `link_validation_jsx_directive_descent.rs` uses.
fn make_pipeline_with_note_directive(cfg: LinkValidationConfig) -> Pipeline {
    let mut directives = HashMap::new();
    directives.insert("note".to_string(), DirectiveSpec::Short("Note".to_string()));
    let features = MarkdownFeaturesConfig {
        link_validation: Some(cfg),
        directives: Some(directives),
        ..Default::default()
    };
    Pipeline::with_defaults_and_features(&features)
}

/// Run one document through a GIVEN pipeline (reusable across calls —
/// unlike the characterization file's helper, which builds a fresh
/// pipeline per run) and return the diagnostics it emitted.
fn run_doc(
    pipeline: &mut Pipeline,
    md: &str,
    registry: &mut HeadingRegistry,
) -> Vec<MarkdownDiagnostic> {
    let mut sink = CollectingSink::new();
    let mut ctx = BuildContext {
        source_path: Some(PathBuf::from("/project/docs/page.mdx")),
        project_root: PathBuf::from("/project"),
        public_dir: PathBuf::from("/project/public"),
        heading_registry: Some(registry),
        diagnostics: Some(&mut sink),
        cross_file_links: None,
    };
    pipeline
        .run_with_context(md, &mut ctx)
        .expect("pipeline must not fail");
    sink.take()
}

/// Fresh-pipeline convenience for the single-document tests.
fn run_once(md: &str) -> Vec<MarkdownDiagnostic> {
    let mut registry = HeadingRegistry::new();
    let mut pipeline = make_pipeline_with_note_directive(LinkValidationConfig::default());
    run_doc(&mut pipeline, md, &mut registry)
}

// ── Exact-count dedup (the partition invariant) ──────────────────────────────

/// The SAME broken anchor once in plain flow text AND once inside
/// `<Note>` must report exactly TWO diagnostics — per-occurrence
/// semantics, no URL-keyed dedup, and no double-report of either
/// occurrence (the hast walk sees only the top-level one, the collector
/// only the nested one).
#[test]
fn same_broken_anchor_top_level_and_nested_reports_exactly_two() {
    let md = "[link](#missing-both)\n\n<Note>\n\n[link](#missing-both)\n\n</Note>\n";
    let diags = run_once(md);
    assert_eq!(
        diags.len(),
        2,
        "one top-level + one nested occurrence = exactly two diagnostics: {diags:?}"
    );
    assert!(
        diags.iter().all(
            |d| matches!(d, MarkdownDiagnostic::BrokenLink { url, .. } if url == "#missing-both")
        ),
        "both diagnostics carry the raw href: {diags:?}"
    );
}

/// A broken link inside `<Note><Note>…</Note></Note>` (JSX-in-JSX) must
/// report exactly ONE diagnostic: the collector's walk visits every node
/// exactly once — a second JSX ancestor keeps the `in_jsx` flag `true`
/// but never re-triggers a sub-collection.
#[test]
fn jsx_in_jsx_nested_broken_link_reports_exactly_one() {
    let md = "<Note>\n\n<Note>\n\n[link](#missing-double-nested)\n\n</Note>\n\n</Note>\n";
    let diags = run_once(md);
    assert_eq!(
        diags.len(),
        1,
        "JSX-in-JSX must not double-collect: {diags:?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "#missing-double-nested"),
        "diagnostic url must be the raw href: {diags:?}"
    );
}

/// A VALID nested link targeting an existing top-level heading must
/// report exactly ZERO diagnostics — nested candidates run through the
/// same registry lookup as structured links, and top-level headings are
/// registered by `HeadingLinksPlugin` before the validator runs.
/// (Top-level target deliberately: JSX-nested headings are absent from
/// the registry on this `run_with_context` path — see #2225's per-path
/// registry-fidelity note.)
#[test]
fn valid_nested_link_to_top_level_heading_reports_zero() {
    let md = "## Real Target\n\n<Note>\n\n[link](#real-target)\n\n</Note>\n";
    let diags = run_once(md);
    assert!(
        diags.is_empty(),
        "a valid nested link must stay silent: {diags:?}"
    );
}

// ── Inline JSX context ───────────────────────────────────────────────────────

/// A broken link inside an INLINE MDX JSX element (`MdxJsxTextElement`,
/// mid-paragraph) must report exactly one diagnostic — the collector
/// treats both JSX element kinds as ancestors, not just flow elements.
#[test]
fn inline_jsx_text_element_nested_broken_link_reports_exactly_one() {
    let md = "before <Note>[link](#missing-inline-jsx)</Note> after\n";
    let diags = run_once(md);
    assert_eq!(
        diags.len(),
        1,
        "a link inside an inline JSX element must warn exactly once: {diags:?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "#missing-inline-jsx"),
        "diagnostic url must be the raw href: {diags:?}"
    );
}

// ── Image candidate (`is_img` semantics) ─────────────────────────────────────

/// `![alt](./missing.png)` inside `<Note>` must report exactly one
/// diagnostic through the existence-only `<img src>` rules (`is_img`
/// travels with the candidate).
#[test]
fn nested_image_missing_file_reports_exactly_one() {
    let md = "<Note>\n\n![alt](./missing-descent.png)\n\n</Note>\n";
    let diags = run_once(md);
    assert_eq!(
        diags.len(),
        1,
        "a nested image with a missing target must warn exactly once: {diags:?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "./missing-descent.png"),
        "diagnostic url must be the raw src: {diags:?}"
    );
}

// ── Pipeline-reuse leak guard ────────────────────────────────────────────────

/// One pipeline, two documents, NO `reset_per_entry` in between: doc 1's
/// nested broken link reports exactly one diagnostic, and doc 2 (clean)
/// reports exactly zero — doc 1's candidate must not replay. Guards the
/// drain-on-take + clear-then-fill mechanism itself, independent of the
/// `reset` belt-and-braces layer.
#[test]
fn pipeline_reuse_does_not_replay_candidates_into_next_document() {
    let mut pipeline = make_pipeline_with_note_directive(LinkValidationConfig::default());

    let mut registry1 = HeadingRegistry::new();
    let diags1 = run_doc(
        &mut pipeline,
        "<Note>\n\n[link](#missing-reuse-doc1)\n\n</Note>\n",
        &mut registry1,
    );
    assert_eq!(
        diags1.len(),
        1,
        "doc 1's nested broken link must warn: {diags1:?}"
    );

    let mut registry2 = HeadingRegistry::new();
    let diags2 = run_doc(
        &mut pipeline,
        "plain paragraph, no links at all\n",
        &mut registry2,
    );
    assert!(
        diags2.is_empty(),
        "doc 2 is clean — doc 1's candidate must not replay: {diags2:?}"
    );
}

// ── resolve_links ordering pin ───────────────────────────────────────────────

/// The collector MUST run after the `resolve_links` application:
/// `ResolveLinksPlugin` descends into JSX children and rewrites the
/// nested `[x](./other.md)` to its URL-space route (`/docs/other/`),
/// which `validate_link` then skips — exactly zero diagnostics. This
/// test FAILS (one `./other.md` false positive — the pre-rewrite
/// spelling probed against a nonexistent on-disk path) if the collector
/// is ever moved before the `resolve_links` application, e.g. by
/// registering it as an ordinary mdast visitor.
#[test]
fn resolve_links_rewrite_precedes_nested_collection() {
    let mut pipeline = make_pipeline_with_note_directive(LinkValidationConfig::default());
    let mut source_map = HashMap::new();
    source_map.insert(
        PathBuf::from("/project/docs/other.md"),
        "/docs/other/".to_string(),
    );
    pipeline.add_resolve_links(source_map);
    pipeline.set_resolve_links_source_dir(PathBuf::from("/project/docs"));

    let mut registry = HeadingRegistry::new();
    let diags = run_doc(
        &mut pipeline,
        "<Note>\n\n[link](./other.md)\n\n</Note>\n",
        &mut registry,
    );
    assert!(
        diags.is_empty(),
        "a nested link resolve_links rewrites into URL space must not \
         false-positive — the collector saw a pre-rewrite spelling: {diags:?}"
    );
}
