//! Characterization tests for the link-validation MDX-JSX / container-directive
//! blind spot (zfb#2184; Link Validation Descent epic #2222, Wave 1 sub #2223).
//!
//! # What this file proves
//!
//! `collect_diagnostics` (`zfb-md-extras/src/link_validation.rs`) walks the
//! **hast** tree and recurses only through `HastNode::Element` and
//! `HastNode::Root`; every other variant (`Text | Raw | JsxRaw | Comment`) is
//! a leaf. This file pins, per #2184's evidence table, exactly which
//! `HastNode` variant each syntactic context produces at the point
//! `collect_diagnostics` runs, and proves the resulting warn/silent split:
//!
//! | context | today | `HastNode` variant produced |
//! |---|---|---|
//! | plain flow text | warns | `Element` (`<p>`) |
//! | table cell | warns | `Element` (`<table>`/…/`<td>`) |
//! | blockquote | warns | `Element` (`<blockquote>`) |
//! | list item | warns | `Element` (`<li>`) |
//! | `<Note>…</Note>` (MDX JSX) | **silent** | `JsxRaw(String)` |
//! | `:::note … :::` (container directive) | **silent** | `JsxRaw(String)` |
//!
//! # Verdict: ONE mechanism, not two
//!
//! `DirectiveRegistry` (`zfb-content/src/plugins/directives.rs`) is an
//! **mdast**-phase visitor: `transform_block_container` / `build_flow_jsx`
//! rewrite a recognised `:::name … :::` run into an ordinary
//! `MdastNode::MdxJsxFlowElement` — the exact same mdast node kind produced
//! for a hand-authored `<Note>…</Note>`. This happens BEFORE `mdast_to_hast`
//! ever runs (`register_features_config_derived`'s ordering contract: the
//! directives step is an mdast visitor, always applied ahead of the
//! mdast→hast conversion). `mdast_to_hast_inner`
//! (`zfb-content/src/pipeline.rs`) then has exactly ONE match arm for JSX
//! nodes:
//!
//! ```rust,ignore
//! MdastNode::MdxJsxFlowElement(_)
//! | MdastNode::MdxJsxTextElement(_)
//! | MdastNode::MdxFlowExpression(_)
//! | MdastNode::MdxTextExpression(_) => HastNode::JsxRaw(emit_jsx_raw(node, strategy, fc)),
//! ```
//!
//! unconditional on `JsxEmitStrategy` (`HtmlPath` vs `JsxPath` only changes
//! what string `emit_jsx_raw` produces, never the outer `JsxRaw` wrapping).
//! So a container directive and a hand-authored MDX JSX element reach
//! `collect_diagnostics` as the identical `HastNode` shape — **one root
//! cause, not two** — confirmed directly by
//! `context_container_directive_note_produces_jsx_raw_node` and
//! `context_mdx_jsx_note_produces_jsx_raw_node` below, which assert the
//! SAME `JsxRaw(_)` variant for both.
//!
//! # RED tests (flip protocol, `crates/CLAUDE.md`)
//!
//! The four `#[ignore = "pending-feature: …/2184"]` tests below assert the
//! DESIRED post-fix behaviour (a broken link inside `<Note>`/`:::note` must
//! warn) — written once, never to be rewritten to suit whatever Wave 3
//! implements. They are proven RED (not merely assumed) — see the
//! per-test doc comment and the #2223 issue comment for the recorded
//! pre-fix `cargo test -- --ignored --exact` output.
//!
//! The three `_produces_..._node` tests are NOT tagged `pending-feature` —
//! they pin today's MECHANISM (which `HastNode` variant is produced), a fact
//! about the current implementation, not a behavioural contract Wave 3 is
//! obligated to preserve. Wave 3 may fix the blind spot without ever
//! changing which `HastNode` variant a JSX element produces (e.g. approach
//! (d) in epic #2222 — collect link candidates from mdast before the hast
//! detour, validate them late) — so these three stay green regardless of
//! which of the epic's four candidate approaches Wave 3 picks.

