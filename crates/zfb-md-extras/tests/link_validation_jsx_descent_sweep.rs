//! Wave 4 confirm-pass sweep (zfb#2184; Link Validation Descent epic #2222,
//! sub #2226) — the "build-level sweep" `link_validation_jsx_descent_counts.rs`'s
//! module doc anticipated ahead of this wave.
//!
//! Companion files each prove one angle: `link_validation_jsx_directive_descent.rs`
//! (Wave 1/3) proves each of the six contexts warns exactly once IN ISOLATION
//! (one context, one broken link, one fixture per test); `link_validation_jsx_descent_counts.rs`
//! (Wave 3) proves the partition invariant at the unit level (top-level +
//! nested = 2, JSX-in-JSX = 1, valid-nested = 0, …). Neither combines all six
//! contexts into ONE document. That is this file's job: assert exact counts
//! over a SINGLE fixture carrying every context at once, so a dedup bug in
//! the new JSX-nested collector path cannot hide behind the four
//! already-working structured-walk contexts' correctness (the literal ask of
//! #2226's acceptance criteria — "a dedup bug in the new path cannot hide
//! behind the old path's correctness").
//!
//! ## Red-first (recorded, not re-derivable from this file)
//!
//! Both counting assertions below were proven falsifiable before being
//! trusted as permanent regression guards (per #2226's "a confirm test that
//! cannot fail is not a confirm test"):
//!
//! - Broken sweep: temporarily pointing the `<Note>` context's link at the
//!   valid target instead of its broken anchor dropped the observed count
//!   from 6 to 5 and removed `#missing-jsxnote-en` from the href set — the
//!   test failed on both the `assert_eq!(6, …)` and the href-set assertion,
//!   for the expected reason. Reverted immediately after observing the
//!   failure.
//! - Dedup guard: temporarily duplicating `LinkValidationPlugin`'s
//!   `for c in &pending { validate_link(...) }` loop body (simulating a
//!   double-drain regression) turned the broken sweep's count from 6 to 8
//!   and made `#missing-jsxnote-en` / `#missing-directive-en` each appear
//!   twice in the href multiset — caught by the per-href exactly-once
//!   assertion, not just the total. Reverted immediately after observing
//!   the failure. See the #2226 completion report for the exact recorded
//!   output of both runs.
//!
//! ## What is NOT covered here
//!
//! JSX-in-JSX nesting, inline `MdxJsxTextElement`, image candidates, and the
//! pipeline-reuse leak guard are `link_validation_jsx_descent_counts.rs`'s
//! job, not repeated here. This file only sweeps the six-context matrix.

use std::collections::HashMap;
use std::path::PathBuf;

use zfb_content::pipeline::{BuildContext, Pipeline};
use zfb_md_ast::{
    diagnostics::{CollectingSink, MarkdownDiagnostic},
    heading_registry::HeadingRegistry,
    DirectiveSpec, LinkValidationConfig, MarkdownFeaturesConfig,
};

/// Same single configuration every companion descent-test file uses: a
/// registered `"note"` container directive (`:::note` -> `<Note>`) alongside
/// `linkValidation`, so `<Note>` (hand-authored JSX) and `:::note` (directive
/// expansion) are both live in the SAME document.
fn make_pipeline_with_note_directive() -> Pipeline {
    let mut directives = HashMap::new();
    directives.insert("note".to_string(), DirectiveSpec::Short("Note".to_string()));
    let features = MarkdownFeaturesConfig {
        link_validation: Some(LinkValidationConfig::default()),
        directives: Some(directives),
        ..Default::default()
    };
    Pipeline::with_defaults_and_features(&features)
}

fn run_once(md: &str) -> Vec<MarkdownDiagnostic> {
    let mut registry = HeadingRegistry::new();
    let mut pipeline = make_pipeline_with_note_directive();
    let mut sink = CollectingSink::new();
    let mut ctx = BuildContext {
        source_path: Some(PathBuf::from("/project/docs/page.mdx")),
        project_root: PathBuf::from("/project"),
        public_dir: PathBuf::from("/project/public"),
        heading_registry: Some(&mut registry),
        diagnostics: Some(&mut sink),
        cross_file_links: None,
    };
    pipeline
        .run_with_context(md, &mut ctx)
        .expect("pipeline must not fail");
    sink.take()
}

/// The raw href of every `BrokenLink` diagnostic, in emission order. Panics
/// on any other diagnostic variant — nothing in this fixture should ever
/// produce a `Generic` diagnostic, so a stray one is a test bug, not
/// something to filter past silently.
fn broken_hrefs(diags: &[MarkdownDiagnostic]) -> Vec<&str> {
    diags
        .iter()
        .map(|d| match d {
            MarkdownDiagnostic::BrokenLink { url, .. } => url.as_str(),
            other => panic!("expected only BrokenLink diagnostics, got {other:?}"),
        })
        .collect()
}

/// Assert `actual` contains exactly the hrefs in `expected`, each exactly
/// once — order-independent (diagnostic emission order is structured-walk
/// first, then nested candidates in document order; this sweep does not
/// pin that order, only the multiset).
fn assert_exact_href_set(actual: &[&str], expected: &[&str], diags: &[MarkdownDiagnostic]) {
    let mut actual_sorted = actual.to_vec();
    actual_sorted.sort_unstable();
    let mut expected_sorted = expected.to_vec();
    expected_sorted.sort_unstable();
    assert_eq!(
        actual_sorted, expected_sorted,
        "expected exactly one diagnostic per href in {expected:?}, no duplicates, \
         no drops: {diags:#?}"
    );
}

