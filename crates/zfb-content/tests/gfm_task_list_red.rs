//! RED characterization tests for GFM task-list checkbox emission.
//!
//! Sub-issue #2022 (Wave 1 of epic #2021, superseding #1950): the
//! `markdown` crate's GFM task-list tokenizer sets `ListItem.checked`
//! (`Option<bool>`) whenever `taskListItem: true`, but none of zfb's
//! mdast→output converters read it — `grep -rn '\.checked\b'
//! crates/zfb-content/src/ crates/zfb-md-ast/src/ crates/zfb-md-extras/src/`
//! returns zero hits. A checked `- [x]` item renders identically to an
//! unchecked `- [ ]` item, and the `[x]`/`[ ]` marker text itself is
//! stripped by the tokenizer — worse than leaving the flag off, which at
//! least keeps the literal marker text visible.
//!
//! Three independent `ListItem` emit sites exist. This file covers the
//! two reachable through the public `zfb_content` facade:
//!
//! - **Site 1** — `pipeline::mdast_to_hast_inner`'s `ListItem` arm
//!   (`crates/zfb-content/src/pipeline.rs:2516`), exercised here via
//!   [`render_html`] (the `Pipeline::run` HTML-serializer path).
//! - **Site 3** — `mdx_jsx_emit::jsx_render_child`'s `ListItem` arm
//!   (`crates/zfb-content/src/mdx_jsx_emit.rs:1777`), exercised here via
//!   [`render_mdx_jsx_module`] with the task list nested inside an MDX
//!   JSX element body (`<Note>\n\n- [x] …\n\n</Note>`). This is the only
//!   shape that reaches this call site: a *top-level* list in MDX is
//!   rendered through Site 1 first (`mdast_to_hast_inner` builds the hast
//!   tree for the whole document, including top-level lists) and then
//!   bridged to JSX text by `HastJsxBridge` — `jsx_render_child`'s own
//!   `ListItem` arm only fires for markdown recursively rendered *inside*
//!   an MDX JSX element's children (the `JsxEmitStrategy::JsxPath`
//!   closure `jsx_raw_recursive` → `jsx_element_text` → `jsx_render_child`
//!   chain), which requires JSX-element nesting.
//!
//! **Site 2** (`mdx_jsx_emit::JsxEmitter::emit_node`'s `ListItem` arm,
//! `crates/zfb-content/src/mdx_jsx_emit.rs:892`) is NOT covered here — see
//! the `#[cfg(test)] mod tests` block in `mdx_jsx_emit.rs` for a red test
//! and an explanation of why that call site is structurally unreachable
//! through any public entry point today (the no-pipeline path that
//! reaches it always resolves GFM constructs to
//! `ResolvedGfmConstructs::CONSERVATIVE`, which hard-codes
//! `task_list_item: false`).
//!
//! ## Desired post-fix contract (pinned by these tests, for wave 2 / #2024)
//!
//! Mirrors the fix direction #1950 itself suggested: a checked item gets
//! a checkbox marked `checked`; an unchecked item gets one without. Both
//! render as a disabled `<input type="checkbox">` — this is static,
//! server-rendered output with no client-side handler to toggle it.
//! Assertions below deliberately check for the presence/absence of the
//! substrings `type="checkbox"` / `checked` / `disabled` rather than one
//! exact tag string, so the fix has latitude on attribute order and
//! self-closing spelling.

use zfb_content::pipeline::{Pipeline, ResolvedGfmConstructs};
use zfb_content::{render_html, render_mdx_jsx_module};

/// `ResolvedGfmConstructs` with ONLY `task_list_item` flipped on, so a
/// failure here can't be attributed to some other GFM construct
/// (strikethrough/table/autolink/footnote) interacting with the fix.
const TASK_LIST_ON: ResolvedGfmConstructs = ResolvedGfmConstructs {
    task_list_item: true,
    ..ResolvedGfmConstructs::CONSERVATIVE
};

// ───────────────────────────────────────────────────────────────────
// Site 1 — pipeline.rs `mdast_to_hast_inner`'s `ListItem` arm, via the
// HTML serializer path (`render_html` / `Pipeline::run`).
// ───────────────────────────────────────────────────────────────────

#[test]
#[ignore = "pending-feature: https://github.com/Takazudo/zudo-front-builder/issues/2024"]
fn html_checked_task_list_item_emits_checked_checkbox() {
    let mut pipeline = Pipeline::with_defaults_and_theme_and_gfm(None, TASK_LIST_ON);
    let html = render_html(&mut pipeline, "- [x] Buy milk\n").expect("render must succeed");
    assert!(
        html.contains("type=\"checkbox\""),
        "checked task-list item must emit a checkbox input; got:\n{html}"
    );
    assert!(
        html.contains("checked"),
        "checked task-list item's checkbox must carry a checked marker; got:\n{html}"
    );
    assert!(
        html.contains("disabled"),
        "the emitted checkbox is static (server-rendered) and must be disabled; got:\n{html}"
    );
    assert!(
        html.contains("Buy milk"),
        "item text must still be present alongside the checkbox; got:\n{html}"
    );
}

