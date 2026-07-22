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
//! visitor rewrites only true phrasing-parent child lists (`Paragraph`,
//! `Heading`, `Strong`, `Emphasis`, `Delete`, `Link`, `LinkReference`, and
//! `TableCell`). It still traverses block containers so each nested phrasing
//! parent is handled independently, but it never matches their direct children.
//! It does NOT enter:
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

use std::ops::Range;

use markdown::{
    mdast::{Emphasis, Link, Node as MdastNode, Strong, Text},
    unist::{Point, Position},
};

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
    if is_phrasing_parent(node) {
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
            | MdastNode::InlineMath(_)
            | MdastNode::Html(_)
            | MdastNode::MdxJsxFlowElement(_)
            | MdastNode::MdxJsxTextElement(_)
            | MdastNode::MdxFlowExpression(_)
            | MdastNode::MdxTextExpression(_)
    )
}

/// True only for parents whose direct children are one phrasing run.
fn is_phrasing_parent(node: &MdastNode) -> bool {
    matches!(
        node,
        MdastNode::Paragraph(_)
            | MdastNode::Heading(_)
            | MdastNode::Strong(_)
            | MdastNode::Emphasis(_)
            | MdastNode::Delete(_)
            | MdastNode::Link(_)
            | MdastNode::LinkReference(_)
            | MdastNode::TableCell(_)
    )
}

#[derive(Clone, Copy, Debug)]
struct Delimiter {
    node: usize,
    start: usize,
    end: usize,
    marker_len: usize,
}

/// Re-tokenise literal asterisk delimiters across contiguous, source-provable
/// phrasing siblings. Every opaque or unprovable child partitions the list.
fn retokenise_children(mut children: Vec<MdastNode>) -> Vec<MdastNode> {
    while let Some((open, close)) = find_sibling_match(&children) {
        children = splice_match(children, open, close);
    }
    children
}

fn find_sibling_match(children: &[MdastNode]) -> Option<(Delimiter, Delimiter)> {
    for (node_index, node) in children.iter().enumerate() {
        let MdastNode::Text(text) = node else {
            continue;
        };
        if !text_source_is_literal(text) {
            continue;
        }
        for (run_start, run_len) in star_runs(&text.value) {
            // Keep markdown-rs's strong-before-emphasis greediness.
            for marker_len in [2, 1] {
                if run_len < marker_len {
                    continue;
                }
                let open = Delimiter {
                    node: node_index,
                    start: run_start,
                    end: run_start + marker_len,
                    marker_len,
                };
                let Some(before) = boundary_before(children, open.node, open.start) else {
                    continue;
                };
                let Some(after) = boundary_after(children, open.node, open.end) else {
                    continue;
                };
                if after == Boundary::Star || !is_left_flanking(before, after, true) {
                    continue;
                }
                let opener_is_amendment = !is_left_flanking(before, after, false);
                if let Some(close) = find_close(children, open, opener_is_amendment) {
                    return Some((open, close));
                }
            }
        }
    }
    None
}

fn find_close(
    children: &[MdastNode],
    open: Delimiter,
    opener_is_amendment: bool,
) -> Option<Delimiter> {
    let mut node_index = open.node;
    while node_index < children.len() {
        if node_index > open.node
            && (!is_movable(&children[node_index])
                || !source_edge_is_contiguous(&children[node_index - 1], &children[node_index]))
        {
            break;
        }
        let MdastNode::Text(text) = &children[node_index] else {
            node_index += 1;
            continue;
        };
        if !text_source_is_literal(text) {
            break;
        }
        for (run_start, run_len) in star_runs(&text.value) {
            if node_index == open.node && run_start < open.end {
                continue;
            }
            if run_len < open.marker_len {
                continue;
            }
            // A close consumes the final marker-width characters of its run.
            let close = Delimiter {
                node: node_index,
                start: run_start + run_len - open.marker_len,
                end: run_start + run_len,
                marker_len: open.marker_len,
            };
            if close.node == open.node && close.start <= open.end {
                continue;
            }
            let Some(before) = boundary_before(children, close.node, close.start) else {
                continue;
            };
            let Some(after) = boundary_after(children, close.node, close.end) else {
                continue;
            };
            if before == Boundary::Star || !is_right_flanking(before, after, true) {
                continue;
            }
            let closer_is_amendment = !is_right_flanking(before, after, false);
            if opener_is_amendment || closer_is_amendment {
                return Some(close);
            }
        }
        node_index += 1;
    }
    None
}

