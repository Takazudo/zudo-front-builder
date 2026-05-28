//! `:::note … :::` style admonitions.
//!
//! As of Sub #45 this module is a thin façade around
//! [`super::directives::DirectiveRegistry`]. The hardcoded 5-arm
//! match (`note`/`tip`/`warning`/`danger`/`info`/`details`) used to
//! live here; it has been replaced by a runtime-configurable directive
//! registry. This file exports:
//!
//! - [`AdmonitionsPlugin`] — backwards-compatible visitor that wires up
//!   the six built-in admonitions, equivalent to constructing
//!   [`DirectiveRegistry::with_defaults`] and using it as the visitor.
//!   New code should prefer the registry directly so it can register
//!   additional directives such as `:::card`, `:::badge`, etc.
//! - [`default_admonition_directives`] — thin delegation to
//!   `zfb_md_extras::admonitions_preset::default_admonition_directives`
//!   (moved there in Wave 3 #570 so `zfb-md-extras` can produce the
//!   preset list independently of `zfb-content`).
//!
//! Rust port of zudo-doc's `remarkAdmonitions`. Detects directive-style
//! admonition blocks at the mdast level and converts them to MDX
//! [`markdown::mdast::MdxJsxFlowElement`] nodes named `Note`, `Tip`,
//! `Warning`, `Danger`, `Info`, or `Details` so downstream MDX
//! components can render them.

use markdown::mdast::Node as MdastNode;

use crate::pipeline::MdastVisitor;

use super::directives::{DirectiveDef, DirectiveRegistry};

/// The six built-in admonition directives.
///
/// Delegates to [`zfb_md_extras::admonitions_preset::default_admonition_directives`]
/// which owns the canonical definition since Wave 3 (#570). Kept here for
/// backwards compatibility with callers that import from this module path.
#[must_use]
pub fn default_admonition_directives() -> Vec<DirectiveDef> {
    zfb_md_extras::admonitions_preset::default_admonition_directives()
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
