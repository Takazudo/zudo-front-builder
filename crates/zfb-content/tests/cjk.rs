//! End-to-end integration tests for [`CjkFriendlyPlugin`].
//!
//! Each case runs through the full default MDX pipeline
//! ([`Pipeline::with_defaults`]) and serialises to HTML, mirroring the
//! 11-cjk.mdx fixture from zudo-doc's `packages/md-plugins`. We assert
//! on substring contents rather than exact-string equality because the
//! default pipeline also applies heading anchors, syntect, etc., and
//! those side-effects are not what this sub-issue is about.
//!
//! See `crates/zfb-content/src/plugins/cjk_friendly.rs` for the
//! per-rule unit tests; this file covers the integration surface.

use zfb_content::pipeline::Pipeline;
use zfb_content::serializer::serialize;

fn render(input: &str) -> String {
    let mut p = Pipeline::with_defaults();
    let hast = p.run(input).expect("pipeline runs");
    serialize(&hast)
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

// --- Acceptance criteria from issue #109. ---

#[test]
fn japanese_strong_around_kanji() {
    let input = "これは**重要**な機能です\n";
    let html = render(input);
    assert_contains(&html, "<p>これは<strong>重要</strong>な機能です</p>", input);
}

#[test]
fn chinese_emphasis_around_kanji() {
    let input = "中文 *斜体* 中文\n";
    let html = render(input);
    assert_contains(&html, "<p>中文 <em>斜体</em> 中文</p>", input);
}

#[test]
fn korean_strong_around_hangul() {
    let input = "한국어**강조**한국어\n";
    let html = render(input);
    assert_contains(&html, "<p>한국어<strong>강조</strong>한국어</p>", input);
}

#[test]
fn triple_marker_around_cjk_nests_strong_and_em() {
    let input = "これは***重要***な機能です\n";
    let html = render(input);
    // markdown-rs emits Em(Strong(...)); both nesting orders are
    // semantically equivalent — assert presence of both tags.
    assert_contains(&html, "<strong>", input);
    assert_contains(&html, "</strong>", input);
    assert_contains(&html, "<em>", input);
    assert_contains(&html, "</em>", input);
    assert_contains(&html, "重要", input);
}

#[test]
fn fullwidth_brackets_flank_strong() {
    let input = "「**重要**」\n";
    let html = render(input);
    assert_contains(&html, "「<strong>重要</strong>」", input);

    let input = "（**強調**）\n";
    let html = render(input);
    assert_contains(&html, "（<strong>強調</strong>）", input);
}

#[test]
fn nested_emphasis_inside_strong_with_cjk() {
    let input = "**外側 *内側* 外側**\n";
    let html = render(input);
    assert_contains(&html, "<strong>", input);
    assert_contains(&html, "<em>内側</em>", input);
}

#[test]
fn mixed_script_keeps_both_flanks() {
    let input = "日本語 mixed with English **bold** text 日本語\n";
    let html = render(input);
    assert_contains(
        &html,
        "<p>日本語 mixed with English <strong>bold</strong> text 日本語</p>",
        input,
    );
}

#[test]
fn ascii_only_emphasis_unaffected() {
    let input = "This is **bold** text\n";
    let html = render(input);
    assert_contains(&html, "<p>This is <strong>bold</strong> text</p>", input);
}

#[test]
fn escape_protected_markers_unaffected() {
    let input = "\\**not bold\\**\n";
    let html = render(input);
    // Must NOT have <strong>not bold</strong>.
    assert_lacks(&html, "<strong>not bold", input);
    assert_contains(&html, "*", input);
}

// --- The CJK-punctuation-flanking cases that base markdown-rs misses
//     (the actual purpose of the visitor). ---

#[test]
fn cjk_punctuation_inside_strong_close() {
    let input = "**テスト。**テスト\n";
    let html = render(input);
    assert_contains(&html, "<strong>テスト。</strong>テスト", input);
}

#[test]
fn cjk_punctuation_inside_emphasis_close() {
    let input = "*テスト。*テスト\n";
    let html = render(input);
    assert_contains(&html, "<em>テスト。</em>テスト", input);
}

#[test]
fn cjk_punctuation_inside_strong_close_with_kanji_lead() {
    let input = "これは**重要。**テスト\n";
    let html = render(input);
    assert_contains(&html, "これは<strong>重要。</strong>テスト", input);
}

// --- No-rewrite zone coverage. ---

#[test]
fn inline_code_not_rewritten() {
    let input = "`これは**重要。**テスト`\n";
    let html = render(input);
    // Inside <code>, the markers stay literal.
    assert_contains(&html, "<code>これは**重要。**テスト</code>", input);
    assert_lacks(&html, "<strong>重要", input);
}

#[test]
fn fenced_code_not_rewritten() {
    let input = "```\nこれは**重要。**テスト\n```\n";
    let html = render(input);
    // Syntect or pre/code wrap: assert literal `**` survives somewhere
    // in the code-block region by checking the raw markers haven't
    // been turned into <strong>.
    assert_contains(&html, "**重要。**", input);
    assert_lacks(&html, "<strong>重要", input);
}

#[test]
fn mdx_jsx_body_not_rewritten() {
    // JSX bodies are passed through verbatim by the serializer; the
    // CJK visitor must not re-tokenise their inner text either.
    let input = "<MyNote>これは**重要。**テスト</MyNote>\n";
    let html = render(input);
    assert_contains(&html, "**重要。**", input);
}

// --- Heading slug Unicode preservation (issue #219). ---

#[test]
fn cjk_heading_gets_unicode_slug() {
    let input = "## コンポーネント構文\n";
    let html = render(input);
    // The heading id and anchor href must both carry the full Japanese text.
    assert_contains(&html, "id=\"コンポーネント構文\"", input);
    assert_contains(&html, "href=\"#コンポーネント構文\"", input);
    // A hash-link anchor must be present.
    assert_contains(&html, "class=\"hash-link\"", input);
}

// --- cjkFriendly config toggle (issue #258). ---

fn render_no_cjk(input: &str) -> String {
    use zfb_content::pipeline::ResolvedGfmConstructs;
    let mut p = Pipeline::with_defaults_and_theme_and_gfm_and_cjk(
        None,
        ResolvedGfmConstructs::CONSERVATIVE,
        false,
    );
    let hast = p.run(input).expect("pipeline runs");
    serialize(&hast)
}

/// Default (no cjkFriendly field) — CJK-friendly is on. The CJK-punct-
/// inside-close case that base CommonMark misses must still be fixed.
#[test]
fn cjk_friendly_default_true_fixes_cjk_punct_close() {
    let input = "**テスト。**テスト\n";
    let html = render(input);
    assert_contains(&html, "<strong>テスト。</strong>テスト", input);
}

/// `cjkFriendly: true` — same output as absent.
#[test]
fn cjk_friendly_explicit_true_same_as_default() {
    use zfb_content::pipeline::ResolvedGfmConstructs;
    let input = "**テスト。**テスト\n";
    let mut p = Pipeline::with_defaults_and_theme_and_gfm_and_cjk(
        None,
        ResolvedGfmConstructs::CONSERVATIVE,
        true,
    );
    let hast = p.run(input).expect("pipeline runs");
    let html = serialize(&hast);
    let html_default = render(input);
    assert_eq!(
        html, html_default,
        "cjkFriendly:true must be byte-identical to absent"
    );
}

/// `cjkFriendly: false` — plugin omitted; CJK-punct-inside-close NOT
/// repaired. The marker stays as literal `**` inside `<p>`.
#[test]
fn cjk_friendly_false_omits_plugin() {
    let input = "**テスト。**テスト\n";
    let html = render_no_cjk(input);
    // Under CommonMark-strict the closing `**` before `テ` is not
    // right-flanking — the run stays literal; no <strong> tag.
    assert_lacks(&html, "<strong>", input);
    assert_contains(&html, "**テスト。**テスト", input);
}

/// `cjkFriendly: false` — ordinary ASCII emphasis must still work.
#[test]
fn cjk_friendly_false_keeps_ascii_emphasis() {
    let input = "This is **bold** text\n";
    let html = render_no_cjk(input);
    assert_contains(&html, "<strong>bold</strong>", input);
}

/// GFM strikethrough at a CJK boundary works under the default pipeline.
/// `remark-cjk-friendly-gfm-strikethrough` covers this in the JS world;
/// in our Rust port it works because markdown-rs's GFM tokeniser handles
/// `~~` delimiter runs independently of the CommonMark emphasis flanking
/// rules. No extension to `CjkFriendlyPlugin` is needed.
#[test]
fn gfm_strikethrough_cjk_boundary_default() {
    let input = "~~取り消し線~~の続き\n";
    let html = render(input);
    assert_contains(&html, "<del>取り消し線</del>の続き", input);
}

/// GFM strikethrough at a CJK boundary also works when `cjkFriendly:
/// false` because strikethrough is handled by the GFM tokeniser, not
/// by `CjkFriendlyPlugin`.
#[test]
fn gfm_strikethrough_cjk_boundary_cjk_friendly_false() {
    let input = "~~取り消し線~~の続き\n";
    let html = render_no_cjk(input);
    assert_contains(&html, "<del>取り消し線</del>の続き", input);
}

/// Bold/italic at CJK boundary — default pipeline.
#[test]
fn bold_at_cjk_boundary_default() {
    let input = "**強調**するテキスト\n";
    let html = render(input);
    assert_contains(&html, "<strong>強調</strong>するテキスト", input);
}

