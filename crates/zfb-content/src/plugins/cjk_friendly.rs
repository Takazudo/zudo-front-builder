//! CJK-aware emphasis/strong tokenisation post-processor.
//!
//! Rust port of [`remark-cjk-friendly`][upstream] (and the broader
//! [markdown-cjk-friendly] family). CommonMark's left-/right-flanking
//! delimiter run rules require the character on the "outer" side of an
//! emphasis run to be either whitespace or Unicode punctuation; CJK
//! ideographs and kana/hangul are letters (Unicode `Lo`/etc.) which are
//! neither, so a `**` adjacent to them does not flank — and the run is
//! left as literal `*` text in the mdast.
//!
//! This breaks `これは**重要。**テスト` (the closing `**` is preceded by
//! `。`, a CJK punctuation, and followed by `テ`, a kana — so CommonMark
//! says "punctuation followed by non-punctuation/non-whitespace" → does
//! NOT close emphasis). The CJK-friendly amendment ([spec][spec])
//! changes the rule so a CJK character on the outside is treated as a
//! valid flanker, exactly as ASCII whitespace would be.
//!
//! `markdown-rs` parses to mdast and does not expose the underlying
//! micromark tokeniser, so the cleanest port is a post-processor: walk
//! the mdast, find `Text` nodes that still contain literal `*`/`**`
//! runs (because micromark rejected them under standard CommonMark
//! rules), and re-tokenise them under the CJK-friendly amendment. The
//! visitor recurses into inline-bearing containers (`Paragraph`,
//! `Heading`, `Strong`, `Emphasis`, `Delete`, `Blockquote`, `Link`,
//! `LinkReference`, `ListItem`, `List`, `Root`) but does NOT enter:
//!
//! - `Code` and `InlineCode` — verbatim content per CommonMark.
//! - `Html` — raw HTML passthrough.
//! - `MdxJsxFlowElement` / `MdxJsxTextElement` — JSX bodies are author-
//!   controlled.
//! - `MdxFlowExpression` / `MdxTextExpression` — `{...}` JS code.
//!
//! Place this plugin in [`Pipeline::with_defaults`] BEFORE any visitor
//! that depends on emphasis being already tokenised (e.g. visitors that
//! synthesize anchor labels from heading text and might miss children
//! we have just inserted).
//!
//! [upstream]: https://www.npmjs.com/package/remark-cjk-friendly
//! [markdown-cjk-friendly]: https://github.com/tats-u/markdown-cjk-friendly
//! [spec]: https://github.com/tats-u/markdown-cjk-friendly/blob/main/specification.md
//! [`Pipeline::with_defaults`]: crate::pipeline::Pipeline::with_defaults

use markdown::mdast::{Emphasis, Node as MdastNode, Strong, Text};

use crate::pipeline::MdastVisitor;

/// Visitor that rewrites Text nodes containing CJK-flanked `*` / `**`
/// runs into proper Emphasis/Strong mdast nodes.
#[derive(Debug, Default, Clone, Copy)]
pub struct CjkFriendlyPlugin;

impl CjkFriendlyPlugin {
    /// New visitor; stateless.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl MdastVisitor for CjkFriendlyPlugin {
    fn visit(&mut self, node: &mut MdastNode) {
        rewrite_inline_children(node);
    }
}

/// Top-level dispatcher: if `node` is a no-recurse boundary, stop;
/// otherwise rewrite its children (if it has inline children) and
/// recurse into each child.
fn rewrite_inline_children(node: &mut MdastNode) {
    if is_no_recurse(node) {
        return;
    }

    // Rewrite the children list at the current level so any literal
    // `*`/`**` runs inside Text nodes become Emphasis/Strong. This must
    // happen before recursing, because newly-created Emphasis/Strong
    // children themselves need to be walked (CJK markers can nest).
    if has_inline_children(node) {
        if let Some(children) = node.children_mut() {
            let new_children = retokenise_children(std::mem::take(children));
            *children = new_children;
        }
    }

    if let Some(children) = node.children_mut() {
        for child in children {
            rewrite_inline_children(child);
        }
    }
}

/// Nodes whose subtree must NOT be re-tokenised. The visitor stops at
/// these — neither rewriting their direct children nor recursing into
/// them. See module docs for the rationale.
fn is_no_recurse(node: &MdastNode) -> bool {
    matches!(
        node,
        MdastNode::Code(_)
            | MdastNode::InlineCode(_)
            | MdastNode::Html(_)
            | MdastNode::MdxJsxFlowElement(_)
            | MdastNode::MdxJsxTextElement(_)
            | MdastNode::MdxFlowExpression(_)
            | MdastNode::MdxTextExpression(_)
    )
}

