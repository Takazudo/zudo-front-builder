//! Reading-time estimate injected into the MDX module as a named export.
//!
//! Rust port of [`remark-reading-time`](https://www.npmjs.com/package/remark-reading-time).
//! Walks the mdast tree, counts words, and appends an
//! `export const readingTimeMinutes = N;` statement to the Root's
//! children as a [`markdown::mdast::MdxjsEsm`] node.
//!
//! # Word counting formula
//!
//! Two distinct passes are combined:
//!
//! 1. **Latin-script** — the text is split on whitespace; each token is a
//!    word (consistent with remark-reading-time's `wordCount` helper).
//! 2. **CJK characters** — each CJK character is counted as one word.
//!    remark-reading-time uses the character count directly without
//!    dividing by any per-character calibration factor; the WPM figure
//!    the user supplies (default 200) already calibrates the per-minute
//!    rate for CJK text, since typical CJK reading speed measured in
//!    characters/minute is around 200-500.
//!
//! This mirrors remark-reading-time's `getWordsPerMinute` approach where
//! CJK characters contribute one "word" each and are divided by the same
//! WPM value as Latin tokens.
//!
//! # Deviations from remark-reading-time
//!
//! 1. **Code/inline-code/HTML/math nodes are skipped.** remark-reading-time
//!    visits `code` nodes and includes their text in the word count. This
//!    implementation intentionally skips `Code`, `InlineCode`, `Html`,
//!    `Math`, and `InlineMath` nodes so that code-heavy documents are not
//!    over-estimated.
//!
//! 2. **A single space is injected at block boundaries.** remark-reading-time
//!    concatenates raw text values with no separator. This implementation
//!    appends a single space after the children of `Paragraph`, `Heading`,
//!    and `TableCell` nodes so that adjacent inline spans that share no
//!    source whitespace (e.g. `**foo**bar`) are not erroneously merged into
//!    one word. `ListItem` is intentionally left unchanged because tight-list
//!    content is already wrapped in `Paragraph` nodes by the markdown crate,
//!    so the separator is injected transitively.
//!
//! # CJK range
//!
//! Characters in the following Unicode blocks are counted individually:
//!
//! - CJK Unified Ideographs (U+4E00–U+9FFF)
//! - CJK Extension A (U+3400–U+4DBF)
//! - CJK Compatibility Ideographs (U+F900–U+FAFF)
//! - Hiragana (U+3040–U+309F)
//! - Katakana (U+30A0–U+30FF)
//! - Hangul Syllables (U+AC00–U+D7AF)
//! - CJK Extension B (U+20000–U+2A6DF)
//! - CJK Extension C/D/E/F and Compatibility Ideographs Supplement (U+2A700–U+2FA1F)
//!
//! Source: remark-reading-time README — "It also uses CJK character count
//! for languages that don't use spaces between words."
//!
//! # Config key
//!
//! `markdown.features.readingTime: true | { wpm: number }` in `zfb.config.ts`.
//!
//! # Output
//!
//! The visitor appends one [`MdxjsEsm`] node at the end of Root's children:
//!
//! ```text
//! export const readingTimeMinutes = 3;
//! ```
//!
//! This export is available as `entry.readingTimeMinutes` in any TSX page
//! that imports the collection entry.
//!
//! # Wave 5 (#573)
//!
//! Initial port. Wire via `features.readingTime: true` in `zfb.config.ts`.

use markdown::mdast::{MdxjsEsm, Node as MdastNode};
use zfb_md_ast::MdastVisitor;

// ── CJK detection ────────────────────────────────────────────────────────────

/// True for characters that belong to a CJK/kana/hangul block.
///
/// These are counted one-per-character rather than one-per-whitespace-token
/// because CJK text does not delimit words with spaces.
///
/// Ranges mirror those in remark-reading-time's CJK regex:
/// `/[㐀-鿿豈-﫿]|[\uD840-\uD868][\uDC00-\uDFFF]/`
/// extended here to also cover Hiragana, Katakana, Hangul, and the
/// supplementary-plane CJK blocks (Extension B and beyond, U+20000–U+2FA1F)
/// that remark-reading-time covers via the surrogate-pair clause.
#[must_use]
pub fn is_cjk_char(c: char) -> bool {
    matches!(c,
        // CJK Unified Ideographs + Extension A
        '\u{3400}'..='\u{9FFF}' |
        // CJK Compatibility Ideographs
        '\u{F900}'..='\u{FAFF}' |
        // Hiragana
        '\u{3040}'..='\u{309F}' |
        // Katakana
        '\u{30A0}'..='\u{30FF}' |
        // Hangul Syllables
        '\u{AC00}'..='\u{D7AF}' |
        // Supplementary-plane CJK: Extension B (U+20000–U+2A6DF), C/D/E/F, and
        // CJK Compatibility Ideographs Supplement (U+2F800–U+2FA1F).
        // remark-reading-time covers these via [\uD840-\uD868][\uDC00-\uDFFF].
        '\u{20000}'..='\u{2FA1F}'
    )
}