#[test]
#[ignore = "pending-feature: https://github.com/Takazudo/zudo-front-builder/issues/2024"]
fn html_unchecked_task_list_item_emits_unchecked_checkbox() {
    let mut pipeline = Pipeline::with_defaults_and_theme_and_gfm(None, TASK_LIST_ON);
    let html = render_html(&mut pipeline, "- [ ] Buy milk\n").expect("render must succeed");
    assert!(
        html.contains("type=\"checkbox\""),
        "unchecked task-list item must still emit a checkbox input; got:\n{html}"
    );
    assert!(
        !html.contains("checked"),
        "unchecked task-list item's checkbox must NOT carry a checked marker; got:\n{html}"
    );
    assert!(
        html.contains("disabled"),
        "the emitted checkbox is static (server-rendered) and must be disabled; got:\n{html}"
    );
    assert!(
        html.contains("Buy milk"),
        "item text must still be present alongside the checkbox; got:\n{html}"
    );
}

/// Control (NOT ignored — must pass today and after the fix): with
/// `taskListItem` disabled, the literal `- [ ]` marker text stays
/// visible in the rendered HTML — the fallback behavior the epic says
/// must not regress. Pins that the fix is additive (only fires when the
/// construct is actually enabled), not a change to the flag-off path.
#[test]
fn html_control_task_list_flag_off_keeps_literal_marker_text() {
    let mut pipeline =
        Pipeline::with_defaults_and_theme_and_gfm(None, ResolvedGfmConstructs::CONSERVATIVE);
    let html = render_html(&mut pipeline, "- [ ] Buy milk\n").expect("render must succeed");
    assert!(
        html.contains("[ ]"),
        "flag-off control: literal task-list marker text must remain visible \
         when taskListItem is disabled; got:\n{html}"
    );
    assert!(
        !html.contains("type=\"checkbox\""),
        "flag-off control: no checkbox should ever be emitted when taskListItem \
         is disabled; got:\n{html}"
    );
}

// ───────────────────────────────────────────────────────────────────
// Site 3 — mdx_jsx_emit.rs `jsx_render_child`'s `ListItem` arm, reached
// via a task list nested inside an MDX JSX element body.
// ───────────────────────────────────────────────────────────────────

#[test]
#[ignore = "pending-feature: https://github.com/Takazudo/zudo-front-builder/issues/2024"]
fn jsx_nested_checked_task_list_item_emits_checked_checkbox() {
    let mut pipeline = Pipeline::with_defaults_and_theme_and_gfm(None, TASK_LIST_ON);
    let out = render_mdx_jsx_module(
        &mut pipeline,
        "<Note>\n\n- [x] Buy milk\n\n</Note>\n",
        "checked.mdx",
    )
    .expect("emit must succeed");
    assert!(
        out.contains("type=\"checkbox\""),
        "checked task-list item nested in an MDX JSX body must emit a checkbox; got:\n{out}"
    );
    assert!(
        out.contains("checked"),
        "checked task-list item's checkbox must carry a checked marker; got:\n{out}"
    );
    assert!(
        out.contains("disabled"),
        "the emitted checkbox is static (server-rendered) and must be disabled; got:\n{out}"
    );
    assert!(
        out.contains("Buy milk"),
        "item text must still be present alongside the checkbox; got:\n{out}"
    );
}

#[test]
#[ignore = "pending-feature: https://github.com/Takazudo/zudo-front-builder/issues/2024"]
fn jsx_nested_unchecked_task_list_item_emits_unchecked_checkbox() {
    let mut pipeline = Pipeline::with_defaults_and_theme_and_gfm(None, TASK_LIST_ON);
    let out = render_mdx_jsx_module(
        &mut pipeline,
        "<Note>\n\n- [ ] Buy milk\n\n</Note>\n",
        "unchecked.mdx",
    )
    .expect("emit must succeed");
    assert!(
        out.contains("type=\"checkbox\""),
        "unchecked task-list item nested in an MDX JSX body must still emit a checkbox; got:\n{out}"
    );
    assert!(
        !out.contains("checked"),
        "unchecked task-list item's checkbox must NOT carry a checked marker; got:\n{out}"
    );
    assert!(
        out.contains("disabled"),
        "the emitted checkbox is static (server-rendered) and must be disabled; got:\n{out}"
    );
    assert!(
        out.contains("Buy milk"),
        "item text must still be present alongside the checkbox; got:\n{out}"
    );
}

/// Control (NOT ignored — must pass today and after the fix): same as
/// [`html_control_task_list_flag_off_keeps_literal_marker_text`] but for
/// the JSX-nested Site 3 path.
#[test]
fn jsx_nested_control_task_list_flag_off_keeps_literal_marker_text() {
    let mut pipeline =
        Pipeline::with_defaults_and_theme_and_gfm(None, ResolvedGfmConstructs::CONSERVATIVE);
    let out = render_mdx_jsx_module(
        &mut pipeline,
        "<Note>\n\n- [ ] Buy milk\n\n</Note>\n",
        "control.mdx",
    )
    .expect("emit must succeed");
    assert!(
        out.contains("[ ]"),
        "flag-off control: literal task-list marker text must remain visible \
         when taskListItem is disabled; got:\n{out}"
    );
    assert!(
        !out.contains("type=\"checkbox\""),
        "flag-off control: no checkbox should ever be emitted when taskListItem \
         is disabled; got:\n{out}"
    );
}