use std::collections::HashMap;
use std::path::PathBuf;

use zfb_content::pipeline::{BuildContext, HastNode, Pipeline};
use zfb_md_ast::{
    diagnostics::{CollectingSink, MarkdownDiagnostic},
    heading_registry::HeadingRegistry,
    DirectiveSpec, LinkValidationConfig, MarkdownFeaturesConfig,
};

// ── Shared fixtures (reused between the `_should_warn_*` and the
//    `_produces_..._node` mechanism pins, so both angles agree on
//    byte-identical input) ─────────────────────────────────────────────────

/// `<Note>…</Note>` (MDX JSX) wrapping one broken bare-fragment link.
const MDX_JSX_NOTE_BROKEN_EN: &str = "<Note>\n\n[link](#missing-note-en)\n\n</Note>\n";

/// `:::note … :::` (container directive, registered as `Note`) wrapping the
/// same shape of broken bare-fragment link.
const CONTAINER_DIRECTIVE_NOTE_BROKEN_EN: &str =
    ":::note\n\n[link](#missing-directive-en)\n\n:::\n";

/// Blockquote wrapping a broken bare-fragment link — the "warns today"
/// contrast case for the mechanism pins below.
const BLOCKQUOTE_BROKEN_EN: &str = "> [link](#missing-blockquote-en)\n";

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build a pipeline with `linkValidation` enabled AND a registered `"note"`
/// container directive (`:::note` → `<Note>`) — the single configuration
/// this whole file uses so every context in #2184's evidence table is
/// exercised under ONE consistent pipeline (proving, among other things,
/// that registering the directives feature does not itself perturb the
/// already-working contexts).
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

/// Run `md` through [`make_pipeline_with_note_directive`], returning both
/// the collected diagnostics AND the final hast tree — the latter lets
/// callers pin the exact `HastNode` variant a context produced at the point
/// `collect_diagnostics` walked it (`LinkValidationPlugin` never mutates the
/// tree, so the returned shape is exactly what it saw).
fn run_with_note_directive(
    md: &str,
    source_path: PathBuf,
    project_root: PathBuf,
    registry: &mut HeadingRegistry,
    cfg: LinkValidationConfig,
) -> (Vec<MarkdownDiagnostic>, HastNode) {
    let mut sink = CollectingSink::new();
    let mut ctx = BuildContext {
        source_path: Some(source_path),
        project_root,
        public_dir: PathBuf::from("/project/public"),
        heading_registry: Some(registry),
        diagnostics: Some(&mut sink),
        cross_file_links: None,
    };
    let mut pipeline = make_pipeline_with_note_directive(cfg);
    let hast = pipeline
        .run_with_context(md, &mut ctx)
        .expect("pipeline must not fail");
    (sink.take(), hast)
}

// ── Control group: contexts that warn TODAY and must keep warning ───────────
//
// None of these are `#[ignore]`d — they run on every T1 gate, both now and
// after Wave 3 lands, so a regression in the "already worked" contexts would
// be caught immediately. Each covers one context in EN and its CJK
// counterpart, per #2184's confirmed EN/JA symmetry.

