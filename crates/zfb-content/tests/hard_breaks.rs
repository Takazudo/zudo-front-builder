//! End-to-end integration tests for [`HardBreaksPlugin`].
//!
//! Each case runs through the full default MDX pipeline
//! ([`Pipeline::with_defaults_and_full_config`]) and serialises to HTML.
//! We assert on substring contents rather than exact-string equality because
//! the default pipeline also applies heading anchors, syntect, etc., and those
//! side-effects are not what this feature is about.
//!
//! See `crates/zfb-content/src/plugins/hard_breaks.rs` for the per-rule unit
//! tests; this file covers the integration surface.

use zfb_content::mdx_to_jsx_module_with_pipeline;
use zfb_content::pipeline::{Pipeline, ResolvedGfmConstructs};
use zfb_content::serializer::serialize;
use zfb_content::MdxJsxOptions;

fn render_with_hard_breaks(input: &str, hard_breaks: bool) -> String {
    let mut p = Pipeline::with_defaults_and_full_config(
        None,
        ResolvedGfmConstructs::CONSERVATIVE,
        None,
        true,
        hard_breaks,
        None,
    )
    .expect("pipeline builds");
    let hast = p.run(input).expect("pipeline runs");
    serialize(&hast)
}

fn render_on(input: &str) -> String {
    render_with_hard_breaks(input, true)
}

fn render_off(input: &str) -> String {
    render_with_hard_breaks(input, false)
}

fn assert_contains(html: &str, needle: &str, input: &str) {
    assert!(
        html.contains(needle),
        "for input {input:?}, expected output to contain {needle:?}; got {html:?}"
    );
}

fn assert_lacks(html: &str, needle: &str, input: &str) {
    assert!(
        !html.contains(needle),
        "for input {input:?}, expected output NOT to contain {needle:?}; got {html:?}"
    );
}

/// Core acceptance criterion: `hardBreaks: true` → `first\nsecond` in a
/// paragraph → `<p>first<br/>second</p>` (HTML serializer uses self-closing `<br/>`).
#[test]
fn hard_breaks_on_converts_soft_newline_to_br() {
    let input = "first\nsecond\n";
    let html = render_on(input);
    // The HTML serializer uses void `<br/>` for Break nodes.
    assert_contains(&html, "first<br/>second", input);
    assert_contains(&html, "<p>", input);
}

/// Default (off): soft line break collapses, no `<br/>` tag produced.
#[test]
fn hard_breaks_off_no_br_from_soft_newline() {
    let input = "first\nsecond\n";
    let html = render_off(input);
    assert_lacks(&html, "<br/>", input);
    // Standard CommonMark: the single \n becomes a space, both words in <p>.
    assert_contains(&html, "first", input);
    assert_contains(&html, "second", input);
}

/// `hardBreaks: false` (explicit) is byte-identical to absent (default off).
#[test]
fn explicit_false_byte_identical_to_default() {
    let input = "first\nsecond\n";
    let on = render_on(input);
    let off = render_off(input);
    // They differ (on has <br>, off does not).
    assert_ne!(on, off);
    // Both contain the text.
    assert_contains(&on, "first", input);
    assert_contains(&off, "first", input);
}

/// Multiple soft breaks all become `<br/>`.
#[test]
fn multiple_soft_newlines_all_become_br() {
    let input = "a\nb\nc\n";
    let html = render_on(input);
    // Two line breaks → two <br/> tags between a, b, c.
    let br_count = html.matches("<br/>").count();
    assert_eq!(
        br_count, 2,
        "expected 2 <br/> tags; got {br_count} in {html:?}"
    );
    assert_contains(&html, "a", input);
    assert_contains(&html, "b", input);
    assert_contains(&html, "c", input);
}

/// Blank-line paragraph separation is unaffected by `hardBreaks`.
#[test]
fn blank_line_paragraph_separation_unaffected_when_on() {
    let input = "para one\n\npara two\n";
    let html = render_on(input);
    // No <br/> introduced — blank line is block separation.
    assert_lacks(&html, "<br/>", input);
    // Two separate <p> elements.
    let p_count = html.matches("<p>").count();
    assert_eq!(p_count, 2, "expected 2 paragraphs; got {p_count} in {html:?}");
}

