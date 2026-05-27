//! Replace fenced code blocks with syntect-highlighted structured HAST.
//!
//! Wraps [`crate::syntect_highlight::Highlighter`] as a hast plugin.
//! For each `<pre><code data-lang="…">…</code></pre>` element (the
//! `data-lang` is set by Sub 3's mdast→hast conversion), this plugin:
//!
//! 1. Skips `mermaid` blocks — those are handled by
//!    [`crate::plugins::MermaidPlugin`].
//! 2. Calls `highlighter.highlight_lines(code, Some(lang), None)`.
//! 3. Replaces the entire `<pre>` [`HastNode`] with a structured HAST
//!    tree of the form:
//!    ```text
//!    Element<pre class="syntect-{slug}">
//!      Element<code>
//!        Element<span class="line">  Raw(line_1_html)  </span>
//!        Element<span class="line">  Raw(line_2_html)  </span>
//!        …
//!    ```
//!    Each `<span class="line">` is an independent [`HastNode::Element`]
//!    that downstream visitors (e.g. wave-5 code-enrichment) can mutate
//!    to add diff markers or line-highlight classes.
//!
//! The per-line content (colored token spans) is kept as
//! [`HastNode::Raw`] inside each `<span class="line">` so the
//! serialized HTML output is byte-identical to what a flat concatenation
//! of the same token spans would produce — the only new bytes are the
//! `<span class="line">` / `</span>` wrappers.
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

fn rewrite_children(children: &mut [HastNode], highlighter: &Highlighter, theme: Option<&str>) {
    for child in children.iter_mut() {
        if let Some((lang, code)) = lang_and_code(child) {
            if lang == "mermaid" {
                continue;
            }
            if let Ok(result) = highlighter.highlight_lines(&code, Some(&lang), theme) {
                // Build structured HAST: <pre class="syntect-{slug}"><code>
                //   <span class="line">Raw(line_html)</span>…
                // </code></pre>
                // The per-line Raw keeps token-span HTML verbatim; the
                // <span class="line"> Element wrapper exposes each line to
                // downstream visitors (wave-5 code-enrichment).
                let line_spans: Vec<HastNode> = result
                    .lines
                    .into_iter()
                    .map(|line_html| HastNode::Element {
                        tag: "span".to_string(),
                        attrs: vec![("class".to_string(), "line".to_string())],
                        children: vec![HastNode::Raw(line_html)],
                        void: false,
                    })
                    .collect();
                let code_el = HastNode::Element {
                    tag: "code".to_string(),
                    attrs: vec![],
                    children: line_spans,
                    void: false,
                };
                *child = HastNode::Element {
                    tag: "pre".to_string(),
                    attrs: vec![(
                        "class".to_string(),
                        format!("syntect-{}", result.theme_slug),
                    )],
                    children: vec![code_el],
                    void: false,
                };
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

    /// Walk a structured syntect HAST node and assert the expected shape:
    /// `Element<pre class="syntect-…">` → `Element<code>` → N
    /// `Element<span class="line">` each containing `Raw(…)`.
    ///
    /// Returns the serialized HTML so callers can make content assertions.
    fn assert_syntect_structure(node: &HastNode) -> String {
        use crate::serializer::serialize;

        let HastNode::Element {
            tag: pre_tag,
            attrs: pre_attrs,
            children: pre_children,
            ..
        } = node
        else {
            panic!("expected Element<pre>, got {node:?}");
        };
        assert_eq!(pre_tag, "pre", "outer tag must be pre");
        assert!(
            pre_attrs
                .iter()
                .any(|(k, v)| k == "class" && v.starts_with("syntect-")),
            "pre must have class=\"syntect-…\": {pre_attrs:?}"
        );
        let code_el = pre_children
            .first()
            .expect("pre must have a child <code>");
        let HastNode::Element {
            tag: code_tag,
            children: code_children,
            ..
        } = code_el
        else {
            panic!("expected Element<code> inside pre, got {code_el:?}");
        };
        assert_eq!(code_tag, "code", "inner tag must be code");
        for (i, span) in code_children.iter().enumerate() {
            let HastNode::Element {
                tag: span_tag,
                attrs: span_attrs,
                children: span_children,
                ..
            } = span
            else {
                panic!("line {i}: expected Element<span>, got {span:?}");
            };
            assert_eq!(span_tag, "span", "line {i}: span tag must be span");
            assert!(
                span_attrs
                    .iter()
                    .any(|(k, v)| k == "class" && v == "line"),
                "line {i}: span must have class=\"line\": {span_attrs:?}"
            );
            assert_eq!(
                span_children.len(),
                1,
                "line {i}: span must have exactly one child (Raw)"
            );
            assert!(
                matches!(span_children[0], HastNode::Raw(_)),
                "line {i}: span child must be Raw, got {:?}",
                span_children[0]
            );
        }
        serialize(node)
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
            unreachable!("expected HastNode::Root")
        };
        // The plugin now emits a structured Element tree, not a Raw blob.
        let html = assert_syntect_structure(&children[0]);
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
            unreachable!("expected HastNode::Root")
        };
        // Even on the fallback path the plugin emits structured HAST.
        let html = assert_syntect_structure(&children[0]);
        // Unknown lang now produces a themed wrapper instead of bare <pre><code>
        assert!(
            html.starts_with("<pre class=\"syntect-"),
            "expected themed wrapper for unknown lang: {html}"
        );
        assert!(html.contains("hello"), "code content missing: {html}");
    }

    /// New acceptance criterion (issue #571): a 3-line `<pre>` produces
    /// 3 `<span class="line">` Element nodes inside `<code>` that a
    /// downstream visitor could mutate independently.
    #[test]
    fn three_line_block_produces_three_span_line_elements() {
        let h = Arc::new(Highlighter::new());
        let mut plugin = SyntectPlugin::new(h);
        let code = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let mut tree = HastNode::Root {
            children: vec![pre_code(Some("rust"), code)],
        };
        plugin.visit(&mut tree);

        let HastNode::Root { children } = &tree else {
            unreachable!("expected Root")
        };
        let HastNode::Element {
            tag,
            children: pre_children,
            ..
        } = &children[0]
        else {
            panic!("expected Element<pre>, got {:?}", children[0])
        };
        assert_eq!(tag, "pre");
        let HastNode::Element {
            tag: code_tag,
            children: code_children,
            ..
        } = pre_children.first().expect("pre must have <code>")
        else {
            panic!("expected Element<code>")
        };
        assert_eq!(code_tag, "code");
        // 3 source lines → 3 <span class="line"> elements.
        assert_eq!(
            code_children.len(),
            3,
            "expected 3 line spans, got {}: {code_children:?}",
            code_children.len()
        );
        for (i, span) in code_children.iter().enumerate() {
            let HastNode::Element { tag, attrs, children: span_children, .. } = span else {
                panic!("line {i}: expected Element<span>, got {span:?}");
            };
            assert_eq!(tag, "span", "line {i}: wrong tag");
            assert!(
                attrs.iter().any(|(k, v)| k == "class" && v == "line"),
                "line {i}: missing class=\"line\": {attrs:?}"
            );
            // Each span must have exactly one Raw child (the token HTML).
            assert_eq!(span_children.len(), 1, "line {i}: expected 1 child");
            assert!(
                matches!(span_children[0], HastNode::Raw(_)),
                "line {i}: child must be Raw, got {:?}",
                span_children[0]
            );
        }
    }
}
