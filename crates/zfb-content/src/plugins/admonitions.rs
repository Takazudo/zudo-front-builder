//! `:::note … :::` style admonitions.
//!
//! Rust port of zudo-doc's `remarkAdmonitions`. Detects directive-style
//! admonition blocks at the mdast level and converts them to MDX
//! [`MdxJsxFlowElement`] nodes named `Note`, `Tip`, `Warning`,
//! `Danger`, `Info`, or `Details` so downstream MDX components can
//! render them.
//!
//! markdown-rs's `ParseOptions::mdx()` does NOT auto-emit directive
//! AST nodes (CommonMark + MDX has no directive grammar). We
//! therefore detect the syntax by scanning consecutive top-level
//! `Paragraph` nodes whose text content looks like:
//!
//! ```text
//! :::note
//! body lines…
//! :::
//! ```
//!
//! Each `:::<kind>` (optionally `:::details title="…"` for details)
//! opens a block; the block ends at the first paragraph that contains
//! exactly `:::` on its first text run. Everything between (excluding
//! both fences) becomes the children of the synthesized JSX element.
//!
//! The walk is recursive so admonitions can appear inside lists,
//! blockquotes, or nested MDX flow elements.

use markdown::mdast::{
    AttributeContent, AttributeValue, MdxJsxAttribute, MdxJsxFlowElement, Node as MdastNode,
};

use crate::pipeline::MdastVisitor;

/// Visitor that converts `:::kind … :::` paragraph runs into
/// `<Kind>…</Kind>` MDX flow elements.
#[derive(Debug, Default, Clone, Copy)]
pub struct AdmonitionsPlugin;

impl AdmonitionsPlugin {
    /// New plugin (stateless).
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl MdastVisitor for AdmonitionsPlugin {
    fn visit(&mut self, node: &mut MdastNode) {
        if let Some(children) = node.children_mut() {
            transform_children(children);
            for c in children.iter_mut() {
                self.visit(c);
            }
        }
    }
}

/// Scan a children list and rewrite `:::kind / :::` runs in place.
fn transform_children(children: &mut Vec<MdastNode>) {
    let mut i = 0;
    while i < children.len() {
        if let Some((kind, title)) = parse_open(&children[i]) {
            // Look for matching close.
            let close_idx = children
                .iter()
                .enumerate()
                .skip(i + 1)
                .find(|(_, c)| is_close(c))
                .map(|(j, _)| j);
            if let Some(close) = close_idx {
                // Drain [i ..= close] and replace with one JSX element.
                let body: Vec<MdastNode> = children.drain(i..=close).collect();
                let inner = body[1..body.len() - 1].to_vec(); // strip open + close paragraphs
                let jsx = build_jsx(&kind, title.as_deref(), inner);
                children.insert(i, jsx);
            }
        }
        i += 1;
    }
}

/// If `node` is a paragraph whose first text run starts with
/// `:::<kind>[…]`, return `(tag_name, optional_title)`.
fn parse_open(node: &MdastNode) -> Option<(String, Option<String>)> {
    let para = match node {
        MdastNode::Paragraph(p) => p,
        _ => return None,
    };
    let first = para.children.first()?;
    let MdastNode::Text(t) = first else {
        return None;
    };
    let line = first_line(&t.value).trim_end();
    let rest = line.strip_prefix(":::")?;
    if rest.is_empty() || rest == ":" {
        return None;
    }
    // Split into the kind word + optional rest.
    let (kind_raw, rest_after) = split_first_word(rest);
    let tag = match kind_raw {
        "note" => "Note",
        "tip" => "Tip",
        "warning" => "Warning",
        "danger" => "Danger",
        "info" => "Info",
        "details" => "Details",
        _ => return None,
    };
    let title = if tag == "Details" {
        extract_title_attr(rest_after.trim())
    } else {
        None
    };
    Some((tag.to_string(), title))
}

/// True if `node` is a paragraph whose first text run is exactly
/// `:::` (the closing fence).
fn is_close(node: &MdastNode) -> bool {
    let MdastNode::Paragraph(p) = node else {
        return false;
    };
    let Some(MdastNode::Text(t)) = p.children.first() else {
        return false;
    };
    first_line(&t.value).trim() == ":::"
}

fn first_line(s: &str) -> &str {
    match s.find('\n') {
        Some(i) => &s[..i],
        None => s,
    }
}

fn split_first_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.find(|c: char| c.is_whitespace()) {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    }
}