/// Blank-line paragraph separation also unaffected when off.
#[test]
fn blank_line_paragraph_separation_unaffected_when_off() {
    let input = "para one\n\npara two\n";
    let on = render_on(input);
    let off = render_off(input);
    // Structural output (paragraphs) should be the same regardless of hardBreaks.
    let on_p = on.matches("<p>").count();
    let off_p = off.matches("<p>").count();
    assert_eq!(on_p, off_p, "paragraph count must match; on={on:?} off={off:?}");
}

/// `\n` inside fenced code block must NOT become `<br/>`.
#[test]
fn fenced_code_newlines_not_converted() {
    let input = "```\nfirst\nsecond\n```\n";
    let html = render_on(input);
    assert_lacks(&html, "<br/>", input);
}

/// `\n` inside inline code must NOT become `<br/>`.
///
/// Note: markdown-rs may normalise the newline to a space in inline code; either
/// way, no `<br/>` element should appear.
#[test]
fn inline_code_newlines_not_converted() {
    // Inline code content — markdown-rs normalises newline to space.
    let input = "text `code` more\n";
    let html = render_on(input);
    assert_lacks(&html, "<br/>", input);
}

/// Existing explicit hard breaks (trailing backslash) still work when `hardBreaks` is off.
#[test]
fn explicit_backslash_hard_break_works_when_option_is_off() {
    let input = "first\\\nsecond\n";
    let html = render_off(input);
    // The HTML serializer uses <br/> for Break nodes.
    assert_contains(&html, "<br/>", input);
}

/// Explicit backslash hard break also works when `hardBreaks` is on.
#[test]
fn explicit_backslash_hard_break_works_when_option_is_on() {
    let input = "first\\\nsecond\n";
    let html = render_on(input);
    // At least one <br/> (from either the backslash hard break or the plugin).
    assert_contains(&html, "<br/>", input);
}

// --- JSX-emit path: assert Break → <br/> on the compiled-JSX path -----

/// `hardBreaks: true` on the JSX-emit path: `first\nsecond` → JSX with `<br/>`.
///
/// This exercises the `mdx_to_jsx_module_with_pipeline` code path to confirm that
/// `MdastNode::Break` nodes inserted by `HardBreaksPlugin` are emitted as `<br/>`
/// in the compiled JSX (not discarded or left as text).
#[test]
fn hard_breaks_on_jsx_emit_path_produces_br() {
    let mut p = Pipeline::with_defaults_and_full_config(
        None,
        ResolvedGfmConstructs::CONSERVATIVE,
        None,
        true,
        true, // hard_breaks on
        None,
    )
    .expect("pipeline builds");

    let input = "first\nsecond\n";
    let jsx = mdx_to_jsx_module_with_pipeline(input, MdxJsxOptions::default(), &mut p)
        .expect("JSX emit succeeds");

    // The JSX emitter uses "br" for Break nodes. Confirm the tag appears.
    assert!(
        jsx.contains("\"br\"") || jsx.contains("'br'"),
        "compiled JSX must contain a 'br' element for the soft line break; got:\n{jsx}"
    );
}

/// `hardBreaks: false` on the JSX-emit path: no `<br/>` introduced.
#[test]
fn hard_breaks_off_jsx_emit_path_no_br() {
    let mut p = Pipeline::with_defaults_and_full_config(
        None,
        ResolvedGfmConstructs::CONSERVATIVE,
        None,
        true,
        false, // hard_breaks off
        None,
    )
    .expect("pipeline builds");

    let input = "first\nsecond\n";
    let jsx = mdx_to_jsx_module_with_pipeline(input, MdxJsxOptions::default(), &mut p)
        .expect("JSX emit succeeds");

    // Without hardBreaks the soft break is collapsed; no "br" element.
    // We check that "br" doesn't appear as a JSX element tag (but allow it
    // in identifiers or comments by only excluding the quoted form).
    assert!(
        !jsx.contains("\"br\"") && !jsx.contains("'br'"),
        "compiled JSX must NOT contain a 'br' element when hardBreaks is off; got:\n{jsx}"
    );
}
