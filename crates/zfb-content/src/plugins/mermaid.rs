//! Mark mermaid code blocks for client-side rendering.
//!
//! Rust port of zudo-doc's `rehypeMermaid`. Looks for
//! `<pre><code class="language-mermaid">…</code></pre>` and adds a
//! `data-mermaid="true"` attribute to the `<pre>` so client-side JS
//! can find and render it. We deliberately do NOT replace the body
//! (that's the client's job), and we do NOT add a `data-mermaid`
//! attribute that already exists (idempotent).

use crate::pipeline::{HastNode, HastVisitor};

/// Visitor that flags mermaid `<pre>` blocks.
#[derive(Debug, Default, Clone, Copy)]
pub struct MermaidPlugin;

impl MermaidPlugin {
    /// New plugin (stateless).
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl HastVisitor for MermaidPlugin {
    fn visit(&mut self, node: &mut HastNode) {
        if is_mermaid_pre(node) {
            if let HastNode::Element { attrs, .. } = node {
                if !attrs.iter().any(|(k, _)| k == "data-mermaid") {
                    attrs.push(("data-mermaid".to_string(), "true".to_string()));
                }
            }
            // Don't recurse into a flagged mermaid block; its body is opaque.
            return;
        }
        match node {
            HastNode::Root { children } | HastNode::Element { children, .. } => {
                for c in children {
                    self.visit(c);
                }
            }
            _ => {}
        }
    }
}

fn is_mermaid_pre(node: &HastNode) -> bool {
    let HastNode::Element { tag, children, .. } = node else {
        return false;
    };
    if tag != "pre" {
        return false;
    }
    let Some(HastNode::Element {
        tag: ctag, attrs, ..
    }) = children.first()
    else {
        return false;
    };
    if ctag != "code" {
        return false;
    }
    attrs
        .iter()
        .any(|(k, v)| k == "class" && class_contains(v, "language-mermaid"))
}

fn class_contains(value: &str, class: &str) -> bool {
    value.split_ascii_whitespace().any(|c| c == class)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pre_with_lang(class: Option<&str>) -> HastNode {
        let mut code_attrs: Vec<(String, String)> = Vec::new();
        if let Some(c) = class {
            code_attrs.push(("class".to_string(), c.to_string()));
        }
        HastNode::Element {
            tag: "pre".to_string(),
            attrs: vec![],
            children: vec![HastNode::Element {
                tag: "code".to_string(),
                attrs: code_attrs,
                children: vec![HastNode::Text("graph TD;".into())],
                void: false,
            }],
            void: false,
        }
    }

    fn root(children: Vec<HastNode>) -> HastNode {
        HastNode::Root { children }
    }

    fn pre_attr(node: &HastNode, key: &str) -> Option<String> {
        let HastNode::Root { children } = node else {
            return None;
        };
        let HastNode::Element { attrs, .. } = &children[0] else {
            return None;
        };
        attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    #[test]
    fn flags_mermaid_block() {
        let mut tree = root(vec![pre_with_lang(Some("language-mermaid"))]);
        MermaidPlugin::new().visit(&mut tree);
        assert_eq!(pre_attr(&tree, "data-mermaid"), Some("true".to_string()));
    }

    #[test]
    fn ignores_non_mermaid_block() {
        let mut tree = root(vec![pre_with_lang(Some("language-rust"))]);
        MermaidPlugin::new().visit(&mut tree);
        assert_eq!(pre_attr(&tree, "data-mermaid"), None);
    }

    #[test]
    fn does_not_replace_content() {
        let mut tree = root(vec![pre_with_lang(Some("language-mermaid"))]);
        MermaidPlugin::new().visit(&mut tree);
        let HastNode::Root { children } = tree else {
            unreachable!("expected HastNode::Root")
        };
        let HastNode::Element { children: pc, .. } = &children[0] else {
            unreachable!("expected HastNode::Element")
        };
        let HastNode::Element { children: cc, .. } = &pc[0] else {
            unreachable!("expected HastNode::Element")
        };
        assert_eq!(cc, &[HastNode::Text("graph TD;".into())]);
    }

    #[test]
    fn idempotent() {
        let mut tree = root(vec![pre_with_lang(Some("language-mermaid"))]);
        MermaidPlugin::new().visit(&mut tree);
        MermaidPlugin::new().visit(&mut tree);
        let HastNode::Root { children } = &tree else {
            unreachable!("expected HastNode::Root")
        };
        let HastNode::Element { attrs, .. } = &children[0] else {
            unreachable!("expected HastNode::Element")
        };
        let count = attrs.iter().filter(|(k, _)| k == "data-mermaid").count();
        assert_eq!(count, 1);
    }
}