fn splice_match(children: Vec<MdastNode>, open: Delimiter, close: Delimiter) -> Vec<MdastNode> {
    let wrapper_position = delimiter_span(&children, open, close);
    let mut before = Vec::with_capacity(children.len());
    let mut inner = Vec::new();
    let mut after = Vec::new();

    for (index, node) in children.into_iter().enumerate() {
        if index < open.node {
            before.push(node);
        } else if index > close.node {
            after.push(node);
        } else if open.node == close.node {
            let MdastNode::Text(text) = node else {
                unreachable!()
            };
            push_text_slice(&mut before, &text, 0..open.start);
            push_text_slice(&mut inner, &text, open.end..close.start);
            push_text_slice(&mut after, &text, close.end..text.value.len());
        } else if index == open.node {
            let MdastNode::Text(text) = node else {
                unreachable!()
            };
            push_text_slice(&mut before, &text, 0..open.start);
            push_text_slice(&mut inner, &text, open.end..text.value.len());
        } else if index == close.node {
            let MdastNode::Text(text) = node else {
                unreachable!()
            };
            push_text_slice(&mut inner, &text, 0..close.start);
            push_text_slice(&mut after, &text, close.end..text.value.len());
        } else {
            // Ownership is transferred: opaque nodes and all their metadata are
            // neither flattened nor rebuilt.
            inner.push(node);
        }
    }

    before.push(wrap_sibling(open.marker_len, inner, wrapper_position));
    before.extend(after);
    before
}

fn delimiter_span(children: &[MdastNode], open: Delimiter, close: Delimiter) -> Option<Position> {
    let MdastNode::Text(open_text) = &children[open.node] else {
        return None;
    };
    let MdastNode::Text(close_text) = &children[close.node] else {
        return None;
    };
    let open_position = open_text.position.as_ref()?;
    let close_position = close_text.position.as_ref()?;
    Some(Position {
        start: point_after(&open_position.start, &open_text.value[..open.start]),
        end: point_after(&close_position.start, &close_text.value[..close.end]),
    })
}

fn push_text_slice(out: &mut Vec<MdastNode>, text: &Text, range: Range<usize>) {
    if range.is_empty() {
        return;
    }
    let value = text.value[range.clone()].to_owned();
    let position = text.position.as_ref().map(|position| Position {
        start: point_after(&position.start, &text.value[..range.start]),
        end: point_after(&position.start, &text.value[..range.end]),
    });
    out.push(MdastNode::Text(Text { value, position }));
}