/// True if `node` is a container whose children form an inline run.
///
/// Block containers (Root, ListItem, etc.) also flow through here —
/// their non-text children are recursed into separately. The
/// re-tokenisation pass is a no-op when no child is a Text containing
/// `*`, so over-inclusion is harmless.
fn has_inline_children(node: &MdastNode) -> bool {
    matches!(
        node,
        MdastNode::Root(_)
            | MdastNode::Paragraph(_)
            | MdastNode::Heading(_)
            | MdastNode::Strong(_)
            | MdastNode::Emphasis(_)
            | MdastNode::Delete(_)
            | MdastNode::Blockquote(_)
            | MdastNode::Link(_)
            | MdastNode::LinkReference(_)
            | MdastNode::ListItem(_)
            | MdastNode::List(_)
            | MdastNode::FootnoteDefinition(_)
            | MdastNode::TableRow(_)
            | MdastNode::TableCell(_)
            | MdastNode::Table(_)
    )
}

/// Re-tokenise each Text child for CJK-flanked emphasis markers. Other
/// children pass through unchanged.
fn retokenise_children(children: Vec<MdastNode>) -> Vec<MdastNode> {
    let mut out = Vec::with_capacity(children.len());
    for child in children {
        match child {
            MdastNode::Text(t) if t.value.contains('*') => {
                out.extend(retokenise_text(&t.value));
            }
            other => out.push(other),
        }
    }
    out
}

/// Re-tokenise a single Text value's `*`/`**` markers using
/// CJK-friendly flanking rules. Returns a vec of mdast nodes
/// (Text/Emphasis/Strong) covering the whole input string.
///
/// markdown-rs has already handled all standard-CommonMark cases —
/// anything left here is text that micromark refused to tokenise. We
/// scan left-to-right and greedily match the first valid CJK-friendly
/// emphasis run, recursing into its inner content and emitting plain
/// Text for everything else.
fn retokenise_text(value: &str) -> Vec<MdastNode> {
    let chars: Vec<char> = value.chars().collect();
    let mut out: Vec<MdastNode> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '*' {
            let run_len = count_run(&chars, i, '*');
            // We only handle `*` (em) and `**` (strong) markers. `***`
            // and longer runs are split by the inner search itself —
            // try to open with the longest viable marker.
            if let Some((marker_len, end_open, close_start, close_end)) =
                find_match(&chars, i, run_len)
            {
                if !buf.is_empty() {
                    out.push(text_node(std::mem::take(&mut buf)));
                }
                let inner: String = chars[end_open..close_start].iter().collect();
                let inner_children = retokenise_text(&inner);
                let wrapped = wrap(marker_len, inner_children);
                out.push(wrapped);
                i = close_end;
                continue;
            }
            // No CJK-friendly match — emit the marker run as literal
            // text. Consume the WHOLE run so we don't loop.
            buf.extend(chars.iter().skip(i).take(run_len));
            i += run_len;
            continue;
        }
        buf.push(chars[i]);
        i += 1;
    }
    if !buf.is_empty() {
        out.push(text_node(buf));
    }
    out
}

/// Count the length of a run of identical characters starting at `i`.
fn count_run(chars: &[char], i: usize, c: char) -> usize {
    let mut n = 0;
    while i + n < chars.len() && chars[i + n] == c {
        n += 1;
    }
    n
}

