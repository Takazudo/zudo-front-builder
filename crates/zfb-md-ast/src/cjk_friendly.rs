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
//! Place this plugin in `Pipeline::with_defaults` (`zfb-content`) BEFORE
//! any visitor that depends on emphasis being already tokenised (e.g.
//! visitors that synthesize anchor labels from heading text and might
//! miss children we have just inserted).
//!
//! [upstream]: https://www.npmjs.com/package/remark-cjk-friendly
//! [markdown-cjk-friendly]: https://github.com/tats-u/markdown-cjk-friendly
//! [spec]: https://github.com/tats-u/markdown-cjk-friendly/blob/main/specification.md

use std::ops::Range;

use markdown::{
    mdast::{Emphasis, Link, Node as MdastNode, Strong, Text},
    unist::{Point, Position},
};

use crate::MdastVisitor;

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
                let before = boundary_before(children, open.node, open.start);
                let after = boundary_after(children, open.node, open.end);
                if after == Some(Boundary::Star)
                    || prove_left_flanking(before, after, true) != Some(true)
                {
                    continue;
                }
                let opener_is_amendment = prove_left_flanking(before, after, false) == Some(false);
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
            let before = boundary_before(children, close.node, close.start);
            let after = boundary_after(children, close.node, close.end);
            if before == Some(Boundary::Star)
                || prove_right_flanking(before, after, true) != Some(true)
            {
                continue;
            }
            let closer_is_amendment = prove_right_flanking(before, after, false) == Some(false);
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
    let bytes = prefix.as_bytes();
    for (index, byte) in bytes.iter().copied().enumerate() {
        match byte {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => {}
            b'\n' | b'\r' => {
                line += 1;
                column = 1;
            }
            b'\t' => {
                let remainder = column % 4;
                column += 1 + if remainder == 0 { 0 } else { 4 - remainder };
            }
            _ => column += 1,
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
    IdeographicVariationSelector,
    Other,
}

impl Boundary {
    fn from_char(character: char) -> Self {
        if character == '*' {
            Self::Star
        } else if is_ideographic_variation_selector(character) {
            Self::IdeographicVariationSelector
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

    fn is_amended_left_context(self) -> bool {
        self == Self::Whitespace
            || self == Self::NonCjkPunctuation
            || self.is_cjk()
            || self == Self::IdeographicVariationSelector
    }
}

fn boundary_from_start(value: &str) -> Option<Boundary> {
    value.chars().next().map(Boundary::from_char)
}

fn boundary_from_end(value: &str) -> Option<Boundary> {
    let mut characters = value.chars().rev();
    let last = characters.next()?;
    if is_ideographic_variation_selector(last) {
        return Some(Boundary::IdeographicVariationSelector);
    }
    if is_non_emoji_general_use_variation_selector(last) {
        return Some(match characters.next() {
            Some(base) if is_cjk(base) || is_unicode_punctuation(base) => Boundary::from_char(base),
            _ => Boundary::Other,
        });
    }
    Some(Boundary::from_char(last))
}

fn is_non_emoji_general_use_variation_selector(character: char) -> bool {
    matches!(character as u32, 0xFE00..=0xFE0E)
}

fn is_ideographic_variation_selector(character: char) -> bool {
    matches!(character as u32, 0xE0100..=0xE01EF)
}

fn is_left_flanking(before: Boundary, after: Boundary, amended: bool) -> bool {
    if after == Boundary::Whitespace {
        return false;
    }
    if amended {
        after != Boundary::NonCjkPunctuation || before.is_amended_left_context()
    } else {
        !after.is_any_punctuation() || before == Boundary::Whitespace || before.is_any_punctuation()
    }
}

const ALL_BOUNDARIES: [Boundary; 7] = [
    Boundary::Whitespace,
    Boundary::Cjk,
    Boundary::CjkPunctuation,
    Boundary::NonCjkPunctuation,
    Boundary::Star,
    Boundary::IdeographicVariationSelector,
    Boundary::Other,
];

/// Return a flanking result only when every possible value of an unknown
/// boundary yields the same answer. Unknown is never guessed as whitespace.
fn prove_flanking(
    before: Option<Boundary>,
    after: Option<Boundary>,
    predicate: impl Fn(Boundary, Boundary) -> bool,
) -> Option<bool> {
    let mut result = None;
    for candidate_before in ALL_BOUNDARIES
        .iter()
        .copied()
        .filter(|candidate| before.is_none_or(|known| known == *candidate))
    {
        for candidate_after in ALL_BOUNDARIES
            .iter()
            .copied()
            .filter(|candidate| after.is_none_or(|known| known == *candidate))
        {
            let candidate = predicate(candidate_before, candidate_after);
            if result.is_some_and(|known| known != candidate) {
                return None;
            }
            result = Some(candidate);
        }
    }
    result
}

fn prove_left_flanking(
    before: Option<Boundary>,
    after: Option<Boundary>,
    amended: bool,
) -> Option<bool> {
    prove_flanking(before, after, |before, after| {
        is_left_flanking(before, after, amended)
    })
}

fn prove_right_flanking(
    before: Option<Boundary>,
    after: Option<Boundary>,
    amended: bool,
) -> Option<bool> {
    prove_flanking(before, after, |before, after| {
        is_right_flanking(before, after, amended)
    })
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
        return boundary_from_end(&text.value[..byte]);
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
        return boundary_from_start(&text.value[byte..]);
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
            boundary_from_start(&text.value)?,
            boundary_from_end(&text.value)?,
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
            boundary_from_start(&label.value)?,
            boundary_from_end(&label.value)?,
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

// `is_cjk` lives in the sibling `cjk` module so the GFM autolink boundary
// pass (`crate::cjk_autolink`) can reach it too. Re-exported here to keep
// this module's original public path (`zfb_content::plugins::cjk_friendly::is_cjk`
// pre-#2398, now `zfb_md_ast::cjk_friendly::is_cjk`).
pub use crate::cjk::is_cjk;

/// CommonMark Unicode punctuation, including symbol categories, generated from
/// the punctuation table shipped by the pinned markdown-rs parser. Keeping the
/// classifications identical is important when deciding whether a delimiter is
/// amendment-specific.
fn is_unicode_punctuation(c: char) -> bool {
    matches!(
        c as u32,
        0x21..=0x2F
            | 0x3A..=0x40
            | 0x5B..=0x60
            | 0x7B..=0x7E
            | 0xA1..=0xA9
            | 0xAB..=0xAC
            | 0xAE..=0xB1
            | 0xB4
            | 0xB6..=0xB8
            | 0xBB
            | 0xBF
            | 0xD7
            | 0xF7
            | 0x2C2..=0x2C5
            | 0x2D2..=0x2DF
            | 0x2E5..=0x2EB
            | 0x2ED
            | 0x2EF..=0x2FF
            | 0x375
            | 0x37E
            | 0x384..=0x385
            | 0x387
            | 0x3F6
            | 0x482
            | 0x55A..=0x55F
            | 0x589..=0x58A
            | 0x58D..=0x58F
            | 0x5BE
            | 0x5C0
            | 0x5C3
            | 0x5C6
            | 0x5F3..=0x5F4
            | 0x606..=0x60F
            | 0x61B
            | 0x61D..=0x61F
            | 0x66A..=0x66D
            | 0x6D4
            | 0x6DE
            | 0x6E9
            | 0x6FD..=0x6FE
            | 0x700..=0x70D
            | 0x7F6..=0x7F9
            | 0x7FE..=0x7FF
            | 0x830..=0x83E
            | 0x85E
            | 0x888
            | 0x964..=0x965
            | 0x970
            | 0x9F2..=0x9F3
            | 0x9FA..=0x9FB
            | 0x9FD
            | 0xA76
            | 0xAF0..=0xAF1
            | 0xB70
            | 0xBF3..=0xBFA
            | 0xC77
            | 0xC7F
            | 0xC84
            | 0xD4F
            | 0xD79
            | 0xDF4
            | 0xE3F
            | 0xE4F
            | 0xE5A..=0xE5B
            | 0xF01..=0xF17
            | 0xF1A..=0xF1F
            | 0xF34
            | 0xF36
            | 0xF38
            | 0xF3A..=0xF3D
            | 0xF85
            | 0xFBE..=0xFC5
            | 0xFC7..=0xFCC
            | 0xFCE..=0xFDA
            | 0x104A..=0x104F
            | 0x109E..=0x109F
            | 0x10FB
            | 0x1360..=0x1368
            | 0x1390..=0x1399
            | 0x1400
            | 0x166D..=0x166E
            | 0x169B..=0x169C
            | 0x16EB..=0x16ED
            | 0x1735..=0x1736
            | 0x17D4..=0x17D6
            | 0x17D8..=0x17DB
            | 0x1800..=0x180A
            | 0x1940
            | 0x1944..=0x1945
            | 0x19DE..=0x19FF
            | 0x1A1E..=0x1A1F
            | 0x1AA0..=0x1AA6
            | 0x1AA8..=0x1AAD
            | 0x1B4E..=0x1B4F
            | 0x1B5A..=0x1B6A
            | 0x1B74..=0x1B7F
            | 0x1BFC..=0x1BFF
            | 0x1C3B..=0x1C3F
            | 0x1C7E..=0x1C7F
            | 0x1CC0..=0x1CC7
            | 0x1CD3
            | 0x1FBD
            | 0x1FBF..=0x1FC1
            | 0x1FCD..=0x1FCF
            | 0x1FDD..=0x1FDF
            | 0x1FED..=0x1FEF
            | 0x1FFD..=0x1FFE
            | 0x2010..=0x2027
            | 0x2030..=0x205E
            | 0x207A..=0x207E
            | 0x208A..=0x208E
            | 0x20A0..=0x20C0
            | 0x2100..=0x2101
            | 0x2103..=0x2106
            | 0x2108..=0x2109
            | 0x2114
            | 0x2116..=0x2118
            | 0x211E..=0x2123
            | 0x2125
            | 0x2127
            | 0x2129
            | 0x212E
            | 0x213A..=0x213B
            | 0x2140..=0x2144
            | 0x214A..=0x214D
            | 0x214F
            | 0x218A..=0x218B
            | 0x2190..=0x2429
            | 0x2440..=0x244A
            | 0x249C..=0x24E9
            | 0x2500..=0x2775
            | 0x2794..=0x2B73
            | 0x2B76..=0x2B95
            | 0x2B97..=0x2BFF
            | 0x2CE5..=0x2CEA
            | 0x2CF9..=0x2CFC
            | 0x2CFE..=0x2CFF
            | 0x2D70
            | 0x2E00..=0x2E2E
            | 0x2E30..=0x2E5D
            | 0x2E80..=0x2E99
            | 0x2E9B..=0x2EF3
            | 0x2F00..=0x2FD5
            | 0x2FF0..=0x2FFF
            | 0x3001..=0x3004
            | 0x3008..=0x3020
            | 0x3030
            | 0x3036..=0x3037
            | 0x303D..=0x303F
            | 0x309B..=0x309C
            | 0x30A0
            | 0x30FB
            | 0x3190..=0x3191
            | 0x3196..=0x319F
            | 0x31C0..=0x31E5
            | 0x31EF
            | 0x3200..=0x321E
            | 0x322A..=0x3247
            | 0x3250
            | 0x3260..=0x327F
            | 0x328A..=0x32B0
            | 0x32C0..=0x33FF
            | 0x4DC0..=0x4DFF
            | 0xA490..=0xA4C6
            | 0xA4FE..=0xA4FF
            | 0xA60D..=0xA60F
            | 0xA673
            | 0xA67E
            | 0xA6F2..=0xA6F7
            | 0xA700..=0xA716
            | 0xA720..=0xA721
            | 0xA789..=0xA78A
            | 0xA828..=0xA82B
            | 0xA836..=0xA839
            | 0xA874..=0xA877
            | 0xA8CE..=0xA8CF
            | 0xA8F8..=0xA8FA
            | 0xA8FC
            | 0xA92E..=0xA92F
            | 0xA95F
            | 0xA9C1..=0xA9CD
            | 0xA9DE..=0xA9DF
            | 0xAA5C..=0xAA5F
            | 0xAA77..=0xAA79
            | 0xAADE..=0xAADF
            | 0xAAF0..=0xAAF1
            | 0xAB5B
            | 0xAB6A..=0xAB6B
            | 0xABEB
            | 0xFB29
            | 0xFBB2..=0xFBC2
            | 0xFD3E..=0xFD4F
            | 0xFDCF
            | 0xFDFC..=0xFDFF
            | 0xFE10..=0xFE19
            | 0xFE30..=0xFE52
            | 0xFE54..=0xFE66
            | 0xFE68..=0xFE6B
            | 0xFF01..=0xFF0F
            | 0xFF1A..=0xFF20
            | 0xFF3B..=0xFF40
            | 0xFF5B..=0xFF65
            | 0xFFE0..=0xFFE6
            | 0xFFE8..=0xFFEE
            | 0xFFFC..=0xFFFD
            | 0x10100..=0x10102
            | 0x10137..=0x1013F
            | 0x10179..=0x10189
            | 0x1018C..=0x1018E
            | 0x10190..=0x1019C
            | 0x101A0
            | 0x101D0..=0x101FC
            | 0x1039F
            | 0x103D0
            | 0x1056F
            | 0x10857
            | 0x10877..=0x10878
            | 0x1091F
            | 0x1093F
            | 0x10A50..=0x10A58
            | 0x10A7F
            | 0x10AC8
            | 0x10AF0..=0x10AF6
            | 0x10B39..=0x10B3F
            | 0x10B99..=0x10B9C
            | 0x10D6E
            | 0x10D8E..=0x10D8F
            | 0x10EAD
            | 0x10F55..=0x10F59
            | 0x10F86..=0x10F89
            | 0x11047..=0x1104D
            | 0x110BB..=0x110BC
            | 0x110BE..=0x110C1
            | 0x11140..=0x11143
            | 0x11174..=0x11175
            | 0x111C5..=0x111C8
            | 0x111CD
            | 0x111DB
            | 0x111DD..=0x111DF
            | 0x11238..=0x1123D
            | 0x112A9
            | 0x113D4..=0x113D5
            | 0x113D7..=0x113D8
            | 0x1144B..=0x1144F
            | 0x1145A..=0x1145B
            | 0x1145D
            | 0x114C6
            | 0x115C1..=0x115D7
            | 0x11641..=0x11643
            | 0x11660..=0x1166C
            | 0x116B9
            | 0x1173C..=0x1173F
            | 0x1183B
            | 0x11944..=0x11946
            | 0x119E2
            | 0x11A3F..=0x11A46
            | 0x11A9A..=0x11A9C
            | 0x11A9E..=0x11AA2
            | 0x11B00..=0x11B09
            | 0x11BE1
            | 0x11C41..=0x11C45
            | 0x11C70..=0x11C71
            | 0x11EF7..=0x11EF8
            | 0x11F43..=0x11F4F
            | 0x11FD5..=0x11FF1
            | 0x11FFF
            | 0x12470..=0x12474
            | 0x12FF1..=0x12FF2
            | 0x16A6E..=0x16A6F
            | 0x16AF5
            | 0x16B37..=0x16B3F
            | 0x16B44..=0x16B45
            | 0x16D6D..=0x16D6F
            | 0x16E97..=0x16E9A
            | 0x16FE2
            | 0x1BC9C
            | 0x1BC9F
            | 0x1CC00..=0x1CCEF
            | 0x1CD00..=0x1CEB3
            | 0x1CF50..=0x1CFC3
            | 0x1D000..=0x1D0F5
            | 0x1D100..=0x1D126
            | 0x1D129..=0x1D164
            | 0x1D16A..=0x1D16C
            | 0x1D183..=0x1D184
            | 0x1D18C..=0x1D1A9
            | 0x1D1AE..=0x1D1EA
            | 0x1D200..=0x1D241
            | 0x1D245
            | 0x1D300..=0x1D356
            | 0x1D6C1
            | 0x1D6DB
            | 0x1D6FB
            | 0x1D715
            | 0x1D735
            | 0x1D74F
            | 0x1D76F
            | 0x1D789
            | 0x1D7A9
            | 0x1D7C3
            | 0x1D800..=0x1D9FF
            | 0x1DA37..=0x1DA3A
            | 0x1DA6D..=0x1DA74
            | 0x1DA76..=0x1DA83
            | 0x1DA85..=0x1DA8B
            | 0x1E14F
            | 0x1E2FF
            | 0x1E5FF
            | 0x1E95E..=0x1E95F
            | 0x1ECAC
            | 0x1ECB0
            | 0x1ED2E
            | 0x1EEF0..=0x1EEF1
            | 0x1F000..=0x1F02B
            | 0x1F030..=0x1F093
            | 0x1F0A0..=0x1F0AE
            | 0x1F0B1..=0x1F0BF
            | 0x1F0C1..=0x1F0CF
            | 0x1F0D1..=0x1F0F5
            | 0x1F10D..=0x1F1AD
            | 0x1F1E6..=0x1F202
            | 0x1F210..=0x1F23B
            | 0x1F240..=0x1F248
            | 0x1F250..=0x1F251
            | 0x1F260..=0x1F265
            | 0x1F300..=0x1F6D7
            | 0x1F6DC..=0x1F6EC
            | 0x1F6F0..=0x1F6FC
            | 0x1F700..=0x1F776
            | 0x1F77B..=0x1F7D9
            | 0x1F7E0..=0x1F7EB
            | 0x1F7F0
            | 0x1F800..=0x1F80B
            | 0x1F810..=0x1F847
            | 0x1F850..=0x1F859
            | 0x1F860..=0x1F887
            | 0x1F890..=0x1F8AD
            | 0x1F8B0..=0x1F8BB
            | 0x1F8C0..=0x1F8C1
            | 0x1F900..=0x1FA53
            | 0x1FA60..=0x1FA6D
            | 0x1FA70..=0x1FA7C
            | 0x1FA80..=0x1FA89
            | 0x1FA8F..=0x1FAC6
            | 0x1FACE..=0x1FADC
            | 0x1FADF..=0x1FAE9
            | 0x1FAF0..=0x1FAF8
            | 0x1FB00..=0x1FB92
            | 0x1FB94..=0x1FBEF
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
        assert!(matches!(&children[0], MdastNode::Text(t)
            if t.value == "知られる"
                && t.position == Some(Position::new(1, 1, 0, 1, 13, 12))));
        let MdastNode::Strong(repaired) = &children[1] else {
            unreachable!("expected repaired Strong, got {:?}", children[1])
        };
        assert_eq!(repaired.children.len(), 2);
        assert_eq!(repaired.children[0], original);
        assert_eq!(repaired.children[0].position(), original_position.as_ref());
        assert!(matches!(&repaired.children[1], MdastNode::Text(t)
            if t.value == "光学式コンプレッサー"
                && t.position == Some(Position::new(1, 43, 42, 1, 73, 72))));
        assert_eq!(repaired.position, Some(Position::new(1, 13, 12, 1, 75, 74)));
        assert!(matches!(&children[2], MdastNode::Text(t)
            if t.value == "と、"
                && t.position == Some(Position::new(1, 75, 74, 1, 81, 80))));
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
        let input = "漢**[a](/a)![b](/b)[c][id]![q][id]*d*~~e~~語** end\n\n[id]: /c\n";
        let mut mdast = markdown::to_mdast(input, &markdown::ParseOptions::gfm()).unwrap();
        CjkFriendlyPlugin::new().visit(&mut mdast);
        let children = first_paragraph_children(&mdast);
        let MdastNode::Strong(repaired) = &children[1] else {
            unreachable!("expected Strong, got {children:#?}")
        };
        assert!(matches!(&repaired.children[0], MdastNode::Link(_)));
        assert!(matches!(&repaired.children[1], MdastNode::Image(_)));
        assert!(matches!(&repaired.children[2], MdastNode::LinkReference(_)));
        assert!(matches!(
            &repaired.children[3],
            MdastNode::ImageReference(_)
        ));
        assert!(matches!(&repaired.children[4], MdastNode::Emphasis(_)));
        assert!(matches!(&repaired.children[5], MdastNode::Delete(_)));
        assert!(matches!(&repaired.children[6], MdastNode::Text(t) if t.value == "語"));
    }

    #[test]
    fn existing_strong_moves_intact_under_repaired_wrapper() {
        let input = "漢**[x](/x) **nested** 語.**漢";
        let parsed = markdown::to_mdast(input, &markdown::ParseOptions::mdx()).unwrap();
        let original = first_paragraph_children(&parsed)
            .iter()
            .find(|node| matches!(node, MdastNode::Strong(_)))
            .expect("markdown-rs should parse the nested strong")
            .clone();

        let transformed = run(input);
        let repaired = first_paragraph_children(&transformed)
            .iter()
            .find_map(|node| match node {
                MdastNode::Strong(strong)
                    if strong
                        .children
                        .iter()
                        .any(|child| matches!(child, MdastNode::Link(_))) =>
                {
                    Some(strong)
                }
                _ => None,
            })
            .expect("expected repaired outer strong");
        assert!(repaired.children.iter().any(|node| node == &original));
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
            MdastNode::InlineMath(markdown::mdast::InlineMath {
                value: "math".into(),
                position: None,
            }),
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
    fn parsed_movable_nodes_use_source_punctuation_not_visible_text() {
        let parsed = markdown::to_mdast(
            "[link](/u) ![image](/u) [reference][id] ![image-ref][id] *em* **strong** ~~delete~~\n\n[id]: /u",
            &markdown::ParseOptions::gfm(),
        )
        .unwrap();
        let punctuation = Some((Boundary::NonCjkPunctuation, Boundary::NonCjkPunctuation));
        let mut seen = [false; 7];
        for child in first_paragraph_children(&parsed) {
            let index = match child {
                MdastNode::Link(_) => 0,
                MdastNode::Image(_) => 1,
                MdastNode::LinkReference(_) => 2,
                MdastNode::ImageReference(_) => 3,
                MdastNode::Emphasis(_) => 4,
                MdastNode::Strong(_) => 5,
                MdastNode::Delete(_) => 6,
                _ => continue,
            };
            seen[index] = true;
            assert_eq!(source_boundaries(child), punctuation, "{child:#?}");
            assert!(is_movable(child));
        }
        assert_eq!(seen, [true; 7]);
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
    fn unknown_outer_boundary_is_used_only_when_flanking_is_invariant() {
        fn positionless_link() -> MdastNode {
            MdastNode::Link(Link {
                children: vec![MdastNode::Text(Text {
                    value: "https://example.com".into(),
                    position: None,
                })],
                position: None,
                url: "https://example.com".into(),
                title: None,
            })
        }

        let repaired = retokenise_children(vec![
            positionless_link(),
            MdastNode::Text(Text {
                value: "**重要。**テスト".into(),
                position: None,
            }),
        ]);
        assert!(
            matches!(&repaired[1], MdastNode::Strong(_)),
            "{repaired:#?}"
        );

        let material_unknown = vec![
            positionless_link(),
            MdastNode::Text(Text {
                value: "**[重要]**テスト".into(),
                position: None,
            }),
        ];
        assert_eq!(
            retokenise_children(material_unknown.clone()),
            material_unknown
        );
    }

    #[test]
    fn point_reconstruction_uses_source_bytes_and_tab_stops() {
        assert_eq!(
            point_after(&Point::new(1, 1, 0), "漢\tA\n字"),
            Point::new(2, 4, 9)
        );
        assert_eq!(
            point_after(&Point::new(1, 2, 10), "\tX"),
            Point::new(1, 6, 12)
        );
        assert_eq!(
            point_after(&Point::new(1, 1, 0), "A\r\nB\rC"),
            Point::new(3, 2, 6)
        );
    }

    #[test]
    fn cjk_variation_sequences_and_ideographic_selectors_flank_openers() {
        for input in [
            "漢\u{FE0E}**[x](/x)語** end",
            "漢\u{E0100}**[x](/x)語** end",
        ] {
            let transformed = run(input);
            assert!(
                first_paragraph_children(&transformed)
                    .iter()
                    .any(|node| matches!(node, MdastNode::Strong(_))),
                "{transformed:#?}"
            );
        }

        let emoji_selector = run("漢\u{FE0F}**[x](/x)語** end");
        assert!(
            !first_paragraph_children(&emoji_selector)
                .iter()
                .any(|node| matches!(node, MdastNode::Strong(_))),
            "{emoji_selector:#?}"
        );
    }

    #[test]
    fn angle_and_bare_autolinks_use_their_actual_source_boundaries() {
        let mut angle = markdown::to_mdast(
            "漢**<https://example.com>語** end",
            &markdown::ParseOptions::gfm(),
        )
        .unwrap();
        CjkFriendlyPlugin::new().visit(&mut angle);
        assert!(
            first_paragraph_children(&angle)
                .iter()
                .any(|node| matches!(node, MdastNode::Strong(strong) if strong.children.iter().any(|child| matches!(child, MdastNode::Link(_))))),
            "{angle:#?}"
        );

        let mut blocked_angle = markdown::to_mdast(
            "A**<https://example.com>語.**漢",
            &markdown::ParseOptions::gfm(),
        )
        .unwrap();
        CjkFriendlyPlugin::new().visit(&mut blocked_angle);
        assert!(
            !first_paragraph_children(&blocked_angle)
                .iter()
                .any(|node| matches!(node, MdastNode::Strong(_))),
            "{blocked_angle:#?}"
        );

        let mut bare = markdown::to_mdast(
            "A**https://example.com 語.**漢",
            &markdown::ParseOptions::gfm(),
        )
        .unwrap();
        CjkFriendlyPlugin::new().visit(&mut bare);
        assert!(
            first_paragraph_children(&bare)
                .iter()
                .any(|node| matches!(node, MdastNode::Strong(strong) if strong.children.iter().any(|child| matches!(child, MdastNode::Link(_))))),
            "{bare:#?}"
        );

        let mut blocked_bare = markdown::to_mdast(
            "A**https://example.com word.**B",
            &markdown::ParseOptions::gfm(),
        )
        .unwrap();
        CjkFriendlyPlugin::new().visit(&mut blocked_bare);
        let children = first_paragraph_children(&blocked_bare);
        assert!(children
            .iter()
            .any(|node| matches!(node, MdastNode::Link(_))));
        assert!(!children
            .iter()
            .any(|node| matches!(node, MdastNode::Strong(_))));
    }

    #[test]
    fn escaped_unmatched_and_unprovable_text_remain_literal() {
        for input in [
            "漢\\**[x](/x)語** end",
            "漢**[x](/x)語\\** end",
            "漢**[x](/x)語 end",
            "&amp;漢**[x](/x)語** end",
        ] {
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
    fn amendment_uses_full_commonmark_unicode_punctuation() {
        let h = run("漢**©thing** end");
        assert_eq!(dump(&h), "漢[STRONG:©thing] end");
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
