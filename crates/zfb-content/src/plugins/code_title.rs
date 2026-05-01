//! Wrap titled fenced code blocks in a titled container `<div>`.
//!
//! Rust port of zudo-doc's `rehypeCodeTitle`. Looks for
//! `<pre><code data-meta="…">…</code></pre>` produced by Sub 3's
//! mdast→hast conversion. If `data-meta` contains `title="..."`, the
//! `<pre>` is wrapped in:
//!
//! ```html
//! <div class="code-block-container">
//!   <div class="code-block-title">{title}</div>
//!   <pre>…</pre>
//! </div>
//! ```
//!
//! No regex dependency: the meta string is scanned with simple
//! `find` calls.

use crate::pipeline::{HastNode, HastVisitor};

/// Visitor that wraps titled code blocks in `<figure>`.
#[derive(Debug, Default, Clone, Copy)]
pub struct CodeTitlePlugin;

impl CodeTitlePlugin {
    /// New plugin (stateless).
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl HastVisitor for CodeTitlePlugin {
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
    let mut i = 0;
    while i < children.len() {
        if let Some(title) = title_for_pre(&children[i]) {
            let mut pre = std::mem::replace(&mut children[i], HastNode::Text(String::new()));
            // Strip the `title="…"` segment from the inner code's
            // `data-meta` so a second pass over this tree (or a
            // child-recursion descent into the new <figure>) does not
            // re-wrap.
            strip_title_from_pre(&mut pre);
            let container = HastNode::Element {
                tag: "div".to_string(),
                attrs: vec![("class".to_string(), "code-block-container".to_string())],
                children: vec![
                    HastNode::Element {
                        tag: "div".to_string(),
                        attrs: vec![("class".to_string(), "code-block-title".to_string())],
                        children: vec![HastNode::Text(title)],
                        void: false,
                    },
                    pre,
                ],
                void: false,
            };
            children[i] = container;
        }
        i += 1;
    }
}

/// Remove the `title="…"` portion from the inner `<code>`'s
/// `data-meta` attribute so this `<pre>` is no longer detected as
/// having a title.
fn strip_title_from_pre(pre: &mut HastNode) {
    let HastNode::Element { children, .. } = pre else {
        return;
    };
    let Some(HastNode::Element { attrs, .. }) = children.first_mut() else {
        return;
    };
    for (k, v) in attrs.iter_mut() {
        if k == "data-meta" {
            *v = remove_title(v);
        }
    }
    // Drop empty data-meta entries so downstream plugins don't see
    // empty noise.
    attrs.retain(|(k, v)| !(k == "data-meta" && v.is_empty()));
}

fn remove_title(meta: &str) -> String {
    let Some(idx) = meta.find("title=") else {
        return meta.to_string();
    };
    let before = &meta[..idx];
    let rest = &meta[idx + "title=".len()..];
    let after = if let Some(stripped) = rest.strip_prefix('"') {
        match stripped.find('"') {
            Some(end) => &stripped[end + 1..],
            None => "",
        }
    } else {
        // No quoted value — drop until next whitespace.
        match rest.find(char::is_whitespace) {
            Some(end) => &rest[end..],
            None => "",
        }
    };
    let combined = format!("{}{}", before.trim_end(), after);
    combined.trim().to_string()
}

/// If `node` is `<pre>` whose first child is `<code>` with a
/// `data-meta="title=..."` attribute, return the captured title.
fn title_for_pre(node: &HastNode) -> Option<String> {
    let HastNode::Element { tag, children, .. } = node else {
        return None;
    };
    if tag != "pre" {
        return None;
    }
    let HastNode::Element {
        tag: ctag, attrs, ..
    } = children.first()?
    else {
        return None;
    };
    if ctag != "code" {
        return None;
    }
    for (k, v) in attrs {
        if k == "data-meta" {
            return extract_title(v);
        }
    }
    None
}

/// Pick `title="..."` out of a meta string like
/// `title="main.rs" foo=1`.
fn extract_title(meta: &str) -> Option<String> {
    let idx = meta.find("title=")?;
    let rest = &meta[idx + "title=".len()..];
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pre_with_meta(meta: Option<&str>) -> HastNode {
        let mut attrs: Vec<(String, String)> = Vec::new();
        if let Some(m) = meta {
            attrs.push(("data-meta".to_string(), m.to_string()));
        }
        HastNode::Element {
            tag: "pre".to_string(),
            attrs: vec![],
            children: vec![HastNode::Element {
                tag: "code".to_string(),
                attrs,
                children: vec![HastNode::Text("body".into())],
                void: false,
            }],
            void: false,
        }
    }

    fn root(children: Vec<HastNode>) -> HastNode {
        HastNode::Root { children }
    }

    #[test]
    fn wraps_pre_with_title() {
        let mut tree = root(vec![pre_with_meta(Some("title=\"main.rs\""))]);
        CodeTitlePlugin::new().visit(&mut tree);
        let HastNode::Root { children } = tree else {
            unreachable!("expected HastNode::Root")
        };
        let HastNode::Element {
            tag,
            attrs,
            children,
            ..
        } = &children[0]
        else {
            unreachable!("unreachable in test")
        };
        assert_eq!(tag, "div");
        assert!(attrs.contains(&("class".to_string(), "code-block-container".to_string())));
        let HastNode::Element {
            tag: title_tag,
            attrs: title_attrs,
            children: title_children,
            ..
        } = &children[0]
        else {
            unreachable!("unreachable in test")
        };
        assert_eq!(title_tag, "div");
        assert!(title_attrs.contains(&("class".to_string(), "code-block-title".to_string())));
        assert_eq!(title_children, &[HastNode::Text("main.rs".into())]);
        let HastNode::Element { tag: pre_tag, .. } = &children[1] else {
            unreachable!("expected HastNode::Element")
        };
        assert_eq!(pre_tag, "pre");
    }

    #[test]
    fn no_title_no_wrap() {
        let mut tree = root(vec![pre_with_meta(Some("foo=bar"))]);
        let original = tree.clone();
        CodeTitlePlugin::new().visit(&mut tree);
        assert_eq!(tree, original);
    }

    #[test]
    fn no_meta_no_wrap() {
        let mut tree = root(vec![pre_with_meta(None)]);
        let original = tree.clone();
        CodeTitlePlugin::new().visit(&mut tree);
        assert_eq!(tree, original);
    }

    #[test]
    fn handles_extra_meta_keys() {
        let mut tree = root(vec![pre_with_meta(Some("foo=1 title=\"a.rs\" bar=2"))]);
        CodeTitlePlugin::new().visit(&mut tree);
        let HastNode::Root { children } = tree else {
            unreachable!("expected HastNode::Root")
        };
        let HastNode::Element { tag, attrs, .. } = &children[0] else {
            unreachable!("expected HastNode::Element")
        };
        assert_eq!(tag, "div");
        assert!(attrs.contains(&("class".to_string(), "code-block-container".to_string())));
    }
}