/// Find the matching close for an opening `*` run starting at `open_i`
/// with run length `open_run`. Tries `**` first (when at least 2 stars
/// are available), then `*`. Returns `(marker_len, inner_start,
/// close_start, close_end)` on success.
fn find_match(
    chars: &[char],
    open_i: usize,
    open_run: usize,
) -> Option<(usize, usize, usize, usize)> {
    // Try strong (`**`) before em (`*`) — matches markdown-rs's
    // greedy-strong preference.
    for marker_len in [2usize, 1usize] {
        if open_run < marker_len {
            continue;
        }
        let inner_start = open_i + marker_len;
        // Opening marker must be followed by a non-whitespace,
        // non-marker character (CommonMark + CJK-friendly: 2bγ).
        let inner_first = chars.get(inner_start).copied();
        if !inner_first.is_some_and(is_valid_inner_char) {
            continue;
        }
        // Outer character on the left of the open marker.
        let left_outer = chars.get(open_i.wrapping_sub(1)).copied();
        if !is_left_flank_outer_ok(left_outer) {
            continue;
        }

        // Search for the matching close marker. Skip any opening
        // markers we encounter — micromark would have nested them, but
        // we are post-processing what micromark already left as text.
        let mut j = inner_start;
        while j < chars.len() {
            if chars[j] != '*' {
                j += 1;
                continue;
            }
            let run = count_run(chars, j, '*');
            if run < marker_len {
                j += run;
                continue;
            }
            // Candidate close marker is the LAST `marker_len` chars of
            // the run (so triple `***` after `**X` could close `**`).
            let close_start = j + run - marker_len;
            let close_end = j + run;
            // Must not be empty content.
            if close_start <= inner_start {
                j += run;
                continue;
            }
            let inner_last = chars.get(close_start - 1).copied();
            let right_outer = chars.get(close_end).copied();
            if is_valid_inner_char(inner_last.unwrap_or(' '))
                && is_right_flank_outer_ok(right_outer)
                && is_cjk_friendly_close(inner_last, right_outer)
            {
                return Some((marker_len, inner_start, close_start, close_end));
            }
            j += run;
        }
    }
    None
}

/// The character right after an opening `*`/`**` (or right before a
/// closing one) must not be whitespace and must not itself be a `*`
/// (otherwise it's part of a longer marker run).
fn is_valid_inner_char(c: char) -> bool {
    !c.is_whitespace() && c != '*'
}

/// Outer-left of an opening marker: standard CommonMark left-flanking
/// requires the left-outer to be (start-of-line | whitespace | Unicode
/// punctuation). The CJK-friendly amendment additionally permits CJK
/// characters. We accept any char that isn't `*` or NUL — the close
/// match also enforces that ONE side is CJK-flanked, which is what
/// distinguishes our amendment from base CommonMark.
fn is_left_flank_outer_ok(left: Option<char>) -> bool {
    match left {
        None => true,
        Some('*') => false,
        Some(_) => true,
    }
}

/// Outer-right of a closing marker: standard CommonMark right-flanking
/// rule. We accept the same set as `is_left_flank_outer_ok`; the
/// CJK-friendly check below enforces the actual flanking semantics.
fn is_right_flank_outer_ok(right: Option<char>) -> bool {
    match right {
        None => true,
        Some('*') => false,
        Some(_) => true,
    }
}

/// CJK-friendly closing predicate.
///
/// markdown-rs has already consumed every match that passes standard
/// CommonMark flanking; whatever is left in a Text node containing `*`
/// failed CommonMark's rules. The amendment we implement says: a `**`
/// closes emphasis when the character JUST INSIDE the close (immediately
/// before the markers) is a CJK punctuation character AND the character
/// JUST OUTSIDE (immediately after the markers) is a CJK character — or
/// vice versa around the open marker.
///
/// In practice the visible failure mode is the inner-side CJK
/// punctuation case (`**X。**Y`). To stay narrow and avoid double-
/// tokenising anything CommonMark already accepted, we require AT LEAST
/// ONE side of the close to be CJK-flank-eligible. Concretely: the
/// closing marker counts as CJK-friendly right-flanking iff
/// `inner_last` is a CJK punctuation AND `right_outer` is a CJK char,
/// OR `right_outer` is a CJK char and `inner_last` is anything
/// non-whitespace.
///
/// The asymmetric "either side is CJK" check is the same condition the
/// `markdown-cjk-friendly` reference uses: 2bγ ("followed by a CJK
/// character") on the right, 2bγ ("preceded by a CJK sequence") on the
/// left.
fn is_cjk_friendly_close(inner_last: Option<char>, right_outer: Option<char>) -> bool {
    let inner = match inner_last {
        Some(c) => c,
        None => return false,
    };
    // If neither side touches a CJK character, the standard CommonMark
    // tokeniser would have already accepted (or rejected) this — we
    // shouldn't second-guess it. Require at least one CJK-flank.
    let inner_is_cjk = is_cjk(inner);
    let inner_is_cjk_punct = inner_is_cjk && is_unicode_punctuation(inner);
    let outer_is_cjk = right_outer.is_some_and(is_cjk);

    // Case A (canonical remark-cjk-friendly fix): inner is CJK
    // punctuation, outer is a CJK character. Standard CommonMark
    // refused because `inner_last` is Unicode punctuation and outer is
    // not Unicode punctuation/whitespace; CJK-friendly accepts because
    // outer is a CJK character (rule 2bγ).
    if inner_is_cjk_punct && outer_is_cjk {
        return true;
    }
    // Case B: inner is a CJK alphanumeric, outer is end-of-line or
    // anything that satisfies stock right-flanking already. markdown-rs
    // already covered this — we should not retokenise.
    // Case C: inner is a non-CJK char that happens to be Unicode
    // punctuation (e.g. `.`), outer is CJK. This is the ASCII-period-
    // before-close case (`**X.**Y` with Y being CJK). The reference
    // implementation also accepts this.
    if outer_is_cjk && is_unicode_punctuation(inner) {
        return true;
    }
    false
}