// ── Broken sweep: all six contexts, one distinct broken anchor each ────────

/// EN: plain flow, table cell, blockquote, list item (the four contexts
/// that already worked before #2184), plus `<Note>` JSX and `:::note`
/// directive (the two Wave 3 fixed) — ONE document, six distinct broken
/// bare-fragment anchors. Must report EXACTLY six diagnostics, each href
/// exactly once: never fewer (a regression reintroducing the #2184 blind
/// spot), never more (a dedup regression double-reporting the new path).
#[test]
fn all_six_contexts_distinct_broken_anchors_report_exactly_six_en() {
    let md = "[link](#missing-flow-en)\n\
\n\
| Col |\n\
| --- |\n\
| [link](#missing-table-en) |\n\
\n\
> [link](#missing-blockquote-en)\n\
\n\
- [link](#missing-list-en)\n\
\n\
<Note>\n\
\n\
[link](#missing-jsxnote-en)\n\
\n\
</Note>\n\
\n\
:::note\n\
\n\
[link](#missing-directive-en)\n\
\n\
:::\n";
    let diags = run_once(md);
    assert_eq!(
        diags.len(),
        6,
        "six contexts, six distinct broken anchors = exactly six diagnostics, \
         never more (dedup) or fewer (blind spot): {diags:#?}"
    );
    let hrefs = broken_hrefs(&diags);
    assert_exact_href_set(
        &hrefs,
        &[
            "#missing-flow-en",
            "#missing-table-en",
            "#missing-blockquote-en",
            "#missing-list-en",
            "#missing-jsxnote-en",
            "#missing-directive-en",
        ],
        &diags,
    );
}

/// CJK counterpart of the EN sweep above, reusing the exact per-context
/// fragment spellings `link_validation_jsx_directive_descent.rs` already
/// established (continuity across companion files) — proves CJK fragment
/// handling holds at full-sweep scope, not just isolated per context.
#[test]
fn all_six_contexts_distinct_broken_anchors_report_exactly_six_ja() {
    let md = "[リンク](#存在しない見出し-流れ)\n\
\n\
| 列 |\n\
| --- |\n\
| [リンク](#存在しない見出し-表) |\n\
\n\
> [リンク](#存在しない見出し-引用)\n\
\n\
- [リンク](#存在しない見出し-リスト)\n\
\n\
<Note>\n\
\n\
[リンク](#存在しない見出し-ノート)\n\
\n\
</Note>\n\
\n\
:::note\n\
\n\
[リンク](#存在しない見出し-ディレクティブ)\n\
\n\
:::\n";
    let diags = run_once(md);
    assert_eq!(
        diags.len(),
        6,
        "six contexts (JA), six distinct broken anchors = exactly six \
         diagnostics: {diags:#?}"
    );
    let hrefs = broken_hrefs(&diags);
    assert_exact_href_set(
        &hrefs,
        &[
            "#存在しない見出し-流れ",
            "#存在しない見出し-表",
            "#存在しない見出し-引用",
            "#存在しない見出し-リスト",
            "#存在しない見出し-ノート",
            "#存在しない見出し-ディレクティブ",
        ],
        &diags,
    );
}

// ── Valid sweep: all six contexts, one shared VALID anchor ─────────────────

/// EN: the same six contexts, but every link points at a real top-level
/// heading. Must report EXACTLY zero diagnostics — closes the specific gap
/// Wave 3's own valid-link test left open: that test covered `<Note>` JSX
/// only, never the `:::note` container-directive form the acceptance
/// criteria names explicitly ("valid links inside `<Note>` / `:::note`").
#[test]
fn all_six_contexts_valid_link_to_shared_heading_reports_zero_en() {
    let md = "## Real Target\n\
\n\
[link](#real-target)\n\
\n\
| Col |\n\
| --- |\n\
| [link](#real-target) |\n\
\n\
> [link](#real-target)\n\
\n\
- [link](#real-target)\n\
\n\
<Note>\n\
\n\
[link](#real-target)\n\
\n\
</Note>\n\
\n\
:::note\n\
\n\
[link](#real-target)\n\
\n\
:::\n";
    let diags = run_once(md);
    assert!(
        diags.is_empty(),
        "six contexts, all pointing at a real top-level heading, must stay \
         silent: {diags:#?}"
    );
}

/// CJK counterpart: a real top-level heading with CJK text, linked from all
/// six contexts.
#[test]
fn all_six_contexts_valid_link_to_shared_heading_reports_zero_ja() {
    let md = "## 実在する見出し\n\
\n\
[リンク](#実在する見出し)\n\
\n\
| 列 |\n\
| --- |\n\
| [リンク](#実在する見出し) |\n\
\n\
> [リンク](#実在する見出し)\n\
\n\
- [リンク](#実在する見出し)\n\
\n\
<Note>\n\
\n\
[リンク](#実在する見出し)\n\
\n\
</Note>\n\
\n\
:::note\n\
\n\
[リンク](#実在する見出し)\n\
\n\
:::\n";
    let diags = run_once(md);
    assert!(
        diags.is_empty(),
        "six contexts (JA), all pointing at a real top-level heading, must \
         stay silent: {diags:#?}"
    );
}
