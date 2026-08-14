//! Regression tests for GFM autolink literals nested inside a link label
//! (zfb#2388).
//!
//! `markdown-rs`'s autolink-literal pass fires inside an existing link's
//! label, which cmark-gfm's extension explicitly does not do (it never
//! descends into a link node). The result is an `<a>` inside an `<a>` —
//! invalid HTML that fails `html-validate` with
//! `element-permitted-content`. zfb 2.5.0 turned `gfm.autolinkLiteral` on
//! by default (a5be2fca), so the defect started firing on ordinary,
//! previously-valid documents.
//!
//! `crates/zfb-content/src/plugins/nested_link.rs` normalises the mdast
//! right after parse; its own `#[cfg(test)] mod tests` covers the tree
//! surgery at Level 1 against hand-built mdast. This file is the Level-3
//! companion: it pins the rendered **output** of both public entry points
//! that parse markdown, since each has its own `to_mdast` call site and
//! could regress independently.
//!
//! - **HTML path** — `render_html` / `Pipeline::run`.
//! - **MDX/JSX emit path** — `render_mdx_jsx_module`, whose `to_mdast`
//!   call is in `mdx_jsx_emit.rs` and is not shared with `Pipeline::run`.
//!
//! The last test pins the gating contract: with `autolinkLiteral` off the
//! normalisation must not run, because `markdown-rs` produces no nested
//! links at all in that configuration.

use zfb_content::pipeline::{Pipeline, ResolvedGfmConstructs};
use zfb_content::{render_html, render_mdx_jsx_module};

/// ONLY `autolink_literal` on. Deliberately built from `ALL_OFF` rather
/// than `CONSERVATIVE` — `CONSERVATIVE` already carries `autolink_literal:
/// true` alongside strikethrough and table, so deriving from it would make
/// "only the construct under test is on" false, and would silently turn the
/// gating test at the bottom of this file into a no-op.
const AUTOLINK_ONLY: ResolvedGfmConstructs = ResolvedGfmConstructs {
    autolink_literal: true,
    ..ResolvedGfmConstructs::ALL_OFF
};

/// `AUTOLINK_ONLY` plus the table construct, for the one test that needs a
/// table to exist before it can put a link in a cell.
const AUTOLINK_AND_TABLE: ResolvedGfmConstructs = ResolvedGfmConstructs {
    table: true,
    ..AUTOLINK_ONLY
};