/// Build a Text node.
fn text_node(value: String) -> MdastNode {
    MdastNode::Text(Text {
        value,
        position: None,
    })
}

/// Wrap children in `Strong` (marker_len == 2) or `Emphasis` (== 1).
fn wrap(marker_len: usize, children: Vec<MdastNode>) -> MdastNode {
    if marker_len == 2 {
        MdastNode::Strong(Strong {
            children,
            position: None,
        })
    } else {
        MdastNode::Emphasis(Emphasis {
            children,
            position: None,
        })
    }
}

// --- CJK character classification ---------------------------------

/// True if `c` is a CJK character per the Unicode 17 ranges used by
/// the [markdown-cjk-friendly] reference. Generated from
/// `node --run print-ranges` against UAX #11 East Asian Width
/// `W`/`F`/`H` minus default-emoji-presentation, plus the Hangul
/// script. See `ranges.md` upstream.
///
/// [markdown-cjk-friendly]: https://github.com/tats-u/markdown-cjk-friendly
#[must_use]
pub fn is_cjk(c: char) -> bool {
    let cp = c as u32;
    // Ranges sorted ascending; matches the reference C/JS table.
    matches!(
        cp,
        0x1100..=0x11FF
            | 0x20A9
            | 0x2329..=0x232A
            | 0x2630..=0x2637
            | 0x268A..=0x268F
            | 0x2E80..=0x2E99
            | 0x2E9B..=0x2EF3
            | 0x2F00..=0x2FD5
            | 0x2FF0..=0x303E
            | 0x3041..=0x3096
            | 0x3099..=0x30FF
            | 0x3105..=0x312F
            | 0x3131..=0x318E
            | 0x3190..=0x31E5
            | 0x31EF..=0x321E
            | 0x3220..=0x3247
            | 0x3250..=0xA48C
            | 0xA490..=0xA4C6
            | 0xA960..=0xA97C
            | 0xAC00..=0xD7A3
            | 0xD7B0..=0xD7C6
            | 0xD7CB..=0xD7FB
            | 0xF900..=0xFAFF
            | 0xFE10..=0xFE19
            | 0xFE30..=0xFE52
            | 0xFE54..=0xFE66
            | 0xFE68..=0xFE6B
            | 0xFF01..=0xFFBE
            | 0xFFC2..=0xFFC7
            | 0xFFCA..=0xFFCF
            | 0xFFD2..=0xFFD7
            | 0xFFDA..=0xFFDC
            | 0xFFE0..=0xFFE6
            | 0xFFE8..=0xFFEE
            | 0x16FE0..=0x16FE4
            | 0x16FF0..=0x16FF6
            | 0x17000..=0x18CD5
            | 0x18CFF..=0x18D1E
            | 0x18D80..=0x18DF2
            | 0x1AFF0..=0x1AFF3
            | 0x1AFF5..=0x1AFFB
            | 0x1AFFD..=0x1AFFE
            | 0x1B000..=0x1B122
            | 0x1B132
            | 0x1B150..=0x1B152
            | 0x1B155
            | 0x1B164..=0x1B167
            | 0x1B170..=0x1B2FB
            | 0x1D300..=0x1D356
            | 0x1D360..=0x1D376
            | 0x1F200
            | 0x1F202
            | 0x1F210..=0x1F219
            | 0x1F21B..=0x1F22E
            | 0x1F230..=0x1F231
            | 0x1F237
            | 0x1F23B
            | 0x1F240..=0x1F248
            | 0x1F260..=0x1F265
            | 0x20000..=0x3FFFD
    )
}

