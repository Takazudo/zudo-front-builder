//! Nested-link normalisation for GFM autolink literals (zfb#2388).
//!
//! GFM's autolink-literal extension is specified to skip content that is
//! already inside a link — cmark-gfm's extension walks the AST and never
//! descends into a `CMARK_NODE_LINK`. `markdown-rs` (the `markdown` crate)
//! does not: its autolink pass fires inside a link label too, so an
//! ordinary markdown link whose label contains a bare URL yields an `<a>`
//! nested in an `<a>`:
//!
//! ```text
//! [http://localhost:4321](http://localhost:4321)
//! → <a href="http://localhost:4321"><a href="http://localhost:4321">…</a></a>
//! ```
//!
//! Nested anchors are invalid HTML (`html-validate`:
//! `element-permitted-content`), and the defect became reachable on
//! previously-valid documents the moment zfb 2.5.0 flipped
//! `gfm.autolinkLiteral` on by default (a5be2fca) — a silent behavioural
//! regression, not a new authoring mistake.
//!
//! `markdown-rs` is a plain crates.io dependency whose tokeniser is not
//! configurable from outside, so — exactly like
//! [`CjkAutolinkBoundaryPlugin`](super::cjk_autolink) does for the CJK
//! boundary defect — the fix is a post-parse mdast pass.
//!
//! # Why unwrapping the *inner* link is the correct normalisation
//!
//! The spec position is that the inner autolink should never have been
//! created at all. Replacing the inner `Link` with its own children
//! reproduces exactly that: the visible text survives, and the enclosing
//! link keeps the destination the author actually wrote.
//!
//! # Why any nested link is safe to unwrap
//!
//! CommonMark forbids links inside links, and `markdown-rs` enforces it for
//! *explicit* syntax — `[a [b](c) d](e)` degrades the outer link to literal
//! text rather than nesting. So a `Link` that is a descendant of another
//! link can only be an autolink-literal artifact, and unwrapping it never
//! discards author-written link syntax.
//!
//! # Shapes this has to cover
//!
//! The nesting is not always a direct child — the autolink fires anywhere
//! inside the label, including through intervening inline markup, so the
//! walk has to recurse while remembering it is inside a link:
//!
//! ```text
//! [**bold https://example.com**](https://x.com)
//! → <a …><strong>bold <a …>…</a></strong></a>
//! ```
//!
//! It also fires inside the *inner* link of an outer link that CommonMark
//! already degraded (`[a [b https://e.com c](d) e](f)`), and inside link
//! labels in table cells and list items — all reached by the same recursion.
//!
//! Constructs the autolink pass does not enter (inline code, raw HTML,
//! image alt text) never produce the nesting, and MDX JSX / expression
//! bodies are author-owned; both are skipped via [`is_no_recurse`], the
//! same boundary set [`CjkAutolinkBoundaryPlugin`](super::cjk_autolink) and
//! [`CjkFriendlyPlugin`](super::cjk_friendly) use.
//!
//! # Why this is applied at the parse sites, not registered as a visitor
//!
//! It runs immediately after `markdown::to_mdast`, ahead of the visitor
//! chain, for two reasons. It covers every pipeline regardless of how it
//! was constructed — the mdast visitors are only wired by the
//! `with_defaults*` constructors, so a bare [`Pipeline::new`] would
//! otherwise keep emitting nested anchors. And it guarantees that no
//! visitor ever observes the malformed tree:
//! [`CjkAutolinkBoundaryPlugin`](super::cjk_autolink) and
//! [`CjkFriendlyPlugin`](super::cjk_friendly) both walk `Link` nodes, and
//! reasoning about them against a shape the GFM spec says cannot exist is
//! a trap worth removing outright.
//!
//! [`Pipeline::new`]: crate::pipeline::Pipeline::new

use markdown::mdast::Node as MdastNode;

/// Unwrap every `Link` / `LinkReference` that is nested inside another link,
/// replacing it with its own children.
///
/// Call directly after `markdown::to_mdast` and only when
/// `constructs.gfm_autolink_literal` is on — with the construct off,
/// `markdown-rs` produces no nested links at all, so gating keeps that
/// configuration's output provably byte-identical.
pub fn unwrap_nested_links(node: &mut MdastNode) {
    rewrite(node, false);
}