fn point_after(start: &Point, prefix: &str) -> Point {
    let mut line = start.line;
    let mut column = start.column;
    for character in prefix.chars() {
        if character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    Point::new(line, column, start.offset + prefix.len())
}

fn star_runs(value: &str) -> Vec<(usize, usize)> {
    let bytes = value.as_bytes();
    let mut runs = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'*' {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && bytes[index] == b'*' {
            index += 1;
        }
        runs.push((start, index - start));
    }
    runs
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Boundary {
    Whitespace,
    Cjk,
    CjkPunctuation,
    NonCjkPunctuation,
    Star,
    Other,
}

impl Boundary {
    fn from_char(character: char) -> Self {
        if character == '*' {
            Self::Star
        } else if character.is_whitespace() {
            Self::Whitespace
        } else if is_cjk(character) {
            if is_unicode_punctuation(character) {
                Self::CjkPunctuation
            } else {
                Self::Cjk
            }
        } else if is_unicode_punctuation(character) {
            Self::NonCjkPunctuation
        } else {
            Self::Other
        }
    }

    fn is_any_punctuation(self) -> bool {
        matches!(self, Self::CjkPunctuation | Self::NonCjkPunctuation)
    }

    fn is_cjk(self) -> bool {
        matches!(self, Self::Cjk | Self::CjkPunctuation)
    }
}

fn is_left_flanking(before: Boundary, after: Boundary, amended: bool) -> bool {
    if after == Boundary::Whitespace {
        return false;
    }
    if amended {
        after != Boundary::NonCjkPunctuation
            || before == Boundary::Whitespace
            || before == Boundary::NonCjkPunctuation
            || before.is_cjk()
    } else {
        !after.is_any_punctuation() || before == Boundary::Whitespace || before.is_any_punctuation()
    }
}

fn is_right_flanking(before: Boundary, after: Boundary, amended: bool) -> bool {
    if before == Boundary::Whitespace {
        return false;
    }
    if amended {
        before != Boundary::NonCjkPunctuation
            || after == Boundary::Whitespace
            || after == Boundary::NonCjkPunctuation
            || after.is_cjk()
    } else {
        !before.is_any_punctuation() || after == Boundary::Whitespace || after.is_any_punctuation()
    }
}

fn boundary_before(children: &[MdastNode], node: usize, byte: usize) -> Option<Boundary> {
    let MdastNode::Text(text) = &children[node] else {
        return None;
    };
    if byte > 0 {
        return text.value[..byte]
            .chars()
            .next_back()
            .map(Boundary::from_char);
    }
    if node == 0 {
        return Some(Boundary::Whitespace);
    }
    if !source_edge_is_contiguous(&children[node - 1], &children[node]) {
        return None;
    }
    source_boundaries(&children[node - 1]).map(|(_, right)| right)
}

fn boundary_after(children: &[MdastNode], node: usize, byte: usize) -> Option<Boundary> {
    let MdastNode::Text(text) = &children[node] else {
        return None;
    };
    if byte < text.value.len() {
        return text.value[byte..].chars().next().map(Boundary::from_char);
    }
    if node + 1 == children.len() {
        return Some(Boundary::Whitespace);
    }
    if !source_edge_is_contiguous(&children[node], &children[node + 1]) {
        return None;
    }
    source_boundaries(&children[node + 1]).map(|(left, _)| left)
}

fn text_source_is_literal(text: &Text) -> bool {
    text.position.as_ref().is_none_or(|position| {
        position.end.offset.saturating_sub(position.start.offset) == text.value.len()
    })
}

fn source_edge_is_contiguous(left: &MdastNode, right: &MdastNode) -> bool {
    match (left.position(), right.position()) {
        (Some(left), Some(right)) => left.end.offset == right.start.offset,
        // Positionless text still supports same-node behavior. It cannot prove
        // a source boundary across two siblings.
        _ => false,
    }
}

fn is_movable(node: &MdastNode) -> bool {
    matches!(
        node,
        MdastNode::Text(_)
            | MdastNode::Link(_)
            | MdastNode::LinkReference(_)
            | MdastNode::Image(_)
            | MdastNode::ImageReference(_)
            | MdastNode::Emphasis(_)
            | MdastNode::Strong(_)
            | MdastNode::Delete(_)
    ) && source_boundaries(node).is_some()
}

fn source_boundaries(node: &MdastNode) -> Option<(Boundary, Boundary)> {
    let punctuation = (Boundary::NonCjkPunctuation, Boundary::NonCjkPunctuation);
    match node {
        MdastNode::Text(text) if text_source_is_literal(text) => Some((
            Boundary::from_char(text.value.chars().next()?),
            Boundary::from_char(text.value.chars().next_back()?),
        )),
        MdastNode::Link(link) => link_source_boundaries(link),
        MdastNode::LinkReference(link) => {
            let position = link.position.as_ref()?;
            let first = link.children.first()?.position()?;
            let last = link.children.last()?.position()?;
            (first.start.offset == position.start.offset + 1
                && last.end.offset < position.end.offset)
                .then_some(punctuation)
        }
        MdastNode::Image(image) if image.position.is_some() => Some(punctuation),
        MdastNode::ImageReference(image) if image.position.is_some() => Some(punctuation),
        MdastNode::Emphasis(emphasis) => {
            delimited_parent_boundaries(emphasis.position.as_ref(), &emphasis.children)
        }
        MdastNode::Strong(strong) => {
            delimited_parent_boundaries(strong.position.as_ref(), &strong.children)
        }
        MdastNode::Delete(delete) => {
            delimited_parent_boundaries(delete.position.as_ref(), &delete.children)
        }
        _ => None,
    }
}

fn delimited_parent_boundaries(
    position: Option<&Position>,
    children: &[MdastNode],
) -> Option<(Boundary, Boundary)> {
    let position = position?;
    let first = children.first()?.position()?;
    let last = children.last()?.position()?;
    (first.start.offset > position.start.offset && last.end.offset < position.end.offset)
        .then_some((Boundary::NonCjkPunctuation, Boundary::NonCjkPunctuation))
}

fn link_source_boundaries(link: &Link) -> Option<(Boundary, Boundary)> {
    let position = link.position.as_ref()?;
    let punctuation = (Boundary::NonCjkPunctuation, Boundary::NonCjkPunctuation);
    if link.children.len() != 1 {
        let first = link.children.first()?.position()?;
        let last = link.children.last()?.position()?;
        return (first.start.offset == position.start.offset + 1
            && last.end.offset < position.end.offset)
            .then_some(punctuation);
    }
    let MdastNode::Text(label) = &link.children[0] else {
        let child = link.children[0].position()?;
        return (child.start.offset == position.start.offset + 1
            && child.end.offset < position.end.offset)
            .then_some(punctuation);
    };
    let label_position = label.position.as_ref()?;

    // Bracket resource link: `[label](destination)`.
    if label_position.start.offset == position.start.offset + 1
        && label_position.end.offset + 1 < position.end.offset
    {
        return Some(punctuation);
    }
    // Angle autolink: `<destination>`.
    if label_position.start.offset == position.start.offset + 1
        && label_position.end.offset + 1 == position.end.offset
        && (link.url == label.value || link.url == format!("mailto:{}", label.value))
    {
        return Some(punctuation);
    }
    // GFM bare URL/email. The source boundaries are visible characters.
    if label_position == position
        && text_source_is_literal(label)
        && (link.url == label.value
            || link.url == format!("mailto:{}", label.value)
            || link.url == format!("http://{}", label.value))
    {
        return Some((
            Boundary::from_char(label.value.chars().next()?),
            Boundary::from_char(label.value.chars().next_back()?),
        ));
    }
    None
}

fn wrap_sibling(
    marker_len: usize,
    children: Vec<MdastNode>,
    position: Option<Position>,
) -> MdastNode {
    if marker_len == 2 {
        MdastNode::Strong(Strong { children, position })
    } else {
        MdastNode::Emphasis(Emphasis { children, position })
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
        assert_eq!(
            dump(&h),
            "日本語 mixed with English [STRONG:bold] text 日本語"
        );
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
    fn exact_bracket_link_fixture_moves_original_node_and_metadata() {
        let input = "知られる**[LA-2A](https://example.com)光学式コンプレッサー**と、**Neve 1073 EQ**という";
        let parsed = markdown::to_mdast(input, &markdown::ParseOptions::mdx()).unwrap();
        let original = first_paragraph_children(&parsed)[1].clone();
        let original_position = original.position().cloned();

        let transformed = run(input);
        let children = first_paragraph_children(&transformed);
        assert_eq!(children.len(), 5, "{children:#?}");
        assert!(matches!(&children[0], MdastNode::Text(t) if t.value == "知られる"));
        let MdastNode::Strong(repaired) = &children[1] else {
            unreachable!("expected repaired Strong, got {:?}", children[1])
        };
        assert_eq!(repaired.children.len(), 2);
        assert_eq!(repaired.children[0], original);
        assert_eq!(repaired.children[0].position(), original_position.as_ref());
        assert!(
            matches!(&repaired.children[1], MdastNode::Text(t) if t.value == "光学式コンプレッサー")
        );
        assert_eq!(
            repaired
                .position
                .as_ref()
                .map(|p| (p.start.offset, p.end.offset)),
            Some((12, 74))
        );
        assert!(matches!(&children[2], MdastNode::Text(t) if t.value == "と、"));
        assert!(
            matches!(&children[3], MdastNode::Strong(s) if matches!(&s.children[0], MdastNode::Text(t) if t.value == "Neve 1073 EQ"))
        );
        assert!(matches!(&children[4], MdastNode::Text(t) if t.value == "という"));
    }

    #[test]
    fn opener_closer_and_both_can_be_separated_by_links() {
        let opener = run("漢**[x](https://x.example)語** end");
        let children = first_paragraph_children(&opener);
        assert!(
            matches!(&children[1], MdastNode::Strong(s) if matches!(&s.children[0], MdastNode::Link(_)))
        );

        let closer = run("**語[x](https://x.example).**漢");
        let children = first_paragraph_children(&closer);
        assert!(
            matches!(&children[0], MdastNode::Strong(s) if s.children.iter().any(|n| matches!(n, MdastNode::Link(_))))
        );

        let both = run("漢*[x](https://x.example)* end");
        let children = first_paragraph_children(&both);
        assert!(
            matches!(&children[1], MdastNode::Emphasis(e) if matches!(&e.children[0], MdastNode::Link(_)))
        );
    }

    #[test]
    fn multiple_candidates_and_prefix_suffix_are_preserved() {
        let h = run("pre 漢**[a](/a)語** mid 漢*[b](/b)語* post");
        let children = first_paragraph_children(&h);
        assert!(matches!(&children[0], MdastNode::Text(t) if t.value == "pre 漢"));
        assert!(children.iter().any(|n| matches!(n, MdastNode::Strong(_))));
        assert!(children.iter().any(|n| matches!(n, MdastNode::Emphasis(_))));
        assert!(matches!(children.last(), Some(MdastNode::Text(t)) if t.value == " post"));
    }

    #[test]
    fn multiple_movable_sibling_kinds_remain_nested_and_ordered() {
        let input = "漢**[a](/a)![b](/b)[c][id]*d*~~e~~語** end\n\n[id]: /c\n";
        let mut mdast = markdown::to_mdast(input, &markdown::ParseOptions::gfm()).unwrap();
        CjkFriendlyPlugin::new().visit(&mut mdast);
        let children = first_paragraph_children(&mdast);
        let MdastNode::Strong(repaired) = &children[1] else {
            unreachable!("expected Strong, got {children:#?}")
        };
        assert!(matches!(&repaired.children[0], MdastNode::Link(_)));
        assert!(matches!(&repaired.children[1], MdastNode::Image(_)));
        assert!(matches!(&repaired.children[2], MdastNode::LinkReference(_)));
        assert!(matches!(&repaired.children[3], MdastNode::Emphasis(_)));
        assert!(matches!(&repaired.children[4], MdastNode::Delete(_)));
        assert!(matches!(&repaired.children[5], MdastNode::Text(t) if t.value == "語"));
    }

    #[test]
    fn inline_barriers_partition_sibling_matching() {
        fn first_child(input: &str, options: markdown::ParseOptions) -> MdastNode {
            let parsed = markdown::to_mdast(input, &options).unwrap();
            let MdastNode::Root(root) = parsed else {
                unreachable!()
            };
            match &root.children[0] {
                MdastNode::Paragraph(paragraph) => paragraph.children[0].clone(),
                node => node.clone(),
            }
        }
        let break_node = {
            let parsed = markdown::to_mdast("a\\\nb", &markdown::ParseOptions::mdx()).unwrap();
            first_paragraph_children(&parsed)[1].clone()
        };
        let barriers = [
            first_child("`code`", markdown::ParseOptions::mdx()),
            first_child("<i>", markdown::ParseOptions::default()),
            first_child("{value}", markdown::ParseOptions::mdx()),
            first_child("<X />", markdown::ParseOptions::mdx()),
            break_node,
            first_child("[^note]\n\n[^note]: note", markdown::ParseOptions::gfm()),
        ];
        for mut barrier in barriers {
            barrier.position_set(Some(Position::new(1, 4, 5, 1, 5, 6)));
            let original = vec![
                MdastNode::Text(Text {
                    value: "漢**".into(),
                    position: Some(Position::new(1, 1, 0, 1, 4, 5)),
                }),
                barrier,
                MdastNode::Text(Text {
                    value: "語.**漢".into(),
                    position: Some(Position::new(1, 5, 6, 1, 11, 15)),
                }),
            ];
            assert_eq!(retokenise_children(original.clone()), original);
        }
    }

    #[test]
    fn does_not_pair_across_paragraphs_list_items_rows_or_cells() {
        for input in [
            "漢**\n\n語** end",
            "- 漢**\n- 語** end",
            "| 漢** | 語** end |\n| --- | --- |",
            "| 漢** |\n| --- |\n| 語** end |",
        ] {
            let mut mdast = markdown::to_mdast(input, &markdown::ParseOptions::gfm()).unwrap();
            CjkFriendlyPlugin::new().visit(&mut mdast);
            fn count_strong(node: &MdastNode) -> usize {
                usize::from(matches!(node, MdastNode::Strong(_)))
                    + node
                        .children()
                        .map(|children| children.iter().map(count_strong).sum())
                        .unwrap_or(0)
            }
            assert_eq!(
                count_strong(&mdast),
                0,
                "crossed block boundary: {mdast:#?}"
            );
        }
    }

    #[test]
    fn link_source_forms_have_distinct_proven_boundaries() {
        let parsed = markdown::to_mdast(
            "[label](/x) <https://example.com> https://example.com",
            &markdown::ParseOptions::gfm(),
        )
        .unwrap();
        let links: Vec<_> = first_paragraph_children(&parsed)
            .iter()
            .filter_map(|node| match node {
                MdastNode::Link(link) => Some(link),
                _ => None,
            })
            .collect();
        assert_eq!(links.len(), 3);
        assert_eq!(
            link_source_boundaries(links[0]),
            Some((Boundary::NonCjkPunctuation, Boundary::NonCjkPunctuation))
        );
        assert_eq!(
            link_source_boundaries(links[1]),
            Some((Boundary::NonCjkPunctuation, Boundary::NonCjkPunctuation))
        );
        assert_eq!(
            link_source_boundaries(links[2]),
            Some((Boundary::Other, Boundary::Other))
        );

        let mut unprovable = links[0].clone();
        unprovable.position = None;
        assert_eq!(link_source_boundaries(&unprovable), None);
    }

    #[test]
    fn source_punctuation_not_visible_link_text_drives_opener() {
        let bracket = run("漢**[ascii](/x)語** end");
        assert!(matches!(
            &first_paragraph_children(&bracket)[1],
            MdastNode::Strong(_)
        ));
    }

    #[test]
    fn escaped_unmatched_and_unprovable_text_remain_literal() {
        for input in ["漢\\**[x](/x)語** end", "漢**[x](/x)語 end"] {
            let h = run(input);
            assert!(!first_paragraph_children(&h)
                .iter()
                .any(|node| matches!(node, MdastNode::Strong(_))));
        }

        let mut children = vec![MdastNode::Text(Text {
            value: "漢**[x]語** end".into(),
            position: Some(Position::new(1, 1, 0, 1, 20, 99)),
        })];
        let original = children.clone();
        children = retokenise_children(children);
        assert_eq!(children, original);
    }

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
    fn amended_opener_in_one_text_node_is_preserved() {
        let h = run("これは**「重要」** end");
        assert_eq!(dump(&h), "これは[STRONG:「重要」] end");
    }

    #[test]
    fn positionless_same_text_is_supported_but_ordinary_markers_are_not_reparsed() {
        let amended = retokenise_children(vec![MdastNode::Text(Text {
            value: "**bold.**漢".into(),
            position: None,
        })]);
        assert!(matches!(&amended[0], MdastNode::Strong(_)));

        let ordinary = vec![MdastNode::Text(Text {
            value: " **bold** ".into(),
            position: None,
        })];
        assert_eq!(retokenise_children(ordinary.clone()), ordinary);
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
                MdastNode::Text(t) if t.value.contains("**重要。**") => {
                    *found = true;
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
        let MdastNode::Root(r) = &h else {
            unreachable!()
        };
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
        let MdastNode::Root(r) = &h else {
            unreachable!()
        };
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
        let MdastNode::Root(r) = &h else {
            unreachable!()
        };
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
        let mut mdast =
            markdown::to_mdast(input, &markdown::ParseOptions::mdx()).expect("markdown-rs parse");
        CjkFriendlyPlugin::new().visit(&mut mdast);
        mdast
    }
}
