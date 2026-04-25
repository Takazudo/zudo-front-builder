//! Replace fenced code blocks with syntect-highlighted HTML.
//!
//! Wraps [`crate::syntect_highlight::Highlighter`] as a hast plugin.
//! For each `<pre><code data-lang="…">…</code></pre>` element (the
//! `data-lang` is set by Sub 3's mdast→hast conversion), this plugin:
//!
//! 1. Skips `mermaid` blocks — those are handled by
//!    [`crate::plugins::MermaidPlugin`].
//! 2. Calls `highlighter.highlight(code, Some(lang), None)`.
//! 3. Replaces the entire `<pre>` [`HastNode`] with
//!    [`HastNode::Raw`] containing the highlighter's output (a
//!    complete `<pre class="syntect-…"><code>…</code></pre>` HTML
//!    fragment).
//!
//! The plugin holds the highlighter behind an [`Arc`] so it can be
//! cheaply cloned across multi-document pipelines.

use std::sync::Arc;

use crate::pipeline::{HastNode, HastVisitor};
use crate::syntect_highlight::Highlighter;

/// Visitor that swaps fenced code blocks for syntect HTML.
#[derive(Clone)]
pub struct SyntectPlugin {
    highlighter: Arc<Highlighter>,
    theme: Option<String>,
}

impl SyntectPlugin {
    /// Construct with a shared highlighter.
    #[must_use]
    pub fn new(highlighter: Arc<Highlighter>) -> Self {
        Self {
            highlighter,
            theme: None,
        }
    }

    /// Override the theme passed to the highlighter (defaults to the
    /// highlighter's configured default).
    #[must_use]
    pub fn with_theme(mut self, theme: impl Into<String>) -> Self {
        self.theme = Some(theme.into());
        self
    }
}

impl HastVisitor for SyntectPlugin {
    fn visit(&mut self, node: &mut HastNode) {
        match node {
            HastNode::Root { children } | HastNode::Element { children, .. } => {
                rewrite_children(children, &self.highlighter, self.theme.as_deref());
                for c in children {
                    self.visit(c);
                }
            }
            _ => {}
        }
    }
}

fn rewrite_children(
    children: &mut [HastNode],
    highlighter: &Highlighter,
    theme: Option<&str>,
) {
    for child in children.iter_mut() {
        if let Some((lang, code)) = lang_and_code(child) {
            if lang == "mermaid" {
                continue;
            }
            if let Ok(html) = highlighter.highlight(&code, Some(&lang), theme) {
                *child = HastNode::Raw(html);
            }
        }
    }
}

/// If `node` is `<pre><code data-lang="…">TEXT</code></pre>`, return
/// `(lang, code_text)`.
fn lang_and_code(node: &HastNode) -> Option<(String, String)> {
    let HastNode::Element { tag, children, .. } = node else {
        return None;
    };
    if tag != "pre" {
        return None;
    }
    let HastNode::Element {
        tag: ctag,
        attrs,
        children: code_children,
        ..
    } = children.first()?
    else {
        return None;
    };
    if ctag != "code" {
        return None;
    }
    let lang = attrs
        .iter()
        .find(|(k, _)| k == "data-lang")
        .map(|(_, v)| v.clone())?;
    let mut code = String::new();
    for c in code_children {
        if let HastNode::Text(t) = c {
            code.push_str(t);
        }
    }
    Some((lang, code))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pre_code(lang: Option<&str>, body: &str) -> HastNode {
        let mut attrs: Vec<(String, String)> = Vec::new();
        if let Some(l) = lang {
            attrs.push(("data-lang".to_string(), l.to_string()));
        }
        HastNode::Element {
            tag: "pre".to_string(),
            attrs: vec![],
            children: vec![HastNode::Element {
                tag: "code".to_string(),
                attrs,
                children: vec![HastNode::Text(body.to_string())],
                void: false,
            }],
            void: false,
        }
    }

    #[test]
    fn highlights_rust_block() {
        let h = Arc::new(Highlighter::new());
        let mut plugin = SyntectPlugin::new(h);
        let mut tree = HastNode::Root {
            children: vec![pre_code(Some("rust"), "fn main() {}\n")],
        };
        plugin.visit(&mut tree);
        let HastNode::Root { children } = tree else {
            panic!()
        };
        let HastNode::Raw(html) = &children[0] else {
            panic!("expected Raw, got {:?}", children[0])
        };
        assert!(html.contains("<pre class=\"syntect-"), "got: {html}");
        assert!(html.contains("</code></pre>"));
    }

    #[test]
    fn skips_mermaid_block() {
        let h = Arc::new(Highlighter::new());
        let mut plugin = SyntectPlugin::new(h);
        let mut tree = HastNode::Root {
            children: vec![pre_code(Some("mermaid"), "graph TD;")],
        };
        let original = tree.clone();
        plugin.visit(&mut tree);
        assert_eq!(tree, original, "mermaid blocks must be untouched");
    }

    #[test]
    fn skips_block_without_data_lang() {
        let h = Arc::new(Highlighter::new());
        let mut plugin = SyntectPlugin::new(h);
        let mut tree = HastNode::Root {
            children: vec![pre_code(None, "anything")],
        };
        let original = tree.clone();
        plugin.visit(&mut tree);
        assert_eq!(tree, original);
    }

    #[test]
    fn unknown_language_falls_back_to_pre_code() {
        let h = Arc::new(Highlighter::new());
        let mut plugin = SyntectPlugin::new(h);
        let mut tree = HastNode::Root {
            children: vec![pre_code(Some("klingon"), "hello")],
        };
        plugin.visit(&mut tree);
        let HastNode::Root { children } = tree else {
            panic!()
        };
        let HastNode::Raw(html) = &children[0] else {
            panic!()
        };
        assert_eq!(html, "<pre><code>hello</code></pre>");
    }
}