/// Plain flow text — the baseline context, run under the SAME
/// directives-enabled pipeline as the silent contexts below (not just the
/// bare default pipeline `link_validation.rs` already covers elsewhere).
#[test]
fn context_plain_flow_text_link_warns_en() {
    let mut registry = HeadingRegistry::new();
    let md = "[link](#missing-flow-en)\n";
    let (diags, _hast) = run_with_note_directive(
        md,
        PathBuf::from("/project/docs/page.mdx"),
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert_eq!(
        diags.len(),
        1,
        "plain flow text: expected exactly one BrokenLink: {diags:?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "#missing-flow-en"),
        "diagnostic url must be the raw href: {diags:?}"
    );
}

/// CJK counterpart of `context_plain_flow_text_link_warns_en`.
#[test]
fn context_plain_flow_text_link_warns_ja() {
    let mut registry = HeadingRegistry::new();
    let md = "[リンク](#存在しない見出し-流れ)\n";
    let (diags, _hast) = run_with_note_directive(
        md,
        PathBuf::from("/project/docs/page.mdx"),
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert_eq!(
        diags.len(),
        1,
        "plain flow text (JA): expected exactly one BrokenLink: {diags:?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "#存在しない見出し-流れ"),
        "diagnostic url must be the raw href: {diags:?}"
    );
}

/// A broken link inside a GFM table cell.
#[test]
fn context_table_cell_link_warns_en() {
    let mut registry = HeadingRegistry::new();
    let md = "| Col |\n| --- |\n| [link](#missing-table-en) |\n";
    let (diags, _hast) = run_with_note_directive(
        md,
        PathBuf::from("/project/docs/page.mdx"),
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert_eq!(
        diags.len(),
        1,
        "table cell: expected exactly one BrokenLink: {diags:?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "#missing-table-en"),
        "diagnostic url must be the raw href: {diags:?}"
    );
}

/// CJK counterpart of `context_table_cell_link_warns_en`.
#[test]
fn context_table_cell_link_warns_ja() {
    let mut registry = HeadingRegistry::new();
    let md = "| 列 |\n| --- |\n| [リンク](#存在しない見出し-表) |\n";
    let (diags, _hast) = run_with_note_directive(
        md,
        PathBuf::from("/project/docs/page.mdx"),
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert_eq!(
        diags.len(),
        1,
        "table cell (JA): expected exactly one BrokenLink: {diags:?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "#存在しない見出し-表"),
        "diagnostic url must be the raw href: {diags:?}"
    );
}

/// A broken link inside a blockquote.
#[test]
fn context_blockquote_link_warns_en() {
    let mut registry = HeadingRegistry::new();
    let (diags, _hast) = run_with_note_directive(
        BLOCKQUOTE_BROKEN_EN,
        PathBuf::from("/project/docs/page.mdx"),
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert_eq!(
        diags.len(),
        1,
        "blockquote: expected exactly one BrokenLink: {diags:?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "#missing-blockquote-en"),
        "diagnostic url must be the raw href: {diags:?}"
    );
}

/// CJK counterpart of `context_blockquote_link_warns_en`.
#[test]
fn context_blockquote_link_warns_ja() {
    let mut registry = HeadingRegistry::new();
    let md = "> [リンク](#存在しない見出し-引用)\n";
    let (diags, _hast) = run_with_note_directive(
        md,
        PathBuf::from("/project/docs/page.mdx"),
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert_eq!(
        diags.len(),
        1,
        "blockquote (JA): expected exactly one BrokenLink: {diags:?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "#存在しない見出し-引用"),
        "diagnostic url must be the raw href: {diags:?}"
    );
}

/// A broken link inside a list item.
#[test]
fn context_list_item_link_warns_en() {
    let mut registry = HeadingRegistry::new();
    let md = "- [link](#missing-list-en)\n";
    let (diags, _hast) = run_with_note_directive(
        md,
        PathBuf::from("/project/docs/page.mdx"),
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert_eq!(
        diags.len(),
        1,
        "list item: expected exactly one BrokenLink: {diags:?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "#missing-list-en"),
        "diagnostic url must be the raw href: {diags:?}"
    );
}

/// CJK counterpart of `context_list_item_link_warns_en`.
#[test]
fn context_list_item_link_warns_ja() {
    let mut registry = HeadingRegistry::new();
    let md = "- [リンク](#存在しない見出し-リスト)\n";
    let (diags, _hast) = run_with_note_directive(
        md,
        PathBuf::from("/project/docs/page.mdx"),
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert_eq!(
        diags.len(),
        1,
        "list item (JA): expected exactly one BrokenLink: {diags:?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "#存在しない見出し-リスト"),
        "diagnostic url must be the raw href: {diags:?}"
    );
}

// ── RED group: contexts that are SILENT today (zfb#2184) ────────────────────
//
// Written in desired POST-FIX form per the flip protocol (`crates/CLAUDE.md`):
// each asserts exactly one BrokenLink is emitted, matching the control group's
// shape above. Today every one of these four fails with `diags.len() == 0`
// (silently swallowed) — see the #2223 issue comment for the recorded
// `cargo test -p zfb-md-extras -- --ignored --exact <name>` output proving
// this RED, not merely assuming it. Wave 3 flips these green; the assertions
// themselves must never be edited to fit whatever Wave 3 implements.

/// A broken link nested inside `<Note>…</Note>` (MDX JSX) must warn exactly
/// like every other context. TODAY it is silently swallowed: the MDX JSX
/// element reaches `collect_diagnostics` as a single opaque
/// `HastNode::JsxRaw(String)` (see `context_mdx_jsx_note_produces_jsx_raw_node`
/// for the direct node-shape proof), which `collect_diagnostics` treats as a
/// leaf (`link_validation.rs`'s `// Leaf nodes.` match arm) — the nested
/// `<a href>` is never visited.
#[test]
fn context_mdx_jsx_note_children_link_should_warn_en() {
    let mut registry = HeadingRegistry::new();
    let (diags, _hast) = run_with_note_directive(
        MDX_JSX_NOTE_BROKEN_EN,
        PathBuf::from("/project/docs/page.mdx"),
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert_eq!(
        diags.len(),
        1,
        "broken link inside <Note>…</Note> must warn: {diags:?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "#missing-note-en"),
        "diagnostic url must be the raw href: {diags:?}"
    );
}

/// CJK counterpart of `context_mdx_jsx_note_children_link_should_warn_en`.
#[test]
fn context_mdx_jsx_note_children_link_should_warn_ja() {
    let mut registry = HeadingRegistry::new();
    let md = "<Note>\n\n[リンク](#存在しない見出し-ノート)\n\n</Note>\n";
    let (diags, _hast) = run_with_note_directive(
        md,
        PathBuf::from("/project/docs/page.mdx"),
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert_eq!(
        diags.len(),
        1,
        "broken link inside <Note>…</Note> (JA) must warn: {diags:?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "#存在しない見出し-ノート"),
        "diagnostic url must be the raw href: {diags:?}"
    );
}

/// A broken link nested inside `:::note … :::` (a registered container
/// directive) must warn exactly like every other context. TODAY it is
/// silently swallowed by the SAME mechanism as the MDX JSX case above —
/// `DirectiveRegistry` expands `:::note … :::` into an
/// `MdxJsxFlowElement` at the mdast phase (indistinguishable, from
/// `mdast_to_hast_inner`'s point of view, from a hand-authored `<Note>`),
/// which then flattens to `HastNode::JsxRaw(String)` — see
/// `context_container_directive_note_produces_jsx_raw_node` for the direct
/// proof that this is the IDENTICAL node shape as the MDX JSX case, not a
/// second, independent bug.
#[test]
fn context_container_directive_note_children_link_should_warn_en() {
    let mut registry = HeadingRegistry::new();
    let (diags, _hast) = run_with_note_directive(
        CONTAINER_DIRECTIVE_NOTE_BROKEN_EN,
        PathBuf::from("/project/docs/page.mdx"),
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert_eq!(
        diags.len(),
        1,
        "broken link inside :::note … ::: must warn: {diags:?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "#missing-directive-en"),
        "diagnostic url must be the raw href: {diags:?}"
    );
}

/// CJK counterpart of `context_container_directive_note_children_link_should_warn_en`.
#[test]
fn context_container_directive_note_children_link_should_warn_ja() {
    let mut registry = HeadingRegistry::new();
    let md = ":::note\n\n[リンク](#存在しない見出し-ディレクティブ)\n\n:::\n";
    let (diags, _hast) = run_with_note_directive(
        md,
        PathBuf::from("/project/docs/page.mdx"),
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert_eq!(
        diags.len(),
        1,
        "broken link inside :::note … ::: (JA) must warn: {diags:?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "#存在しない見出し-ディレクティブ"),
        "diagnostic url must be the raw href: {diags:?}"
    );
}

// ── Mechanism pins: which `HastNode` variant does each context produce? ────
//
// Direct, non-diagnostic-based evidence for the findings note on #2223.
// These are NOT `pending-feature`-tagged: they characterize TODAY's
// mechanism, which Wave 3 is not obligated to change (see the module doc).

/// A hand-authored MDX JSX element's content reaches `collect_diagnostics`
/// as a SINGLE opaque `HastNode::JsxRaw(String)` — not a structured
/// `Element` with a recursible `children` list. This is *why* the broken
/// link inside it is invisible to `collect_diagnostics`'s `Element | Root`
/// recursion, independent of any diagnostic assertion.
#[test]
fn context_mdx_jsx_note_produces_jsx_raw_node() {
    let mut registry = HeadingRegistry::new();
    let (_diags, hast) = run_with_note_directive(
        MDX_JSX_NOTE_BROKEN_EN,
        PathBuf::from("/project/docs/page.mdx"),
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    let HastNode::Root { children } = &hast else {
        panic!("expected Root, got {hast:?}");
    };
    assert_eq!(
        children.len(),
        1,
        "expected exactly one top-level node: {children:?}"
    );
    assert!(
        matches!(&children[0], HastNode::JsxRaw(_)),
        "MDX JSX element content must flatten to a single opaque JsxRaw node: {children:?}"
    );
}

/// The container-directive counterpart of
/// `context_mdx_jsx_note_produces_jsx_raw_node`: `:::note … :::` ALSO
/// flattens to `HastNode::JsxRaw(_)` — the SAME variant, proving the two
/// silent contexts in #2184's evidence table share ONE mechanism (the
/// directive is expanded to an `MdxJsxFlowElement` in the mdast phase,
/// before `mdast_to_hast` ever sees it — see the module doc), not two
/// independent bugs.
#[test]
fn context_container_directive_note_produces_jsx_raw_node() {
    let mut registry = HeadingRegistry::new();
    let (_diags, hast) = run_with_note_directive(
        CONTAINER_DIRECTIVE_NOTE_BROKEN_EN,
        PathBuf::from("/project/docs/page.mdx"),
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    let HastNode::Root { children } = &hast else {
        panic!("expected Root, got {hast:?}");
    };
    assert_eq!(
        children.len(),
        1,
        "expected exactly one top-level node: {children:?}"
    );
    assert!(
        matches!(&children[0], HastNode::JsxRaw(_)),
        "container directive content must ALSO flatten to JsxRaw — the same \
         mechanism as hand-authored MDX JSX, not a second bug: {children:?}"
    );
}

/// Contrast case: a blockquote (a context that warns today) produces a
/// structured `Element` node — `collect_diagnostics` recurses through it and
/// finds the nested `<a href>` normally. This is the shape the two `JsxRaw`
/// pins above are contrasted against.
#[test]
fn context_blockquote_produces_structured_element_node() {
    let mut registry = HeadingRegistry::new();
    let (_diags, hast) = run_with_note_directive(
        BLOCKQUOTE_BROKEN_EN,
        PathBuf::from("/project/docs/page.mdx"),
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    let HastNode::Root { children } = &hast else {
        panic!("expected Root, got {hast:?}");
    };
    assert_eq!(
        children.len(),
        1,
        "expected exactly one top-level node: {children:?}"
    );
    assert!(
        matches!(&children[0], HastNode::Element { tag, .. } if tag == "blockquote"),
        "blockquote must produce a structured, recursible Element node: {children:?}"
    );
}
