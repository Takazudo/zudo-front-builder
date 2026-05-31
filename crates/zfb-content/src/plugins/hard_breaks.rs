//! Hard-break post-processor: convert soft line breaks inside paragraphs into
//! `MdastNode::Break` (renders as `<br>`).
//!
//! Mirrors [`remark-breaks`][upstream] parity. A single `\n` inside a
//! paragraph text node becomes an explicit `Break` node so the HTML/JSX
//! emitters output `<br>`. Two or more blank lines between paragraphs are
//! already separate `Paragraph` nodes in the mdast tree and are unaffected.
//!
//! **Visitor contract** — the same no-recurse boundary set as
//! [`CjkFriendlyPlugin`] is used: `Code`, `InlineCode`, `Html`, and MDX nodes
//! are skipped so verbatim content is never split.
//!
//! Register AFTER [`CjkFriendlyPlugin`] in the mdast phase so CJK
//! emphasis re-tokenisation sees intact `Text` nodes first. The emitters
//! (`pipeline.rs` hast path and `mdx_jsx_emit.rs` JSX path) already map
//! `MdastNode::Break → <br>` — no emitter changes required.
//!
//! [upstream]: https://www.npmjs.com/package/remark-breaks
//! [`CjkFriendlyPlugin`]: super::cjk_friendly::CjkFriendlyPlugin

use markdown::mdast::{Break, Node as MdastNode, Text};

use crate::pipeline::MdastVisitor;

/// Visitor that splits `Text` nodes containing `\n` into `Text` / `Break`
/// sequences so soft line breaks render as `<br>`.
#[derive(Debug, Default, Clone, Copy)]
pub struct HardBreaksPlugin;

impl HardBreaksPlugin {
    /// New visitor; stateless.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl MdastVisitor for HardBreaksPlugin {
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

    if has_inline_children(node) {
        if let Some(children) = node.children_mut() {
            let new_children = split_children(std::mem::take(children));
            *children = new_children;
        }
    }

    if let Some(children) = node.children_mut() {
        for child in children {
            rewrite_inline_children(child);
        }
    }
}

/// Nodes whose subtree must NOT be split on `\n`. The visitor stops at
/// these — neither rewriting their direct children nor recursing into
/// them. Mirrors the same boundary set as `CjkFriendlyPlugin`.
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
/// Mirrors `CjkFriendlyPlugin::has_inline_children`.
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

/// Walk `children`, splitting any `Text` node containing `\n` into
/// alternating `Text` / `Break` / `Text` sequences. Other children pass
/// through unchanged.
fn split_children(children: Vec<MdastNode>) -> Vec<MdastNode> {
    let mut out = Vec::with_capacity(children.len());
    for child in children {
        match child {
            MdastNode::Text(t) if t.value.contains('\n') => {
                out.extend(split_text_on_newline(&t.value));
            }
            other => out.push(other),
        }
    }
    out
}