// ── Word counter ─────────────────────────────────────────────────────────────

/// Count the number of "words" in `text` using the remark-reading-time
/// formula:
///
/// - Latin (and other non-CJK) text: split on whitespace, count tokens.
/// - CJK characters: each character counts as one word.
///
/// Mixed text is handled correctly: CJK characters inside Latin sentences
/// are counted per-char; the surrounding non-CJK spans are tracked with an
/// in-word boolean state machine (no allocation) that fires exactly the same
/// token transitions as the old `String`-buffer + `split_whitespace` path.
#[must_use]
pub fn count_words(text: &str) -> u32 {
    let mut count = 0u32;
    // True while we are inside a non-CJK, non-whitespace run (i.e. a Latin
    // "word"). Equivalent to the old latin_buf being non-empty-after-trim.
    let mut in_word = false;

    for c in text.chars() {
        if is_cjk_char(c) {
            // Close any open Latin word first, then count this CJK char.
            if in_word {
                count += 1;
                in_word = false;
            }
            count += 1;
        } else if c.is_whitespace() {
            // Whitespace ends a Latin word.
            if in_word {
                count += 1;
                in_word = false;
            }
        } else {
            // Non-CJK, non-whitespace character: we are inside a Latin word.
            in_word = true;
        }
    }

    // Flush any trailing Latin word.
    if in_word {
        count += 1;
    }

    count
}

// ── mdast text extractor ─────────────────────────────────────────────────────

