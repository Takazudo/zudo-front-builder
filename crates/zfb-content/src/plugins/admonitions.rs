//! `:::note … :::` style admonitions.
//!
//! As of Sub #45 this module is a thin façade around
//! [`super::directives::DirectiveRegistry`]. The hardcoded 5-arm
//! match (`note`/`tip`/`warning`/`danger`/`info`/`details`) used to
//! live here; it has been replaced by a runtime-configurable directive
//! registry. This file continues to export:
//!
//! - [`AdmonitionsPlugin`] — backwards-compatible visitor that wires up
//!   the six built-in admonitions, equivalent to constructing
//!   [`DirectiveRegistry::with_defaults`] and using it as the visitor.
//!   New code should prefer the registry directly so it can register
//!   additional directives such as `:::card`, `:::badge`, etc.
//! - [`default_admonition_directives`] — the six directive definitions
//!   the registry uses.
//!
//! Rust port of zudo-doc's `remarkAdmonitions`. Detects directive-style
//! admonition blocks at the mdast level and converts them to MDX
//! [`markdown::mdast::MdxJsxFlowElement`] nodes named `Note`, `Tip`,
//! `Warning`, `Danger`, `Info`, or `Details` so downstream MDX
//! components can render them.

use markdown::mdast::Node as MdastNode;

use crate::pipeline::MdastVisitor;

use super::directives::{DirectiveDef, DirectiveKind, DirectiveRegistry};

/// The six built-in admonition directives.
///
/// Returned in the historical match order (`note`, `tip`, `warning`,
/// `danger`, `info`, `details`) so callers building a registry from
/// these get a deterministic order. `details` has
/// `title_from_label = false` because the legacy syntax is
/// `:::details title="Click me"` (unbraced attribute), not
/// `:::details[Click me]`.
#[must_use]
pub fn default_admonition_directives() -> Vec<DirectiveDef> {
    vec![
        DirectiveDef {
            name: "note".to_string(),
            kind: DirectiveKind::Container,
            component_name: "Note".to_string(),
            title_from_label: false,
        },
        DirectiveDef {
            name: "tip".to_string(),
            kind: DirectiveKind::Container,
            component_name: "Tip".to_string(),
            title_from_label: false,
        },
        DirectiveDef {
            name: "warning".to_string(),
            kind: DirectiveKind::Container,
            component_name: "Warning".to_string(),
            title_from_label: false,
        },
        DirectiveDef {
            name: "danger".to_string(),
            kind: DirectiveKind::Container,
            component_name: "Danger".to_string(),
            title_from_label: false,
        },
        DirectiveDef {
            name: "info".to_string(),
            kind: DirectiveKind::Container,
            component_name: "Info".to_string(),
            title_from_label: false,
        },
        DirectiveDef {
            name: "details".to_string(),
            kind: DirectiveKind::Container,
            component_name: "Details".to_string(),
            title_from_label: false,
        },
    ]
}

/// Visitor that converts `:::kind … :::` paragraph runs into
/// `<Kind>…</Kind>` MDX flow elements for the six built-in admonitions.
///
/// Backwards-compatible facade over
/// [`DirectiveRegistry::with_defaults`]. New code should prefer using
/// the registry directly so it can register additional directives
/// alongside the built-ins.
#[derive(Debug, Default, Clone)]
pub struct AdmonitionsPlugin {
    registry: DirectiveRegistry,
}

impl AdmonitionsPlugin {
    /// New plugin preloaded with the six built-in admonitions.
    #[must_use]
    pub fn new() -> Self {
        Self {
            registry: DirectiveRegistry::with_defaults(),
        }
    }
}

impl MdastVisitor for AdmonitionsPlugin {
    fn visit(&mut self, node: &mut MdastNode) {
        self.registry.visit(node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use markdown::mdast::{
        AttributeContent, AttributeValue, MdxJsxFlowElement, Paragraph, Root, Text,
    };

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

    fn flow(node: &MdastNode) -> &MdxJsxFlowElement {
        match node {
            MdastNode::MdxJsxFlowElement(j) => j,
            other => panic!("expected MdxJsxFlowElement, got {other:?}"),
        }
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
        let j = flow(&children[0]);
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
            let j = flow(&children[0]);
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
        let j = flow(&children[0]);
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