/// Walk `node`, flattening link children once `inside_link` holds, then
/// recurse. Stops at no-recurse boundaries (verbatim / author-owned
/// content) — mirrors [`CjkAutolinkBoundaryPlugin`](super::cjk_autolink).
fn rewrite(node: &mut MdastNode, inside_link: bool) {
    if is_no_recurse(node) {
        return;
    }
    let inside_link = inside_link || is_link(node);
    if inside_link {
        if let Some(children) = node.children_mut() {
            let flattened = flatten_links(std::mem::take(children));
            *children = flattened;
        }
    }
    if let Some(children) = node.children_mut() {
        for child in children {
            rewrite(child, inside_link);
        }
    }
}

/// True for the two mdast nodes that render as an `<a>` element.
///
/// `Definition` is deliberately absent — it carries no children and emits
/// nothing. `FootnoteReference` also renders an anchor, but it is a leaf
/// the autolink pass cannot nest a link inside, so it is out of scope here.
fn is_link(node: &MdastNode) -> bool {
    matches!(node, MdastNode::Link(_) | MdastNode::LinkReference(_))
}

/// Nodes whose subtree must NOT be touched: verbatim code, raw HTML, and
/// MDX JSX / expression bodies (author-controlled). Same set as
/// [`CjkAutolinkBoundaryPlugin`](super::cjk_autolink) and
/// [`CjkFriendlyPlugin`](super::cjk_friendly).
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