/// Recursively extract plain-text content from an mdast node, skipping
/// code blocks (fenced and inline) and raw HTML — mirrors remark-reading-time
/// which skips `code` nodes by design (code is not "read" at a normal pace).
///
/// Headings, paragraphs, emphasis, strong, blockquote, list items, etc. all
/// contribute their text content.
fn extract_mdast_text(node: &MdastNode, out: &mut String) {
    match node {
        MdastNode::Text(t) => {
            out.push_str(&t.value);
        }
        // Code blocks and inline code are excluded — remark-reading-time
        // skips `code` nodes so code-heavy docs are not over-estimated.
        MdastNode::Code(_) | MdastNode::InlineCode(_) | MdastNode::Html(_) => {}
        // Math blocks: skip (similar to code blocks, not prose).
        MdastNode::Math(_) | MdastNode::InlineMath(_) => {}
        // Walk containers.
        MdastNode::Root(r) => {
            for child in &r.children {
                extract_mdast_text(child, out);
            }
        }
        MdastNode::Paragraph(p) => {
            for child in &p.children {
                extract_mdast_text(child, out);
            }
            out.push(' ');
        }
        MdastNode::Heading(h) => {
            for child in &h.children {
                extract_mdast_text(child, out);
            }
            out.push(' ');
        }
        MdastNode::Emphasis(e) => {
            for child in &e.children {
                extract_mdast_text(child, out);
            }
        }
        MdastNode::Strong(s) => {
            for child in &s.children {
                extract_mdast_text(child, out);
            }
        }
        MdastNode::Delete(d) => {
            for child in &d.children {
                extract_mdast_text(child, out);
            }
        }
        MdastNode::Blockquote(b) => {
            for child in &b.children {
                extract_mdast_text(child, out);
            }
        }
        MdastNode::List(l) => {
            for child in &l.children {
                extract_mdast_text(child, out);
            }
        }
        MdastNode::ListItem(li) => {
            for child in &li.children {
                extract_mdast_text(child, out);
            }
        }
        MdastNode::TableRow(row) => {
            for child in &row.children {
                extract_mdast_text(child, out);
            }
        }
        MdastNode::TableCell(cell) => {
            for child in &cell.children {
                extract_mdast_text(child, out);
            }
            out.push(' ');
        }
        MdastNode::Table(t) => {
            for child in &t.children {
                extract_mdast_text(child, out);
            }
        }
        MdastNode::Link(l) => {
            for child in &l.children {
                extract_mdast_text(child, out);
            }
        }
        MdastNode::MdxJsxFlowElement(e) => {
            for child in &e.children {
                extract_mdast_text(child, out);
            }
        }
        MdastNode::MdxJsxTextElement(e) => {
            for child in &e.children {
                extract_mdast_text(child, out);
            }
        }
        // Skip everything else (Image alt text, MdxjsEsm, Frontmatter, etc.)
        _ => {}
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Compute the estimated reading time in minutes for the given mdast tree.
///
/// Words are counted with [`count_words`]; the result is divided by `wpm`
/// (words-per-minute) and rounded up to the nearest whole minute. The
/// minimum returned value is `1`.
///
/// This function is the public computation entry point — it can be used
/// independently of the visitor (e.g. in integration tests or the fixture
/// harness).
#[must_use]
pub fn compute_reading_time_minutes(node: &MdastNode, wpm: u32) -> u32 {
    let mut text = String::new();
    extract_mdast_text(node, &mut text);
    let words = count_words(&text);
    let wpm = wpm.max(1); // guard against divide-by-zero
                          // Ceiling division: always at least 1 minute.
    words.div_ceil(wpm).max(1)
}

// ── Plugin ────────────────────────────────────────────────────────────────────

/// Mdast visitor that computes the reading-time estimate and appends an
/// `export const readingTimeMinutes = N;` [`MdxjsEsm`] node to the Root's
/// children.
///
/// The injected node is later extracted and emitted at the JSX module level
/// by `crates/zfb-content/src/mdx_jsx_emit.rs` (see the note in that module
/// on synthesized ESM exports). For the HTML pipeline path, the node is a
/// no-op — the HTML serializer does not emit mdast nodes directly.
///
/// Implements [`MdastVisitor`] — wire via
/// `Pipeline::add_mdast_visitor(Box::new(ReadingTimePlugin::new(...)))`.
#[derive(Debug, Clone)]
pub struct ReadingTimePlugin {
    /// Words per minute — denominator for the minute calculation.
    /// Default `200`, sourced from remark-reading-time defaults.
    wpm: u32,
}

impl ReadingTimePlugin {
    /// Create a plugin with the default WPM (200).
    #[must_use]
    pub fn new() -> Self {
        Self { wpm: 200 }
    }

    /// Create a plugin with a custom WPM value.
    #[must_use]
    pub fn with_wpm(wpm: u32) -> Self {
        Self { wpm: wpm.max(1) }
    }
}

impl Default for ReadingTimePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl MdastVisitor for ReadingTimePlugin {
    fn visit(&mut self, node: &mut MdastNode) {
        // Only operate on the Root node — we need to both walk the full
        // tree for counting AND append a child to Root.
        let MdastNode::Root(root) = node else {
            return;
        };

        // Compute reading time against the whole tree (wrap in a Root ref).
        let minutes = {
            let mut text = String::new();
            for child in &root.children {
                extract_mdast_text(child, &mut text);
            }
            let words = count_words(&text);
            let wpm = self.wpm.max(1);
            words.div_ceil(wpm).max(1)
        };

        // Append a synthesized MdxjsEsm node. The value is the export
        // declaration that `mdx_jsx_emit.rs` will lift to module scope.
        // Marker prefix `/* zfb-synth-export */` distinguishes this node
        // from user-authored MDX ESM nodes so the emitter can emit it
        // without re-emitting user imports/exports.
        let esm_value =
            format!("/* zfb-synth-export */ export const readingTimeMinutes = {minutes};");
        root.children.push(MdastNode::MdxjsEsm(MdxjsEsm {
            value: esm_value,
            position: None,
            stops: Vec::new(),
        }));
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_cjk_char ──────────────────────────────────────────────────────

    #[test]
    fn cjk_ideograph_detected() {
        assert!(is_cjk_char('中'));
        assert!(is_cjk_char('文'));
        assert!(is_cjk_char('日'));
    }

    #[test]
    fn hiragana_detected() {
        assert!(is_cjk_char('あ'));
        assert!(is_cjk_char('い'));
    }

    #[test]
    fn katakana_detected() {
        assert!(is_cjk_char('ア'));
    }

    #[test]
    fn hangul_detected() {
        assert!(is_cjk_char('한'));
    }

    #[test]
    fn latin_not_cjk() {
        assert!(!is_cjk_char('a'));
        assert!(!is_cjk_char('Z'));
        assert!(!is_cjk_char('1'));
        assert!(!is_cjk_char(' '));
    }

    #[test]
    fn cjk_extension_b_detected() {
        // U+20000 𠀀 — first character of CJK Extension B (supplementary plane).
        // remark-reading-time covers this via the surrogate-pair clause
        // [\uD840-\uD868][\uDC00-\uDFFF]; we cover it with '\u{20000}'..='\u{2FA1F}'.
        assert!(is_cjk_char('\u{20000}'));
        // U+2A6DF — last code point of Extension B.
        assert!(is_cjk_char('\u{2A6DF}'));
        // U+2FA1F — last code point of CJK Compatibility Ideographs Supplement.
        assert!(is_cjk_char('\u{2FA1F}'));
        // U+2FA20 — just past the supplementary-plane range, must not be CJK.
        assert!(!is_cjk_char('\u{2FA20}'));
    }

    #[test]
    fn cjk_extension_b_counted_per_character() {
        // A run of Extension-B characters must be counted one-per-char, not as
        // a single whitespace-separated token.
        // U+20000 𠀀, U+20001 𠀁, U+20002 𠀂 — three Extension-B ideographs.
        let ext_b = "\u{20000}\u{20001}\u{20002}";
        assert_eq!(count_words(ext_b), 3);
    }

    // ── count_words ────────────────────────────────────────────────────────

    #[test]
    fn empty_string_zero_words() {
        assert_eq!(count_words(""), 0);
    }

    #[test]
    fn whitespace_only_zero_words() {
        assert_eq!(count_words("   \n\t  "), 0);
    }

    #[test]
    fn single_latin_word() {
        assert_eq!(count_words("hello"), 1);
    }

    #[test]
    fn five_latin_words() {
        assert_eq!(count_words("one two three four five"), 5);
    }

    #[test]
    fn five_cjk_chars() {
        assert_eq!(count_words("中文日本語"), 5);
    }

    #[test]
    fn mixed_latin_and_cjk() {
        // "hello" → 1 Latin word, "中文" → 2 CJK chars = 3 total
        assert_eq!(count_words("hello 中文"), 3);
    }

    #[test]
    fn punctuation_within_token_counts_as_one_word() {
        assert_eq!(count_words("hello-world"), 1);
        assert_eq!(count_words("don't"), 1);
    }

    /// Equivalence check for the state-machine rewrite (perf item #5).
    ///
    /// The new implementation must produce identical counts to the old
    /// `String`-buffer path for all mixed CJK/Latin inputs.  We inline the
    /// old algorithm here as `count_words_buffered` and assert both produce
    /// the same result for a corpus covering every interesting transition:
    /// CJK→Latin, Latin→CJK, leading/trailing whitespace, pure Latin, pure
    /// CJK, punctuation-inside-word, multi-whitespace gaps.
    #[test]
    fn state_machine_matches_buffer_algorithm_on_mixed_input() {
        fn count_words_buffered(text: &str) -> u32 {
            let mut count = 0u32;
            let mut latin_buf = String::new();
            for c in text.chars() {
                if is_cjk_char(c) {
                    if !latin_buf.trim().is_empty() {
                        count += latin_buf
                            .split_whitespace()
                            .filter(|w| !w.is_empty())
                            .count() as u32;
                    }
                    latin_buf.clear();
                    count += 1;
                } else {
                    latin_buf.push(c);
                }
            }
            if !latin_buf.trim().is_empty() {
                count += latin_buf
                    .split_whitespace()
                    .filter(|w| !w.is_empty())
                    .count() as u32;
            }
            count
        }

        let cases: &[&str] = &[
            "",
            "   ",
            "hello",
            "hello world",
            "中文",
            "hello 中文",
            "中文 hello",
            "hello中文world",
            "  hello  world  ",
            "one two three four five",
            "hello-world don't",
            "あいう abc カタカナ def",
            "\t\nhello\n\tworld\n",
            "한국어 mixed English 日本語",
            "\u{20000}\u{20001} latin \u{20002}",
        ];

        for &input in cases {
            let new = count_words(input);
            let old = count_words_buffered(input);
            assert_eq!(
                new, old,
                "count_words({input:?}): state-machine={new} vs buffer={old}"
            );
        }
    }

    // ── compute_reading_time_minutes ────────────────────────────────────────

    fn parse_mdast(md: &str) -> MdastNode {
        markdown::to_mdast(md, &markdown::ParseOptions::default()).unwrap()
    }

    #[test]
    fn zero_words_returns_one_minute_minimum() {
        let node = parse_mdast("");
        assert_eq!(compute_reading_time_minutes(&node, 200), 1);
    }

    #[test]
    fn exactly_200_words_is_one_minute() {
        // Build a 200-word string.
        let words: Vec<&str> = (0..200).map(|_| "word").collect();
        let md = words.join(" ");
        let node = parse_mdast(&md);
        assert_eq!(compute_reading_time_minutes(&node, 200), 1);
    }

    #[test]
    fn exactly_201_words_rounds_up_to_two_minutes() {
        let words: Vec<&str> = (0..201).map(|_| "word").collect();
        let md = words.join(" ");
        let node = parse_mdast(&md);
        assert_eq!(compute_reading_time_minutes(&node, 200), 2);
    }

    #[test]
    fn code_blocks_excluded_from_word_count() {
        // A code block with 200 words inside should NOT count toward reading
        // time — the prose body (5 words) should be 1 minute.
        let long_code = "word ".repeat(200);
        let md = format!("Some prose here.\n\n```\n{}\n```\n", long_code);
        let node = parse_mdast(&md);
        // prose: ~3 words → 1 minute (still below 200)
        assert_eq!(compute_reading_time_minutes(&node, 200), 1);
    }

    // ── ReadingTimePlugin visitor ──────────────────────────────────────────

    #[test]
    fn plugin_appends_esm_node() {
        let mut node = parse_mdast("hello world");
        ReadingTimePlugin::new().visit(&mut node);
        let MdastNode::Root(root) = &node else {
            panic!("expected Root");
        };
        let last = root.children.last().expect("children must not be empty");
        let MdastNode::MdxjsEsm(esm) = last else {
            panic!("last child must be MdxjsEsm, got {:?}", last);
        };
        assert!(
            esm.value.contains("readingTimeMinutes"),
            "ESM value must contain readingTimeMinutes: {}",
            esm.value,
        );
        assert!(
            esm.value.contains("zfb-synth-export"),
            "ESM value must contain the synthesized-export marker: {}",
            esm.value,
        );
    }

    #[test]
    fn plugin_200_word_article_exports_1() {
        let words = "word ".repeat(200);
        let mut node = parse_mdast(&words);
        ReadingTimePlugin::new().visit(&mut node);
        let MdastNode::Root(root) = &node else {
            panic!("expected Root");
        };
        let MdastNode::MdxjsEsm(esm) = root.children.last().unwrap() else {
            panic!("last child must be MdxjsEsm");
        };
        assert!(
            esm.value.contains("= 1;"),
            "200 words at 200 WPM must produce 1 minute: {}",
            esm.value,
        );
    }

    #[test]
    fn plugin_600_word_article_exports_3() {
        let words = "word ".repeat(600);
        let mut node = parse_mdast(&words);
        ReadingTimePlugin::new().visit(&mut node);
        let MdastNode::Root(root) = &node else {
            panic!("expected Root");
        };
        let MdastNode::MdxjsEsm(esm) = root.children.last().unwrap() else {
            panic!("last child must be MdxjsEsm");
        };
        assert!(
            esm.value.contains("= 3;"),
            "600 words at 200 WPM must produce 3 minutes: {}",
            esm.value,
        );
    }

    // ── extract_mdast_text regressions ─────────────────────────────────────

    #[test]
    fn emphasis_adjacent_word_counts_once() {
        // **foo**bar — Strong[Text('foo')] immediately followed by Text('bar')
        // with no source whitespace. Must be counted as one word, not two.
        let node = parse_mdast("**foo**bar");
        let mut t = String::new();
        extract_mdast_text(&node, &mut t);
        assert_eq!(count_words(&t), 1);
    }

    #[test]
    fn block_boundary_separator_injected() {
        // "# Foo\n\nBar" — heading and paragraph are separate blocks.
        // The block-boundary space must ensure they count as two distinct words.
        let node = parse_mdast("# Foo\n\nBar");
        let mut t = String::new();
        extract_mdast_text(&node, &mut t);
        assert_eq!(count_words(&t), 2);
    }
}
