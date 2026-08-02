//! Shared MDX-JSX mdast helpers: synthesizing `MdxJsxTextElement` nodes and
//! walking/mutating a tree while tracking JSX-nesting ancestry.
//!
//! Home for two building blocks the JSX-nested mutation passes (#2248
//! ImageDimensions, #2249 ExternalLinks) both need. Landed here (rather than
//! in `zfb-md-extras` or `zfb-content`) per #2247: both consumer crates
//! already depend on `zfb-md-ast`, which owns the `MdastVisitor` /
//! `BuildContext` contract these passes implement against.

use markdown::mdast::{
    AttributeContent, AttributeValue, MdxJsxAttribute, MdxJsxTextElement, Node as MdastNode,
};
use markdown::unist::Position;

/// Build an `MdxJsxTextElement` node with `Literal`-valued attributes.
///
/// Every entry in `attrs` becomes an `AttributeContent::Property` whose
/// value is `AttributeValue::Literal` — the same shape
/// `DirectiveRegistry::build_attributes` produces
/// (`crates/zfb-content/src/plugins/directives.rs`), which
/// `JsxEmitter::emit_jsx` (`mdx_jsx_emit.rs`) already knows how to escape
/// and emit byte-identically to the native `Image`/`Link` arms.
///
/// Text (inline) variant only — `Image` and `Link` are phrasing content, so
/// a JSX-nested replacement for either is always inline. Do NOT add a flow
/// (`MdxJsxFlowElement`) sibling constructor here; nothing in this epic
/// needs one, and an unused constructor would just be surface area to keep
/// in sync. A `DirectiveRegistry` migration onto these constructors is an
/// optional follow-up cleanup, out of scope for this epic.
#[must_use]
pub fn jsx_text_element(
    name: &str,
    attrs: Vec<(String, String)>,
    children: Vec<MdastNode>,
    position: Option<Position>,
) -> MdastNode {
    let attributes = attrs
        .into_iter()
        .map(|(k, v)| {
            AttributeContent::Property(MdxJsxAttribute {
                name: k,
                value: Some(AttributeValue::Literal(v)),
            })
        })
        .collect();
    MdastNode::MdxJsxTextElement(MdxJsxTextElement {
        children,
        position,
        name: Some(name.to_string()),
        attributes,
    })
}

/// Ancestry-walk driver: pre-order visits every node reachable while
/// `in_jsx` tracking is `true`, and lets `f` mutate nodes in place.
///
/// Mutation-flavored mirror of `collect_jsx_nested_links`'s descent
/// (`crates/zfb-md-extras/src/link_validation.rs`) — same ancestry
/// contract, generalized so any JSX-nested mutation pass can share it
/// instead of re-deriving the ancestry rules:
///
/// - `in_jsx` flips to `true` for the children of `MdxJsxFlowElement` /
///   `MdxJsxTextElement` and STAYS `true` for everything below — JSX
///   nested inside JSX does not re-trigger anything, it is already inside
///   the "JSX-nested" partition.
/// - `FootnoteDefinition` RESETS `in_jsx` to `false` for its own subtree,
///   with re-arm on JSX authored inside the body. Same reasoning as the
///   collector: a footnote definition's body renders via the
///   document-level footnote section — which the hast-phase plugins DO
///   see structurally — so an ordinary (non-JSX-relocated) descendant
///   must NOT be treated as JSX-nested purely because some OUTER JSX
///   ancestor happens to contain the reference site. But JSX authored
///   INSIDE the definition body collapses to an opaque `JsxRaw` leaf in
///   that rendered section, invisible to the hast walk — so descent must
///   re-enter a nested JSX arm and flip `in_jsx` back to `true` there.
///   Do NOT "simplify" this into an unconditional skip (loses the
///   re-arm — definition-internal JSX would never be treated) or into
///   plain unmodified recursion that carries the incoming `in_jsx`
///   (reintroduces double-treatment: the hast walk already treats the
///   relocated section).
/// - The callback fires on every node for which the walk currently has
///   `in_jsx == true`, BEFORE recursing into that node — so a
///   replacement `f` makes in place is what gets descended into. It
///   never fires on a top-level node (the walk always starts with
///   `in_jsx = false`).
/// - Descent is GENERIC (`MdastNode::children_mut`) rather than a
///   per-node-kind arm list, mirroring the collector's own generic-descent
///   rationale: mutation targets (`Image`, `Link`) can live inside
///   `Paragraph`, `Table*`, `Emphasis`/`Strong`/`Delete`, … and a narrow
///   allowlist would silently miss coverage a future container type adds.
pub fn rewrite_jsx_nested(node: &mut MdastNode, f: &mut dyn FnMut(&mut MdastNode)) {
    walk(node, false, f);
}