/// Approximate the CommonMark "Unicode punctuation" predicate.
///
/// CommonMark defines Unicode punctuation as Unicode classes
/// `Pc`/`Pd`/`Pe`/`Pf`/`Pi`/`Po`/`Ps`/`Sc`/`Sk`/`Sm`/`So`. Pulling in
/// `unicode_categories` for the full table would add a dependency for
/// one predicate; we instead approximate with ASCII-punctuation +
/// the CJK punctuation ranges we already classify above. That is
/// sufficient for the CJK-friendly amendment because the only branch
/// of `is_cjk_friendly_close` that consults this function is gated on
/// `outer_is_cjk` — so we just need to recognise the classic problem
/// punctuation (`。`, `、`, `：`, `；`, `！`, `？`, ASCII `. , : ; ! ?` ,
/// fullwidth brackets, etc.), all of which are covered by ASCII
/// punctuation OR our CJK ranges.
fn is_unicode_punctuation(c: char) -> bool {
    if c.is_ascii_punctuation() {
        return true;
    }
    // CJK punctuation block (U+3000..U+303F) is partly inside our CJK
    // ranges. Anything in the CJK range that is also a typical
    // punctuation mark we treat as Unicode punctuation. Restrict the
    // check to the well-known fullwidth-and-CJK punctuation blocks so
    // we don't mis-classify ideographs.
    matches!(
        c as u32,
        0x3000..=0x303F     // CJK Symbols and Punctuation
            | 0xFF01..=0xFF0F  // Fullwidth ASCII punctuation (! .. /)
            | 0xFF1A..=0xFF20  // (: .. @)
            | 0xFF3B..=0xFF40  // ([ .. `)
            | 0xFF5B..=0xFF65  // ({ .. ･)
            | 0xFE30..=0xFE4F  // CJK Compatibility Forms
            | 0xFE50..=0xFE6F  // Small Form Variants + half/full punctuation
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &str) -> MdastNode {
        let mut mdast = markdown::to_mdast(input, &markdown::ParseOptions::mdx())
            .expect("markdown-rs should parse fixture");
        CjkFriendlyPlugin::new().visit(&mut mdast);
        mdast
    }

    fn first_paragraph_children(node: &MdastNode) -> &[MdastNode] {
        let MdastNode::Root(r) = node else {
            unreachable!("expected Root, got {node:?}")
        };
        let MdastNode::Paragraph(p) = &r.children[0] else {
            unreachable!("expected Paragraph, got {:?}", r.children[0])
        };
        &p.children
    }

    fn flatten_text(node: &MdastNode, out: &mut String) {
        match node {
            MdastNode::Text(t) => out.push_str(&t.value),
            MdastNode::Strong(s) => {
                out.push_str("[STRONG:");
                for c in &s.children {
                    flatten_text(c, out);
                }
                out.push(']');
            }
            MdastNode::Emphasis(e) => {
                out.push_str("[EM:");
                for c in &e.children {
                    flatten_text(c, out);
                }
                out.push(']');
            }
            other => out.push_str(&format!("{other:?}")),
        }
    }

    fn dump(node: &MdastNode) -> String {
        let mut out = String::new();
        for c in first_paragraph_children(node) {
            flatten_text(c, &mut out);
        }
        out
    }

    // --- Cases markdown-rs already handled (must NOT regress). ---

    #[test]
    fn standard_japanese_strong_unchanged() {
        // markdown-rs already tokenises this correctly; the visitor
        // must be a no-op.
        let h = run("これは**重要**な機能です");
        assert_eq!(dump(&h), "これは[STRONG:重要]な機能です");
    }

    #[test]
    fn standard_chinese_emphasis_unchanged() {
        let h = run("中文 *斜体* 中文");
        assert_eq!(dump(&h), "中文 [EM:斜体] 中文");
    }

    #[test]
    fn standard_korean_strong_unchanged() {
        let h = run("한국어**강조**한국어");
        assert_eq!(dump(&h), "한국어[STRONG:강조]한국어");
    }

    #[test]
    fn fullwidth_brackets_unchanged() {
        let h = run("「**重要**」");
        assert_eq!(dump(&h), "「[STRONG:重要]」");
        let h = run("（**強調**）");
        assert_eq!(dump(&h), "（[STRONG:強調]）");
    }

    #[test]
    fn ascii_only_emphasis_unchanged() {
        let h = run("This is **bold** text");
        assert_eq!(dump(&h), "This is [STRONG:bold] text");
    }

    #[test]
    fn mixed_script_unchanged() {
        let h = run("日本語 mixed with English **bold** text 日本語");
        assert_eq!(dump(&h), "日本語 mixed with English [STRONG:bold] text 日本語");
    }

    #[test]
    fn triple_marker_unchanged() {
        // markdown-rs nests Emphasis(Strong(...)).
        let h = run("これは***重要***な機能です");
        let p = first_paragraph_children(&h);
        // Find the Emphasis node and assert it wraps a Strong.
        let mut found = false;
        for c in p {
            if let MdastNode::Emphasis(e) = c {
                if let MdastNode::Strong(_) = &e.children[0] {
                    found = true;
                }
            }
        }
        assert!(found, "expected Emphasis(Strong(...)) in {p:?}");
    }

    #[test]
    fn nested_emphasis_unchanged() {
        let h = run("**outer *inner* outer**");
        let p = first_paragraph_children(&h);
        let MdastNode::Strong(s) = &p[0] else {
            unreachable!("expected Strong, got {:?}", p[0])
        };
        // Strong contains: Text " outer ", Emphasis "inner", Text " outer".
        let mut has_em = false;
        for c in &s.children {
            if let MdastNode::Emphasis(_) = c {
                has_em = true;
            }
        }
        assert!(has_em, "expected nested Emphasis inside Strong");
    }

    // --- Cases the visitor must FIX. ---

    #[test]
    fn cjk_punct_inside_strong_close_japanese() {
        // The canonical remark-cjk-friendly bug.
        let h = run("**テスト。**テスト");
        assert_eq!(dump(&h), "[STRONG:テスト。]テスト");
    }

    #[test]
    fn cjk_punct_inside_emphasis_close_japanese() {
        let h = run("*テスト。*テスト");
        assert_eq!(dump(&h), "[EM:テスト。]テスト");
    }

    #[test]
    fn cjk_punct_inside_strong_with_kanji_lead() {
        let h = run("これは**重要。**テスト");
        assert_eq!(dump(&h), "これは[STRONG:重要。]テスト");
    }

    #[test]
    fn ascii_period_inside_close_followed_by_cjk() {
        // ASCII `.` inside, CJK outside. The amendment also covers
        // this: outer is CJK (2bγ), so closing is allowed.
        let h = run("**bold.**テスト");
        assert_eq!(dump(&h), "[STRONG:bold.]テスト");
    }

    // --- No-rewrite zones. ---

    #[test]
    fn inline_code_untouched() {
        let h = run("`これは**重要。**テスト`");
        // The whole thing is one InlineCode child; visitor must not
        // recurse into it.
        let p = first_paragraph_children(&h);
        match &p[0] {
            MdastNode::InlineCode(c) => {
                assert_eq!(c.value, "これは**重要。**テスト");
            }
            other => unreachable!("expected InlineCode, got {other:?}"),
        }
    }

    #[test]
    fn fenced_code_untouched() {
        let mut mdast = markdown::to_mdast(
            "```\nこれは**重要。**テスト\n```\n",
            &markdown::ParseOptions::mdx(),
        )
        .unwrap();
        CjkFriendlyPlugin::new().visit(&mut mdast);
        let MdastNode::Root(r) = &mdast else {
            unreachable!()
        };
        let MdastNode::Code(c) = &r.children[0] else {
            unreachable!("expected Code, got {:?}", r.children[0])
        };
        assert_eq!(c.value, "これは**重要。**テスト");
    }

    #[test]
    fn html_raw_untouched() {
        let mut mdast = markdown::to_mdast(
            "<p>これは**重要。**テスト</p>",
            &markdown::ParseOptions::default(),
        )
        .unwrap();
        CjkFriendlyPlugin::new().visit(&mut mdast);
        let MdastNode::Root(r) = &mdast else {
            unreachable!()
        };
        // Default ParseOptions parses this as raw Html node.
        let mut found_raw = false;
        for c in &r.children {
            if let MdastNode::Html(h) = c {
                if h.value.contains("**重要。**") {
                    found_raw = true;
                }
            }
        }
        assert!(found_raw, "expected raw HTML to contain literal markers");
    }

    #[test]
    fn mdx_jsx_body_untouched() {
        // MDX flow JSX with CJK-punct-inside-marker text. Its body
        // tokens are MdxJsxFlowElement children; visitor must not
        // rewrite Text inside.
        let mut mdast = markdown::to_mdast(
            "<Note>\n\nこれは**重要。**テスト\n\n</Note>\n",
            &markdown::ParseOptions::mdx(),
        )
        .unwrap();
        CjkFriendlyPlugin::new().visit(&mut mdast);
        // Any Text descendant inside <Note> still contains literal `**`.
        let mut found_literal = false;
        fn walk(n: &MdastNode, found: &mut bool) {
            if let MdastNode::MdxJsxFlowElement(j) = n {
                for c in &j.children {
                    walk_inner(c, found);
                }
                return;
            }
            if let Some(children) = match n {
                MdastNode::Root(r) => Some(&r.children),
                _ => None,
            } {
                for c in children {
                    walk(c, found);
                }
            }
        }
        fn walk_inner(n: &MdastNode, found: &mut bool) {
            match n {
                MdastNode::Text(t) => {
                    if t.value.contains("**重要。**") {
                        *found = true;
                    }
                }
                MdastNode::Paragraph(p) => {
                    for c in &p.children {
                        walk_inner(c, found);
                    }
                }
                _ => {}
            }
        }
        walk(&mdast, &mut found_literal);
        assert!(
            found_literal,
            "<Note> body must keep literal `**重要。**`; got {mdast:#?}",
        );
    }

    #[test]
    fn mdx_text_expression_untouched() {
        let mut mdast = markdown::to_mdast(
            "{ \"これは**重要。**テスト\" }",
            &markdown::ParseOptions::mdx(),
        )
        .unwrap();
        CjkFriendlyPlugin::new().visit(&mut mdast);
        // Find the MdxFlowExpression and confirm its raw payload still
        // contains literal `**`.
        let MdastNode::Root(r) = &mdast else {
            unreachable!()
        };
        let mut found = false;
        for c in &r.children {
            if let MdastNode::MdxFlowExpression(e) = c {
                if e.value.contains("**重要。**") {
                    found = true;
                }
            }
        }
        assert!(found, "MdxFlowExpression payload must be untouched");
    }

    #[test]
    fn mdx_jsx_text_element_body_untouched() {
        // Inline (`text`) MDX JSX element — single line, parses as
        // MdxJsxTextElement (not MdxJsxFlowElement). Cover the inline
        // form explicitly so both JSX node types in the no-rewrite list
        // have a dedicated test.
        let mut mdast = markdown::to_mdast(
            "Outside <Inline>これは**重要。**テスト</Inline> after",
            &markdown::ParseOptions::mdx(),
        )
        .unwrap();
        CjkFriendlyPlugin::new().visit(&mut mdast);
        // Walk to find an MdxJsxTextElement and assert its inner Text
        // still contains the literal markers.
        fn find_jsx_text(n: &MdastNode, found: &mut bool) {
            if let MdastNode::MdxJsxTextElement(j) = n {
                for c in &j.children {
                    if let MdastNode::Text(t) = c {
                        if t.value.contains("**重要。**") {
                            *found = true;
                        }
                    }
                }
                return;
            }
            if let Some(children) = match n {
                MdastNode::Root(r) => Some(&r.children),
                MdastNode::Paragraph(p) => Some(&p.children),
                _ => None,
            } {
                for c in children {
                    find_jsx_text(c, found);
                }
            }
        }
        let mut found = false;
        find_jsx_text(&mdast, &mut found);
        assert!(
            found,
            "<Inline> body must keep literal `**重要。**`; got {mdast:#?}",
        );
    }

    #[test]
    fn mdx_inline_text_expression_untouched() {
        // Inline `{...}` JS expression embedded in a paragraph — parses
        // as MdxTextExpression (not MdxFlowExpression). Cover that path
        // explicitly so both expression node types have a test.
        let mut mdast = markdown::to_mdast(
            "Before {\"これは**重要。**テスト\"} after",
            &markdown::ParseOptions::mdx(),
        )
        .unwrap();
        CjkFriendlyPlugin::new().visit(&mut mdast);
        fn find_text_expr(n: &MdastNode, found: &mut bool) {
            if let MdastNode::MdxTextExpression(e) = n {
                if e.value.contains("**重要。**") {
                    *found = true;
                }
                return;
            }
            if let Some(children) = match n {
                MdastNode::Root(r) => Some(&r.children),
                MdastNode::Paragraph(p) => Some(&p.children),
                _ => None,
            } {
                for c in children {
                    find_text_expr(c, found);
                }
            }
        }
        let mut found = false;
        find_text_expr(&mdast, &mut found);
        assert!(found, "MdxTextExpression payload must be untouched");
    }

    // --- Escape protection: `\**` becomes literal `*`, which markdown-rs
    // surfaces as Text "*". The visitor must NOT produce a Strong from
    // the bare leftover stars.

    #[test]
    fn escape_protected_markers_unchanged() {
        // The non-CJK case is already handled correctly by markdown-rs;
        // the visitor must not invent emphasis from the stray `*`.
        let h = run("\\**not bold\\**");
        // markdown-rs parses "\**not bold\**" as Text("*") + Em("not bold*").
        // After our pass, we should not have transformed the stray `*`
        // into a Strong (no closing pair).
        let p = first_paragraph_children(&h);
        let mut strong_count = 0;
        for c in p {
            if let MdastNode::Strong(_) = c {
                strong_count += 1;
            }
        }
        assert_eq!(
            strong_count, 0,
            "must not synthesize Strong from escaped markers; got {p:?}"
        );
    }

    #[test]
    fn cjk_escape_protected_markers_unchanged() {
        // `これは\**重要\**な機能です` — escape kills both opening and
        // closing markers. markdown-rs leaves stray `*` chars in Text,
        // and the visitor must not retokenise them either.
        let h = run("これは\\**重要\\**な機能です");
        let p = first_paragraph_children(&h);
        let mut strong_count = 0;
        for c in p {
            if let MdastNode::Strong(_) = c {
                strong_count += 1;
            }
        }
        assert_eq!(strong_count, 0, "got {p:?}");
    }

    // --- Container coverage. ---

    #[test]
    fn cjk_punct_close_inside_heading() {
        let h = run_root("# **テスト。**テスト\n");
        let MdastNode::Root(r) = &h else { unreachable!() };
        let MdastNode::Heading(head) = &r.children[0] else {
            unreachable!("expected Heading, got {:?}", r.children[0])
        };
        // heading has Strong + Text.
        let mut has_strong = false;
        for c in &head.children {
            if let MdastNode::Strong(_) = c {
                has_strong = true;
            }
        }
        assert!(has_strong, "expected Strong inside heading; got {head:?}");
    }

    #[test]
    fn cjk_punct_close_inside_list_item() {
        let h = run_root("- **テスト。**テスト\n");
        // Walk to the list item paragraph.
        let MdastNode::Root(r) = &h else { unreachable!() };
        let MdastNode::List(l) = &r.children[0] else {
            unreachable!()
        };
        let MdastNode::ListItem(li) = &l.children[0] else {
            unreachable!()
        };
        let MdastNode::Paragraph(p) = &li.children[0] else {
            unreachable!()
        };
        let mut has_strong = false;
        for c in &p.children {
            if let MdastNode::Strong(_) = c {
                has_strong = true;
            }
        }
        assert!(has_strong, "expected Strong in list item para; got {p:?}");
    }

    #[test]
    fn cjk_punct_close_inside_blockquote() {
        let h = run_root("> **テスト。**テスト\n");
        let MdastNode::Root(r) = &h else { unreachable!() };
        let MdastNode::Blockquote(b) = &r.children[0] else {
            unreachable!()
        };
        let MdastNode::Paragraph(p) = &b.children[0] else {
            unreachable!()
        };
        let mut has_strong = false;
        for c in &p.children {
            if let MdastNode::Strong(_) = c {
                has_strong = true;
            }
        }
        assert!(has_strong);
    }

    #[test]
    fn cjk_punct_close_inside_link_label() {
        let h = run_root("[**テスト。**テスト](http://x)");
        let p = first_paragraph_children(&h);
        let MdastNode::Link(l) = &p[0] else {
            unreachable!("expected Link, got {:?}", p[0])
        };
        let mut has_strong = false;
        for c in &l.children {
            if let MdastNode::Strong(_) = c {
                has_strong = true;
            }
        }
        assert!(has_strong, "expected Strong in link label; got {l:?}");
    }

    fn run_root(input: &str) -> MdastNode {
        let mut mdast = markdown::to_mdast(input, &markdown::ParseOptions::mdx())
            .expect("markdown-rs parse");
        CjkFriendlyPlugin::new().visit(&mut mdast);
        mdast
    }
}