/// Replace each `Link` / `LinkReference` in `children` with its own
/// children. Recurses into the spliced-in run so a directly-stacked
/// `Link > Link > Link` collapses in one pass; every other child (notably
/// `Image`, which is legal inside a link) passes through untouched.
fn flatten_links(children: Vec<MdastNode>) -> Vec<MdastNode> {
    let mut out = Vec::with_capacity(children.len());
    for child in children {
        match child {
            MdastNode::Link(l) => out.extend(flatten_links(l.children)),
            MdastNode::LinkReference(l) => out.extend(flatten_links(l.children)),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use markdown::mdast::{
        Image, InlineCode, Link, LinkReference, Paragraph, ReferenceKind, Root, Strong, Text,
    };

    fn text(value: &str) -> MdastNode {
        MdastNode::Text(Text {
            value: value.to_string(),
            position: None,
        })
    }

    fn link(url: &str, children: Vec<MdastNode>) -> MdastNode {
        MdastNode::Link(Link {
            url: url.to_string(),
            title: None,
            children,
            position: None,
        })
    }

    fn link_ref(identifier: &str, children: Vec<MdastNode>) -> MdastNode {
        MdastNode::LinkReference(LinkReference {
            identifier: identifier.to_string(),
            label: Some(identifier.to_string()),
            reference_kind: ReferenceKind::Full,
            children,
            position: None,
        })
    }

    fn para(children: Vec<MdastNode>) -> MdastNode {
        MdastNode::Paragraph(Paragraph {
            children,
            position: None,
        })
    }

    fn root(children: Vec<MdastNode>) -> MdastNode {
        MdastNode::Root(Root {
            children,
            position: None,
        })
    }

    fn strong(children: Vec<MdastNode>) -> MdastNode {
        MdastNode::Strong(Strong {
            children,
            position: None,
        })
    }

    /// The issue's headline repro: a label that is itself a bare URL.
    #[test]
    fn unwraps_a_directly_nested_autolink() {
        let mut tree = root(vec![para(vec![link(
            "http://localhost:4321",
            vec![link(
                "http://localhost:4321",
                vec![text("http://localhost:4321")],
            )],
        )])]);

        unwrap_nested_links(&mut tree);

        let expected = root(vec![para(vec![link(
            "http://localhost:4321",
            vec![text("http://localhost:4321")],
        )])]);
        assert_eq!(tree, expected);
    }

    /// A bare URL merely embedded in the label keeps its surrounding text.
    #[test]
    fn unwraps_an_embedded_autolink_and_keeps_siblings() {
        let mut tree = root(vec![para(vec![link(
            "https://example.com",
            vec![
                text("see "),
                link("https://example.com", vec![text("https://example.com")]),
                text(" now"),
            ],
        )])]);

        unwrap_nested_links(&mut tree);

        let expected = root(vec![para(vec![link(
            "https://example.com",
            vec![text("see "), text("https://example.com"), text(" now")],
        )])]);
        assert_eq!(tree, expected);
    }

    /// The nesting is not always a direct child — the autolink fires through
    /// intervening inline markup, so the walk must stay "inside a link".
    #[test]
    fn unwraps_through_intervening_inline_markup() {
        let mut tree = root(vec![para(vec![link(
            "https://x.com",
            vec![strong(vec![
                text("bold "),
                link("https://example.com", vec![text("https://example.com")]),
            ])],
        )])]);

        unwrap_nested_links(&mut tree);

        let expected = root(vec![para(vec![link(
            "https://x.com",
            vec![strong(vec![text("bold "), text("https://example.com")])],
        )])]);
        assert_eq!(tree, expected);
    }

    /// `Link > Link > Link` collapses in a single pass.
    #[test]
    fn collapses_directly_stacked_links() {
        let mut tree = root(vec![para(vec![link(
            "https://a.com",
            vec![link(
                "https://a.com",
                vec![link("https://a.com", vec![text("https://a.com")])],
            )],
        )])]);

        unwrap_nested_links(&mut tree);

        let expected = root(vec![para(vec![link(
            "https://a.com",
            vec![text("https://a.com")],
        )])]);
        assert_eq!(tree, expected);
    }

    /// `LinkReference` is an `<a>` too — nested either way round.
    #[test]
    fn unwraps_link_references_in_both_positions() {
        let mut tree = root(vec![
            para(vec![link(
                "https://x.com",
                vec![link_ref("ref", vec![text("label")])],
            )]),
            para(vec![link_ref(
                "ref",
                vec![link("https://y.com", vec![text("https://y.com")])],
            )]),
        ]);

        unwrap_nested_links(&mut tree);

        let expected = root(vec![
            para(vec![link("https://x.com", vec![text("label")])]),
            para(vec![link_ref("ref", vec![text("https://y.com")])]),
        ]);
        assert_eq!(tree, expected);
    }

    /// A top-level autolink is not nested — it must survive untouched.
    #[test]
    fn leaves_an_unnested_link_alone() {
        let mut tree = root(vec![para(vec![
            text("bare "),
            link("https://example.com", vec![text("https://example.com")]),
            text(" here"),
        ])]);
        let expected = tree.clone();

        unwrap_nested_links(&mut tree);

        assert_eq!(tree, expected);
    }

    /// An `Image` inside a link is valid HTML and must be preserved.
    #[test]
    fn preserves_an_image_inside_a_link() {
        let mut tree = root(vec![para(vec![link(
            "https://example.com",
            vec![MdastNode::Image(Image {
                url: "img.png".to_string(),
                alt: "alt".to_string(),
                title: None,
                position: None,
            })],
        )])]);
        let expected = tree.clone();

        unwrap_nested_links(&mut tree);

        assert_eq!(tree, expected);
    }

    /// Author-owned / verbatim subtrees are boundaries: a `Link` parked
    /// inside one is left exactly as written.
    #[test]
    fn does_not_descend_into_no_recurse_subtrees() {
        let mut tree = root(vec![para(vec![link(
            "https://x.com",
            vec![
                MdastNode::InlineCode(InlineCode {
                    value: "https://code.com".to_string(),
                    position: None,
                }),
                MdastNode::MdxJsxTextElement(markdown::mdast::MdxJsxTextElement {
                    name: Some("Foo".to_string()),
                    attributes: vec![],
                    children: vec![link("https://inner.com", vec![text("inner")])],
                    position: None,
                }),
            ],
        )])]);
        let expected = tree.clone();

        unwrap_nested_links(&mut tree);

        assert_eq!(tree, expected);
    }
}
