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
/// these get a deterministic order. All six have `title_from_label =
/// true` so `:::note[Custom Title]` promotes the bracketed label to a
/// `title="…"` attribute — matching the behaviour of Docusaurus and
/// Starlight. `:::details` continues to accept the legacy unbraced form
/// (`:::details title="Click me"`) since attribute parsing is additive:
/// the label path and the unbraced-attr path both set `title`, and the
/// unbraced form writes directly to `attrs` (not `label`), so both
/// co-exist without conflict. Consumers that want the old label-as-child
/// behaviour can register their own `DirectiveDef` with
/// `title_from_label: false`.
#[must_use]
pub fn default_admonition_directives() -> Vec<DirectiveDef> {
    vec![
        DirectiveDef {
            name: "note".to_string(),
            kind: DirectiveKind::Container,
            component_name: "Note".to_string(),
            title_from_label: true,
            attrs: Vec::new(),
        },
        DirectiveDef {
            name: "tip".to_string(),
            kind: DirectiveKind::Container,
            component_name: "Tip".to_string(),
            title_from_label: true,
            attrs: Vec::new(),
        },
        DirectiveDef {
            name: "warning".to_string(),
            kind: DirectiveKind::Container,
            component_name: "Warning".to_string(),
            title_from_label: true,
            attrs: Vec::new(),
        },
        DirectiveDef {
            name: "danger".to_string(),
            kind: DirectiveKind::Container,
            component_name: "Danger".to_string(),
            title_from_label: true,
            attrs: Vec::new(),
        },
        DirectiveDef {
            name: "info".to_string(),
            kind: DirectiveKind::Container,
            component_name: "Info".to_string(),
            title_from_label: true,
            attrs: Vec::new(),
        },
        DirectiveDef {
            name: "details".to_string(),
            kind: DirectiveKind::Container,
            component_name: "Details".to_string(),
            title_from_label: true,
            attrs: Vec::new(),
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
///
/// ## Blank-line requirement
///
/// Each fence line (`:::note[…]` and the closing `:::`) **must** be
/// separated from surrounding content by blank lines so the markdown
/// parser treats them as separate paragraphs. Without the blank lines
/// both fences and the body collapse into a single paragraph and the
/// admonition is not recognised. [`DirectiveRegistry`] emits a
/// [`super::directives::DirectiveDiagnostic`] at content-processing
/// time when it detects this pattern; the build orchestrator prints it
/// as a warning.
///
/// Correct:
/// ```markdown
/// :::note[Title]
///
/// Body text.
///
/// :::
/// ```
///
/// Incorrect (no blank lines — emits diagnostic):
/// ```markdown
/// :::note
/// Body text.
/// :::
/// ```
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
            other => unreachable!("expected MdxJsxFlowElement, got {other:?}"),
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
            unreachable!("expected MdastNode::Root")
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
                unreachable!("expected MdastNode::Root")
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
            unreachable!("expected MdastNode::Root")
        };
        let j = flow(&children[0]);
        assert_eq!(j.attributes.len(), 1);
        let AttributeContent::Property(prop) = &j.attributes[0] else {
            unreachable!("expected AttributeContent::Property")
        };
        assert_eq!(prop.name, "title");
        match &prop.value {
            Some(AttributeValue::Literal(v)) => assert_eq!(v, "Click me"),
            other => unreachable!("expected literal, got {other:?}"),
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
            unreachable!("expected MdastNode::Root")
        };
        // No transformation: 3 paragraphs preserved.
        assert_eq!(children.len(), 3);
    }

    #[test]
    fn missing_close_left_alone() {
        let root = run_root(vec![text_para(":::note"), text_para("body")]);
        let MdastNode::Root(Root { children, .. }) = root else {
            unreachable!("expected MdastNode::Root")
        };
        assert_eq!(children.len(), 2);
        assert!(matches!(children[0], MdastNode::Paragraph(_)));
    }

    // ---- title_from_label (Sub #135) ----

    #[test]
    fn note_with_label_promotes_to_title_attr() {
        // :::note[Custom Title] should produce title="Custom Title" on
        // the emitted JSX element.
        let root = run_root(vec![
            text_para(":::note[Custom Title]"),
            text_para("body"),
            text_para(":::"),
        ]);
        let MdastNode::Root(Root { children, .. }) = root else {
            unreachable!("expected MdastNode::Root")
        };
        assert_eq!(children.len(), 1);
        let j = flow(&children[0]);
        assert_eq!(j.name.as_deref(), Some("Note"));
        let title_attr = j.attributes.iter().find_map(|a| {
            if let AttributeContent::Property(p) = a {
                if p.name == "title" {
                    if let Some(AttributeValue::Literal(v)) = &p.value {
                        return Some(v.clone());
                    }
                }
            }
            None
        });
        assert_eq!(
            title_attr.as_deref(),
            Some("Custom Title"),
            "label promoted to title attribute"
        );
    }

    #[test]
    fn all_kinds_accept_label_as_title() {
        // Verify all six built-in admonitions promote [label] → title="…".
        for (key, tag) in [
            ("note", "Note"),
            ("tip", "Tip"),
            ("warning", "Warning"),
            ("danger", "Danger"),
            ("info", "Info"),
            ("details", "Details"),
        ] {
            let root = run_root(vec![
                text_para(&format!(":::{key}[My Label]")),
                text_para("body"),
                text_para(":::"),
            ]);
            let MdastNode::Root(Root { children, .. }) = root else {
                unreachable!("expected MdastNode::Root")
            };
            assert_eq!(children.len(), 1, "single JSX element for {key}");
            let j = flow(&children[0]);
            assert_eq!(j.name.as_deref(), Some(tag), "component name for {key}");
            let title_attr = j.attributes.iter().find_map(|a| {
                if let AttributeContent::Property(p) = a {
                    if p.name == "title" {
                        if let Some(AttributeValue::Literal(v)) = &p.value {
                            return Some(v.clone());
                        }
                    }
                }
                None
            });
            assert_eq!(
                title_attr.as_deref(),
                Some("My Label"),
                "title attr for {key}"
            );
        }
    }
}
