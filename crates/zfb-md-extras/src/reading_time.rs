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
/// `/[㐀-鿿豈-﫿]|[\uD840-\uD868][\uDC00-\uDFFF]/`
/// extended here to also cover Hiragana, Katakana, and Hangul.
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
        '\u{AC00}'..='\u{D7AF}'
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
/// are extracted first, and the remaining (de-CJK'd) parts are split on
/// whitespace.
#[must_use]
pub fn count_words(text: &str) -> u32 {
    let mut count = 0u32;
    let mut latin_buf = String::new();

    for c in text.chars() {
        if is_cjk_char(c) {
            // Flush any accumulated Latin-script buffer first.
            if !latin_buf.trim().is_empty() {
                count += latin_buf
                    .split_whitespace()
                    .filter(|w| !w.is_empty())
                    .count() as u32;
            }
            latin_buf.clear();
            // Each CJK character is one word.
            count += 1;
        } else {
            latin_buf.push(c);
        }
    }

    // Flush any trailing Latin-script content.
    if !latin_buf.trim().is_empty() {
        count += latin_buf
            .split_whitespace()
            .filter(|w| !w.is_empty())
            .count() as u32;
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
            out.push(' ');
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
        }
        MdastNode::Heading(h) => {
            for child in &h.children {
                extract_mdast_text(child, out);
            }
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
    ((words + wpm - 1) / wpm).max(1)
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
            ((words + wpm - 1) / wpm).max(1)
        };

        // Append a synthesized MdxjsEsm node. The value is the export
        // declaration that `mdx_jsx_emit.rs` will lift to module scope.
        // Marker prefix `/* zfb-synth-export */` distinguishes this node
        // from user-authored MDX ESM nodes so the emitter can emit it
        // without re-emitting user imports/exports.
        let esm_value = format!(
            "/* zfb-synth-export */ export const readingTimeMinutes = {minutes};"
        );
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
        let long_code: String = std::iter::repeat("word ")
            .take(200)
            .collect::<String>();
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
        let words: String = std::iter::repeat("word ").take(200).collect();
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
        let words: String = std::iter::repeat("word ").take(600).collect();
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
}