/// True when one anchor opens before the previous one closed.
///
/// Normalises the JSX emitter's `<_components.a …>` spelling to the HTML
/// one first, so a single helper serves both entry points. Scans for the
/// exact tag boundaries (`<a ` / `<a>` / `</a>`) so an unrelated element
/// like `<abbr>` cannot be miscounted as an anchor.
fn has_nested_anchor(markup: &str) -> bool {
    let normalized = markup
        .replace("<_components.a", "<a")
        .replace("</_components.a>", "</a>");
    let mut depth = 0usize;
    let mut i = 0usize;
    while i < normalized.len() {
        let rest = &normalized[i..];
        if rest.starts_with("</a>") {
            depth = depth.saturating_sub(1);
            i += 4;
            continue;
        }
        if let Some(after) = rest.strip_prefix("<a") {
            if after.starts_with(' ') || after.starts_with('>') {
                depth += 1;
                if depth > 1 {
                    return true;
                }
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    false
}

fn render(src: &str) -> String {
    let mut pipeline = Pipeline::with_defaults_and_theme_and_gfm(None, AUTOLINK_ONLY);
    render_html(&mut pipeline, src).expect("render must succeed")
}

/// The detector is the shared discriminator for almost every test below, so
/// it is pinned against the issue's own verbatim before/after strings (HTML
/// and JSX). Without this, a detector that silently stopped matching would
/// turn the whole file green while the bug was live — which is exactly what
/// happened to an earlier revision of this helper on the JSX spelling.
#[test]
fn nested_anchor_detector_is_not_vacuous() {
    assert!(has_nested_anchor(
        "<p><a href=\"http://localhost:4321\"><a href=\"http://localhost:4321\">http://localhost:4321</a></a></p>"
    ));
    assert!(has_nested_anchor(
        "<a href=\"https://example.com\">see <a href=\"https://example.com\">https://example.com</a> now</a>"
    ));
    assert!(has_nested_anchor(
        "<_components.a href=\"http://x\"><_components.a href=\"http://x\">x</_components.a></_components.a>"
    ));

    assert!(!has_nested_anchor(
        "<p><a href=\"http://localhost:4321\">http://localhost:4321</a></p>"
    ));
    assert!(!has_nested_anchor(
        "<a href=\"https://a.com\">a</a> and <a href=\"https://b.com\">b</a>"
    ));
    assert!(!has_nested_anchor(
        "<_components.a href=\"http://x\">x</_components.a>"
    ));
    assert!(
        !has_nested_anchor("<a href=\"https://a.com\"><abbr title=\"x\">a</abbr></a>"),
        "an `<abbr>` inside an anchor must not be counted as a second anchor"
    );
}

// ───────────────────────────────────────────────────────────────────
// HTML path — `render_html` / `Pipeline::run`.
// ───────────────────────────────────────────────────────────────────

/// The issue's headline repro: a link whose label is itself a bare URL.
/// The expected output is spelled out in full because the issue pins it.
#[test]
fn html_label_that_is_a_bare_url_emits_exactly_one_anchor() {
    let html = render("[http://localhost:4321](http://localhost:4321)");
    assert!(
        html.contains("<p><a href=\"http://localhost:4321\">http://localhost:4321</a></p>"),
        "a URL-shaped label must render exactly one anchor; got:\n{html}"
    );
    assert!(!has_nested_anchor(&html), "nested anchor emitted:\n{html}");
}

/// The issue's second repro: a bare URL merely embedded in the label. The
/// surrounding label text must survive the unwrap.
#[test]
fn html_bare_url_inside_a_label_keeps_surrounding_text() {
    let html = render("[see https://example.com now](https://example.com)");
    assert!(
        html.contains("<a href=\"https://example.com\">see https://example.com now</a>"),
        "label text around the URL must be preserved; got:\n{html}"
    );
    assert!(!has_nested_anchor(&html), "nested anchor emitted:\n{html}");
}

/// The `www.` autolink form nests too — it is a separate branch of the
/// tokenizer from the `scheme://` form, and the issue did not list it.
#[test]
fn html_www_form_label_emits_exactly_one_anchor() {
    let html = render("[www.example.com](https://example.com)");
    assert!(
        html.contains("<a href=\"https://example.com\">www.example.com</a>"),
        "the `www.` autolink form must not nest; got:\n{html}"
    );
    assert!(!has_nested_anchor(&html), "nested anchor emitted:\n{html}");
}

/// The nesting is not always a direct child: the autolink fires through
/// intervening inline markup, so the fix has to keep walking while inside
/// a link rather than only inspecting the label's immediate children.
#[test]
fn html_autolink_nested_under_inline_markup_is_unwrapped() {
    let html = render("[**bold https://example.com**](https://x.com)");
    assert!(
        html.contains("<a href=\"https://x.com\"><strong>bold https://example.com</strong></a>"),
        "an autolink under `strong` inside a label must be unwrapped; got:\n{html}"
    );
    assert!(!has_nested_anchor(&html), "nested anchor emitted:\n{html}");
}

/// CommonMark degrades the OUTER link of `[a [b](c) d](e)` to literal text,
/// but the surviving inner link is still a link — and the autolink fires
/// inside *its* label. The fix must reach that one too.
#[test]
fn html_autolink_inside_a_degraded_outer_link_is_unwrapped() {
    let html = render("[a [b https://e.com c](d) e](f)");
    assert!(
        html.contains("<a href=\"d\">b https://e.com c</a>"),
        "the surviving inner link must not carry a nested autolink; got:\n{html}"
    );
    assert!(!has_nested_anchor(&html), "nested anchor emitted:\n{html}");
}

/// Block context must not matter — table cells and list items reach the
/// label through different parents than a paragraph does.
#[test]
fn html_nesting_is_unwrapped_in_table_cells_and_list_items() {
    let mut pipeline = Pipeline::with_defaults_and_theme_and_gfm(None, AUTOLINK_AND_TABLE);
    let table = render_html(
        &mut pipeline,
        "| [https://a.com](https://a.com) |\n| --- |\n| x |",
    )
    .expect("render must succeed");
    assert!(
        table.contains("<th><a href=\"https://a.com\">https://a.com</a></th>"),
        "table cell must render one anchor; got:\n{table}"
    );
    assert!(
        !has_nested_anchor(&table),
        "nested anchor emitted:\n{table}"
    );

    let list = render("- [https://a.com](https://a.com)");
    assert!(
        list.contains("<a href=\"https://a.com\">https://a.com</a>"),
        "list item must render one anchor; got:\n{list}"
    );
    assert!(!has_nested_anchor(&list), "nested anchor emitted:\n{list}");
}

/// The fix must not disturb the feature it is fixing: a bare URL in
/// ordinary prose still becomes an autolink.
#[test]
fn html_top_level_autolink_still_renders() {
    let html = render("bare https://example.com here");
    assert!(
        html.contains("<p>bare <a href=\"https://example.com\">https://example.com</a> here</p>"),
        "a bare URL outside any link must still autolink; got:\n{html}"
    );
}

/// An `Image` inside a link is legal HTML and must survive — the unwrap
/// targets links, not every child of a link.
#[test]
fn html_image_inside_a_link_is_preserved() {
    let html = render("[![alt](img.png)](https://example.com)");
    assert!(
        html.contains("<a href=\"https://example.com\"><img src=\"img.png\" alt=\"alt\"/></a>"),
        "a linked image must be preserved verbatim; got:\n{html}"
    );
}

/// A plain label with no URL in it is untouched.
#[test]
fn html_ordinary_link_is_untouched() {
    let html = render("[label](https://example.com)");
    assert!(
        html.contains("<p><a href=\"https://example.com\">label</a></p>"),
        "an ordinary link must render unchanged; got:\n{html}"
    );
}

// ───────────────────────────────────────────────────────────────────
// MDX / JSX emit path — `render_mdx_jsx_module`, whose `to_mdast` call
// site is separate from `Pipeline::run`'s.
// ───────────────────────────────────────────────────────────────────

/// The JSX emitter parses through its own `to_mdast` call, so it needs its
/// own coverage: a fix applied only to `Pipeline::run` would leave every
/// MDX-compiled page emitting nested anchors.
#[test]
fn jsx_emit_label_that_is_a_bare_url_emits_exactly_one_anchor() {
    let mut pipeline = Pipeline::with_defaults_and_theme_and_gfm(None, AUTOLINK_ONLY);
    let module = render_mdx_jsx_module(
        &mut pipeline,
        "[http://localhost:4321](http://localhost:4321)\n",
        "test.mdx",
    )
    .expect("jsx emit must succeed");

    assert!(
        !has_nested_anchor(&module),
        "MDX/JSX emit must not produce a nested anchor; got:\n{module}"
    );
    assert!(
        module.contains("http://localhost:4321"),
        "the link must still be present in the emitted module; got:\n{module}"
    );
}

fn emit_jsx(src: &str) -> String {
    let mut pipeline = Pipeline::with_defaults_and_theme_and_gfm(None, AUTOLINK_ONLY);
    render_mdx_jsx_module(&mut pipeline, src, "test.mdx").expect("jsx emit must succeed")
}

/// A markdown link inside an author-written JSX element nests too: a JSX
/// element's mdast children are ordinary markdown-parsed nodes, so the
/// autolink pass fires in them. This is the case the CJK passes'
/// `is_no_recurse` set would have hidden had it been inherited wholesale —
/// link validation hit the identical JSX blind spot in zfb#2184 / zfb#2223.
#[test]
fn jsx_emit_link_inside_a_jsx_element_is_unwrapped() {
    let inline = emit_jsx("<Note>[see https://example.com now](https://example.com)</Note>\n");
    assert!(
        !has_nested_anchor(&inline),
        "a link inside an inline JSX element must not nest; got:\n{inline}"
    );

    let block = emit_jsx("<Note>\n\n[http://localhost:4321](http://localhost:4321)\n\n</Note>\n");
    assert!(
        !has_nested_anchor(&block),
        "a link inside a block JSX element must not nest; got:\n{block}"
    );
}

/// An MDX element literally named `a` IS an anchor, so a bare URL
/// autolinked inside it produces a nested anchor and must be unwrapped.
#[test]
fn jsx_emit_bare_url_inside_an_mdx_anchor_element_is_unwrapped() {
    let module = emit_jsx("<a href=\"/x\">bare https://example.com</a>\n");
    assert!(
        !has_nested_anchor(&module),
        "an MDX `<a>` wrapping a bare URL must not nest; got:\n{module}"
    );
    assert!(
        module.contains("https://example.com"),
        "the URL text must survive the unwrap; got:\n{module}"
    );
}

/// The `<a>` handling keys on the lowercase intrinsic name only — a
/// capitalised component's rendered output is unknowable, so a link
/// directly inside one must be left alone.
#[test]
fn jsx_emit_link_inside_a_component_still_renders_its_anchor() {
    let module = emit_jsx("<Anchor>[label](https://example.com)</Anchor>\n");
    assert!(
        module.contains("href=\"https://example.com\""),
        "a link inside a component must still render as an anchor; got:\n{module}"
    );
    assert!(
        !has_nested_anchor(&module),
        "nested anchor emitted:\n{module}"
    );
}

// ───────────────────────────────────────────────────────────────────
// Gating contract.
// ───────────────────────────────────────────────────────────────────

/// With `autolinkLiteral` off, `markdown-rs` never creates the inner link,
/// so the normalisation is gated off and the label renders as plain text.
/// This pins that the fix does not reach into a configuration it has no
/// business changing.
#[test]
fn html_autolink_off_leaves_the_label_as_plain_text() {
    let mut pipeline =
        Pipeline::with_defaults_and_theme_and_gfm(None, ResolvedGfmConstructs::ALL_OFF);
    let html = render_html(
        &mut pipeline,
        "[see https://example.com now](https://example.com)",
    )
    .expect("render must succeed");
    assert!(
        html.contains("<a href=\"https://example.com\">see https://example.com now</a>"),
        "with autolink off the label is plain text inside one anchor; got:\n{html}"
    );
    assert!(!has_nested_anchor(&html), "nested anchor emitted:\n{html}");
}