fn walk(node: &mut MdastNode, in_jsx: bool, f: &mut dyn FnMut(&mut MdastNode)) {
    if in_jsx {
        f(node);
    }
    let child_in_jsx = match node {
        MdastNode::MdxJsxFlowElement(_) | MdastNode::MdxJsxTextElement(_) => true,
        MdastNode::FootnoteDefinition(_) => false,
        _ => in_jsx,
    };
    if let Some(children) = node.children_mut() {
        for child in children.iter_mut() {
            walk(child, child_in_jsx, f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use markdown::mdast::{FootnoteDefinition, Image, Link, MdxJsxFlowElement, Paragraph, Text};

    fn pos() -> Position {
        Position::new(1, 1, 0, 1, 5, 4)
    }

    fn text(value: &str) -> MdastNode {
        MdastNode::Text(Text {
            value: value.to_string(),
            position: None,
        })
    }

    // ── jsx_text_element ────────────────────────────────────────────────

    #[test]
    fn jsx_text_element_produces_literal_attribute_shape_and_carries_position() {
        let node = jsx_text_element(
            "_components.img",
            vec![
                ("src".to_string(), "/a.png".to_string()),
                ("alt".to_string(), "desc".to_string()),
            ],
            vec![],
            Some(pos()),
        );

        let MdastNode::MdxJsxTextElement(el) = node else {
            panic!("expected MdxJsxTextElement, got {node:?}");
        };
        assert_eq!(el.name.as_deref(), Some("_components.img"));
        assert_eq!(el.position, Some(pos()));
        assert!(el.children.is_empty());
        assert_eq!(el.attributes.len(), 2);
        for (attr, (expected_name, expected_value)) in el
            .attributes
            .iter()
            .zip([("src", "/a.png"), ("alt", "desc")])
        {
            let AttributeContent::Property(prop) = attr else {
                panic!("expected AttributeContent::Property, got {attr:?}");
            };
            assert_eq!(prop.name, expected_name);
            assert_eq!(
                prop.value,
                Some(AttributeValue::Literal(expected_value.to_string()))
            );
        }
    }

    #[test]
    fn jsx_text_element_carries_children() {
        let node = jsx_text_element("_components.a", vec![], vec![text("label")], None);
        let MdastNode::MdxJsxTextElement(el) = node else {
            panic!("expected MdxJsxTextElement, got {node:?}");
        };
        assert_eq!(el.children.len(), 1);
        assert!(matches!(&el.children[0], MdastNode::Text(t) if t.value == "label"));
    }

    // ── rewrite_jsx_nested ──────────────────────────────────────────────

    /// Marker helper: records every node the callback fired on, tagged by
    /// a cheap discriminant string so assertions can check exactly which
    /// nodes were (and were not) visited.
    fn tag(node: &MdastNode) -> &'static str {
        match node {
            MdastNode::Image(_) => "image",
            MdastNode::Link(_) => "link",
            MdastNode::Paragraph(_) => "paragraph",
            MdastNode::MdxJsxTextElement(_) => "jsx_text",
            MdastNode::MdxJsxFlowElement(_) => "jsx_flow",
            MdastNode::FootnoteDefinition(_) => "footnote_def",
            MdastNode::Text(_) => "text",
            _ => "other",
        }
    }

    fn image(url: &str) -> MdastNode {
        MdastNode::Image(Image {
            position: None,
            alt: String::new(),
            url: url.to_string(),
            title: None,
        })
    }

    fn jsx_flow(name: &str, children: Vec<MdastNode>) -> MdastNode {
        MdastNode::MdxJsxFlowElement(MdxJsxFlowElement {
            children,
            position: None,
            name: Some(name.to_string()),
            attributes: vec![],
        })
    }

    fn jsx_text(name: &str, children: Vec<MdastNode>) -> MdastNode {
        MdastNode::MdxJsxTextElement(MdxJsxTextElement {
            children,
            position: None,
            name: Some(name.to_string()),
            attributes: vec![],
        })
    }

    fn paragraph(children: Vec<MdastNode>) -> MdastNode {
        MdastNode::Paragraph(Paragraph {
            children,
            position: None,
        })
    }

    fn footnote_def(children: Vec<MdastNode>) -> MdastNode {
        MdastNode::FootnoteDefinition(FootnoteDefinition {
            children,
            position: None,
            identifier: "fn1".to_string(),
            label: None,
        })
    }

    /// A top-level (non-JSX-nested) `Image` never fires the callback,
    /// even though the walk visits it during descent.
    #[test]
    fn top_level_node_never_fires_callback() {
        let mut root = paragraph(vec![image("/a.png")]);
        let mut fired = Vec::new();
        rewrite_jsx_nested(&mut root, &mut |n| fired.push(tag(n)));
        assert!(fired.is_empty(), "top-level nodes must not fire: {fired:?}");
    }

    /// An `Image`/`Link` inside JSX fires — the partition-detection proof:
    /// only nodes strictly inside a JSX body are in scope.
    #[test]
    fn jsx_nested_image_and_link_fire_but_sibling_top_level_does_not() {
        let mut root = MdastNode::Root(markdown::mdast::Root {
            children: vec![
                paragraph(vec![image("/top-level.png")]),
                jsx_flow(
                    "Note",
                    vec![paragraph(vec![
                        image("/nested.png"),
                        MdastNode::Link(Link {
                            children: vec![text("label")],
                            position: None,
                            url: "/nested-link".to_string(),
                            title: None,
                        }),
                    ])],
                ),
            ],
            position: None,
        });

        let mut fired = Vec::new();
        rewrite_jsx_nested(&mut root, &mut |n| fired.push(tag(n)));

        // The JSX element itself is entered under the AMBIENT `in_jsx`
        // (`false`, since it's a direct child of Root) and so does not
        // fire — only ITS children (and everything generically below
        // them) inherit the flipped `true`. The top-level
        // paragraph/image sibling never fires either — the
        // partition-detection proof.
        assert_eq!(fired, vec!["paragraph", "image", "link", "text"]);
    }

    /// JSX-inside-JSX does not re-trigger anything special — the inner
    /// JSX element and everything below it simply keeps firing (`in_jsx`
    /// was already `true`).
    #[test]
    fn jsx_in_jsx_keeps_firing_without_retriggering() {
        let mut root = jsx_flow("Outer", vec![jsx_text("Inner", vec![image("/deep.png")])]);
        let mut fired = Vec::new();
        rewrite_jsx_nested(&mut root, &mut |n| fired.push(tag(n)));
        assert_eq!(fired, vec!["jsx_text", "image"]);
    }

    /// A `FootnoteDefinition` resets `in_jsx` to `false` for its subtree:
    /// an ordinary link inside a definition body — even when the
    /// definition itself is authored inside outer JSX — does not fire.
    #[test]
    fn footnote_definition_resets_in_jsx_for_ordinary_content() {
        let mut root = jsx_flow(
            "Wrapper",
            vec![footnote_def(vec![paragraph(vec![MdastNode::Link(Link {
                children: vec![text("label")],
                position: None,
                url: "#missing".to_string(),
                title: None,
            })])])],
        );
        let mut fired = Vec::new();
        rewrite_jsx_nested(&mut root, &mut |n| fired.push(tag(n)));
        // The footnote definition is entered under the WRAPPER's flipped
        // `in_jsx = true` (it's a direct child of the JSX element), so it
        // fires — the reset only applies to ITS OWN subtree. Nothing
        // inside its body fires.
        assert_eq!(fired, vec!["footnote_def"]);
    }

    /// JSX authored INSIDE a footnote-definition body re-arms `in_jsx`:
    /// the definition-internal JSX and its descendants fire again, even
    /// though the definition boundary reset the flag to `false`.
    #[test]
    fn jsx_inside_footnote_definition_body_rearms_in_jsx() {
        let mut root = footnote_def(vec![paragraph(vec![jsx_text(
            "Note",
            vec![image("/inside-def-jsx.png")],
        )])]);
        let mut fired = Vec::new();
        rewrite_jsx_nested(&mut root, &mut |n| fired.push(tag(n)));
        // The definition itself is top-level (in_jsx starts false), so
        // it does not fire, nor does the paragraph inside its body
        // (still reset). The JSX element inside the body is entered
        // under that same reset `false` and so ALSO does not fire
        // itself — but it re-arms `in_jsx` for ITS children, so the
        // image beneath it fires.
        assert_eq!(fired, vec!["image"]);
    }

    /// The callback can replace a node in place; the walk then descends
    /// into the REPLACEMENT's children, not the original's.
    #[test]
    fn callback_replacement_is_what_gets_descended_into() {
        let mut root = jsx_flow("Note", vec![image("/a.png")]);
        let mut fired = Vec::new();
        rewrite_jsx_nested(&mut root, &mut |n| {
            fired.push(tag(n));
            if let MdastNode::Image(img) = n {
                let url = img.url.clone();
                *n = jsx_text("_components.img", vec![text(&url)]);
            }
        });
        // The root JSX element doesn't fire (top-level). Its child image
        // fires and gets replaced in place. Descent then reads the
        // node's children AFTER the callback ran, so it walks into the
        // REPLACEMENT's child (the synthesized element's Text child,
        // itself not JSX) — which still fires because `in_jsx` is still
        // `true` (the replacement is still nested inside `Note`).
        assert_eq!(fired, vec!["image", "text"]);
        let MdastNode::MdxJsxFlowElement(outer) = &root else {
            panic!("expected MdxJsxFlowElement root");
        };
        assert!(matches!(
            &outer.children[0],
            MdastNode::MdxJsxTextElement(_)
        ));
    }
}
