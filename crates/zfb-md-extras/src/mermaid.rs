//! Replace mermaid code blocks with a renderable `<div class="mermaid">`.
//!
//! Rust port of zudo-doc's `rehypeMermaid`. Looks for
//! `<pre><code class="language-mermaid">…</code></pre>` and replaces
//! the entire `<pre>` with:
//!
//! ```html
//! <div class="mermaid" data-mermaid>{body text}</div>
//! ```
//!
//! The `<code>`'s text content is extracted recursively (using
//! [`zfb_md_ast::extract_text`]) so the unescaped diagram source — not HTML
//! entities — reaches the client-side mermaid renderer.
//!
//! Moved from `zfb-content::plugins::mermaid` in Wave 3 (#570).
//! Wire via `features.mermaid = true` in `zfb.config.ts`.

use zfb_md_ast::{extract_text, HastNode, HastVisitor};

/// Visitor that swaps mermaid `<pre>` blocks for `<div class="mermaid">`.
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
        match node {
            HastNode::Root { children } | HastNode::Element { children, .. } => {
                rewrite_children(children);
                for c in children {
                    self.visit(c);
                }
            }
            _ => {}
        }
    }
}

fn rewrite_children(children: &mut [HastNode]) {
    for child in children.iter_mut() {
        if is_mermaid_pre(child) {
            let body = extract_text(child);
            *child = HastNode::Element {
                tag: "div".to_string(),
                attrs: vec![
                    ("class".to_string(), "mermaid".to_string()),
                    // JS shape uses bare `data-mermaid` (no value); we
                    // emit it with an empty string so the serializer
                    // produces `data-mermaid=""`, matching the
                    // hast-util-to-html default for `data-mermaid: true`.
                    ("data-mermaid".to_string(), String::new()),
                ],
                children: vec![HastNode::Text(body)],
                void: false,
            };
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

    fn first_child_of_root(node: &HastNode) -> &HastNode {
        let HastNode::Root { children } = node else {
            unreachable!("expected HastNode::Root")
        };
        &children[0]
    }

    #[test]
    fn replaces_pre_with_mermaid_div() {
        let mut tree = root(vec![pre_with_lang(Some("language-mermaid"))]);
        MermaidPlugin::new().visit(&mut tree);
        let HastNode::Element {
            tag,
            attrs,
            children,
            ..
        } = first_child_of_root(&tree)
        else {
            unreachable!("expected element");
        };
        assert_eq!(tag, "div");
        assert!(attrs.contains(&("class".to_string(), "mermaid".to_string())));
        assert!(attrs.iter().any(|(k, _)| k == "data-mermaid"));
        assert_eq!(children, &[HastNode::Text("graph TD;".into())]);
    }

    #[test]
    fn ignores_non_mermaid_block() {
        let mut tree = root(vec![pre_with_lang(Some("language-rust"))]);
        let original = tree.clone();
        MermaidPlugin::new().visit(&mut tree);
        assert_eq!(tree, original);
    }

    #[test]
    fn extracts_body_text_not_html_entities() {
        let pre = HastNode::Element {
            tag: "pre".to_string(),
            attrs: vec![],
            children: vec![HastNode::Element {
                tag: "code".to_string(),
                attrs: vec![("class".to_string(), "language-mermaid".to_string())],
                children: vec![HastNode::Text("graph TD;\n  A-->B;".into())],
                void: false,
            }],
            void: false,
        };
        let mut tree = root(vec![pre]);
        MermaidPlugin::new().visit(&mut tree);
        let HastNode::Element { children, .. } = first_child_of_root(&tree) else {
            unreachable!("expected element");
        };
        let HastNode::Text(body) = &children[0] else {
            unreachable!("expected text body");
        };
        assert!(
            body.contains("A-->B"),
            "raw arrow must survive into mermaid body, got {body:?}",
        );
        assert!(!body.contains("&gt;"), "body must not be HTML-encoded");
    }

    #[test]
    fn idempotent_after_replacement() {
        let mut tree = root(vec![pre_with_lang(Some("language-mermaid"))]);
        MermaidPlugin::new().visit(&mut tree);
        let after_first = tree.clone();
        MermaidPlugin::new().visit(&mut tree);
        assert_eq!(tree, after_first);
    }
}