/// Parse an attribute string like `title="Click me"` and return the
/// value of `title` if present.
fn extract_title_attr(s: &str) -> Option<String> {
    let rest = s.strip_prefix("title")?.trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn build_jsx(tag: &str, title: Option<&str>, children: Vec<MdastNode>) -> MdastNode {
    let mut attributes: Vec<AttributeContent> = Vec::new();
    if let Some(t) = title {
        attributes.push(AttributeContent::Property(MdxJsxAttribute {
            name: "title".to_string(),
            value: Some(AttributeValue::Literal(t.to_string())),
        }));
    }
    MdastNode::MdxJsxFlowElement(MdxJsxFlowElement {
        children,
        position: None,
        name: Some(tag.to_string()),
        attributes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use markdown::mdast::{Paragraph, Root, Text};

    fn text_para(value: &str) -> MdastNode {
        MdastNode::Paragraph(Paragraph {
            children: vec![MdastNode::Text(Text {
                value: value.to_string(),
                position: None,
            })],
            position: None,
        })
    }

    fn run_root(children: Vec<MdastNode>) -> MdastNode {
        let mut root = MdastNode::Root(Root {
            children,
            position: None,
        });
        AdmonitionsPlugin::new().visit(&mut root);
        root
    }

    #[test]
    fn converts_note_block() {
        let root = run_root(vec![
            text_para(":::note"),
            text_para("body content"),
            text_para(":::"),
        ]);
        let MdastNode::Root(Root { children, .. }) = root else {
            panic!()
        };
        assert_eq!(children.len(), 1);
        let MdastNode::MdxJsxFlowElement(j) = &children[0] else {
            panic!("expected MdxJsxFlowElement, got {:?}", children[0])
        };
        assert_eq!(j.name.as_deref(), Some("Note"));
        assert_eq!(j.children.len(), 1, "body para preserved");
        assert!(j.attributes.is_empty());
    }

    #[test]
    fn converts_all_kinds() {
        for (key, tag) in [
            ("note", "Note"),
            ("tip", "Tip"),
            ("warning", "Warning"),
            ("danger", "Danger"),
            ("info", "Info"),
            ("details", "Details"),
        ] {
            let root = run_root(vec![
                text_para(&format!(":::{key}")),
                text_para("body"),
                text_para(":::"),
            ]);
            let MdastNode::Root(Root { children, .. }) = root else {
                panic!()
            };
            assert_eq!(children.len(), 1);
            let MdastNode::MdxJsxFlowElement(j) = &children[0] else {
                panic!()
            };
            assert_eq!(j.name.as_deref(), Some(tag), "kind {key}");
        }
    }

    #[test]
    fn captures_details_title() {
        let root = run_root(vec![
            text_para(":::details title=\"Click me\""),
            text_para("hidden"),
            text_para(":::"),
        ]);
        let MdastNode::Root(Root { children, .. }) = root else {
            panic!()
        };
        let MdastNode::MdxJsxFlowElement(j) = &children[0] else {
            panic!()
        };
        assert_eq!(j.attributes.len(), 1);
        let AttributeContent::Property(prop) = &j.attributes[0] else {
            panic!()
        };
        assert_eq!(prop.name, "title");
        match &prop.value {
            Some(AttributeValue::Literal(v)) => assert_eq!(v, "Click me"),
            other => panic!("expected literal, got {other:?}"),
        }
    }

    #[test]
    fn unknown_kind_left_alone() {
        let root = run_root(vec![
            text_para(":::nope"),
            text_para("body"),
            text_para(":::"),
        ]);
        let MdastNode::Root(Root { children, .. }) = root else {
            panic!()
        };
        // No transformation: 3 paragraphs preserved.
        assert_eq!(children.len(), 3);
    }

    #[test]
    fn missing_close_left_alone() {
        let root = run_root(vec![text_para(":::note"), text_para("body")]);
        let MdastNode::Root(Root { children, .. }) = root else {
            panic!()
        };
        assert_eq!(children.len(), 2);
        assert!(matches!(children[0], MdastNode::Paragraph(_)));
    }
}