/// Split `value` on `\n`, emitting `Text` fragments interleaved with
/// `Break` nodes. Empty fragments (from leading/trailing `\n`) are
/// preserved as empty `Text` nodes to avoid discarding whitespace that
/// authors may have intentionally placed.
fn split_text_on_newline(value: &str) -> Vec<MdastNode> {
    let mut out = Vec::new();
    let mut first = true;
    for fragment in value.split('\n') {
        if !first {
            out.push(MdastNode::Break(Break { position: None }));
        }
        out.push(MdastNode::Text(Text {
            value: fragment.to_string(),
            position: None,
        }));
        first = false;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &str) -> MdastNode {
        let mut mdast = markdown::to_mdast(input, &markdown::ParseOptions::mdx())
            .expect("markdown-rs should parse fixture");
        HardBreaksPlugin::new().visit(&mut mdast);
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

    #[test]
    fn soft_newline_becomes_break() {
        // "first\nsecond" inside a paragraph → [Text("first"), Break, Text("second")]
        let h = run("first\nsecond\n");
        let p = first_paragraph_children(&h);
        assert_eq!(p.len(), 3, "expected Text+Break+Text, got {p:?}");
        assert!(matches!(&p[0], MdastNode::Text(t) if t.value == "first"), "p[0]: {p:?}");
        assert!(matches!(&p[1], MdastNode::Break(_)), "p[1]: {p:?}");
        assert!(matches!(&p[2], MdastNode::Text(t) if t.value == "second"), "p[2]: {p:?}");
    }

    #[test]
    fn multiple_soft_newlines() {
        // "a\nb\nc" → Text("a") + Break + Text("b") + Break + Text("c")
        let h = run("a\nb\nc\n");
        let p = first_paragraph_children(&h);
        assert_eq!(p.len(), 5, "expected 5 nodes, got {p:?}");
        assert!(matches!(&p[0], MdastNode::Text(t) if t.value == "a"));
        assert!(matches!(&p[1], MdastNode::Break(_)));
        assert!(matches!(&p[2], MdastNode::Text(t) if t.value == "b"));
        assert!(matches!(&p[3], MdastNode::Break(_)));
        assert!(matches!(&p[4], MdastNode::Text(t) if t.value == "c"));
    }

    #[test]
    fn no_newline_unchanged() {
        // No `\n` in paragraph text → children unmodified.
        let h = run("hello world\n");
        let p = first_paragraph_children(&h);
        assert_eq!(p.len(), 1, "expected single Text node, got {p:?}");
        assert!(matches!(&p[0], MdastNode::Text(t) if t.value == "hello world"));
    }

    #[test]
    fn blank_line_separation_unaffected() {
        // Blank-line paragraph separation → still two Paragraph nodes; no Break added.
        let h = run("para one\n\npara two\n");
        let MdastNode::Root(r) = &h else { unreachable!() };
        assert_eq!(
            r.children.len(),
            2,
            "expected 2 paragraphs, got {:?}",
            r.children
        );
        let MdastNode::Paragraph(p1) = &r.children[0] else { unreachable!() };
        let MdastNode::Paragraph(p2) = &r.children[1] else { unreachable!() };
        // Neither paragraph should contain a Break.
        for node in p1.children.iter().chain(p2.children.iter()) {
            assert!(
                !matches!(node, MdastNode::Break(_)),
                "unexpected Break in paragraph children"
            );
        }
    }

    #[test]
    fn inline_code_not_split() {
        // `\n` inside inline code must NOT become Break.
        let h = run("before `code\nnext` after\n");
        let p = first_paragraph_children(&h);
        let mut found_break = false;
        for node in p {
            if let MdastNode::Break(_) = node {
                found_break = true;
            }
            if let MdastNode::InlineCode(c) = node {
                // The InlineCode value contains the literal newline (or a
                // space as markdown-rs may normalise it); it must NOT
                // be split into a Break node.
                assert!(
                    !c.value.is_empty(),
                    "InlineCode node must be present and intact"
                );
            }
        }
        assert!(
            !found_break,
            "Break must not appear when newline is inside inline code; got {p:?}"
        );
    }

    #[test]
    fn fenced_code_not_split() {
        let mut mdast = markdown::to_mdast(
            "```\nfirst\nsecond\n```\n",
            &markdown::ParseOptions::mdx(),
        )
        .unwrap();
        HardBreaksPlugin::new().visit(&mut mdast);
        let MdastNode::Root(r) = &mdast else { unreachable!() };
        let MdastNode::Code(c) = &r.children[0] else {
            unreachable!("expected Code, got {:?}", r.children[0])
        };
        assert_eq!(c.value, "first\nsecond", "fenced code value must be intact");
    }

    #[test]
    fn explicit_hard_break_backslash_unaffected() {
        // A trailing-backslash hard break is already a Break node in mdast
        // before our visitor runs; the visitor sees only Text children, so
        // the existing Break must be preserved alongside the text.
        let h = run("first\\\nsecond\n");
        let p = first_paragraph_children(&h);
        let has_break = p.iter().any(|n| matches!(n, MdastNode::Break(_)));
        assert!(
            has_break,
            "backslash hard break must still produce a Break node; got {p:?}"
        );
    }

    #[test]
    fn mdx_jsx_text_element_not_split() {
        let mut mdast = markdown::to_mdast(
            "Outside <Inline>line one\nline two</Inline> after\n",
            &markdown::ParseOptions::mdx(),
        )
        .unwrap();
        HardBreaksPlugin::new().visit(&mut mdast);
        // Walk and assert no Break is inside an MdxJsxTextElement.
        fn find_break_in_jsx(n: &MdastNode, found: &mut bool) {
            if let MdastNode::MdxJsxTextElement(j) = n {
                for c in &j.children {
                    if let MdastNode::Break(_) = c {
                        *found = true;
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
                    find_break_in_jsx(c, found);
                }
            }
        }
        let mut found = false;
        find_break_in_jsx(&mdast, &mut found);
        assert!(
            !found,
            "Break must not appear inside MdxJsxTextElement; got {mdast:#?}"
        );
    }
}
