//! Add `id` + permalink anchor to `<h2>`–`<h6>` elements.
//!
//! Rust port of zudo-doc's `rehypeHeadingLinks`. Walks the hast tree
//! and, for each `<h2>` … `<h6>`:
//!
//! - Generates a slug from the heading's text content using a
//!   github-slugger-equivalent algorithm (lowercase, non-alphanumerics
//!   collapsed to `-`, leading/trailing `-` stripped).
//! - Deduplicates slugs within one document by appending `-1`, `-2`, …
//!   to repeated slugs (matching `github-slugger` behaviour).
//! - Adds `id="<slug>"` to the heading element (or replaces an
//!   existing `id` attribute).
//! - Prepends an anchor element as the first child:
//!   `<a href="#<slug>" class="heading-anchor"
//!   aria-label="Permalink to this heading">#</a>`.
//!
//! `<h1>` is intentionally left alone — page titles are typically
//! emitted by the layout, not the markdown body.

use std::collections::HashMap;

use crate::pipeline::{HastNode, HastVisitor};
use crate::plugins::util::hast_text::extract_text;

/// Visitor that adds permalink anchors to headings.
pub struct HeadingLinksPlugin {
    seen: HashMap<String, usize>,
}

impl HeadingLinksPlugin {
    /// New plugin with an empty per-document slug counter.
    #[must_use]
    pub fn new() -> Self {
        Self {
            seen: HashMap::new(),
        }
    }

    fn next_slug(&mut self, base: &str) -> String {
        let count = self.seen.entry(base.to_string()).or_insert(0);
        let slug = if *count == 0 {
            base.to_string()
        } else {
            format!("{base}-{count}")
        };
        *count += 1;
        slug
    }
}

impl Default for HeadingLinksPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl HastVisitor for HeadingLinksPlugin {
    fn visit(&mut self, node: &mut HastNode) {
        // Compute slug first (immutable borrow only).
        let mut slug_to_apply: Option<String> = None;
        if let HastNode::Element { tag, .. } = node {
            if is_target_heading(tag) {
                let text = extract_text(node);
                let base = slugify(&text);
                if !base.is_empty() {
                    slug_to_apply = Some(self.next_slug(&base));
                }
            }
        }
        if let Some(slug) = slug_to_apply {
            if let HastNode::Element {
                attrs, children, ..
            } = node
            {
                set_attr(attrs, "id", &slug);
                children.insert(0, anchor(&slug));
            }
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

fn is_target_heading(tag: &str) -> bool {
    matches!(tag, "h2" | "h3" | "h4" | "h5" | "h6")
}

fn anchor(slug: &str) -> HastNode {
    HastNode::Element {
        tag: "a".to_string(),
        attrs: vec![
            ("href".to_string(), format!("#{slug}")),
            ("class".to_string(), "heading-anchor".to_string()),
            (
                "aria-label".to_string(),
                "Permalink to this heading".to_string(),
            ),
        ],
        children: vec![HastNode::Text("#".to_string())],
        void: false,
    }
}

fn set_attr(attrs: &mut Vec<(String, String)>, key: &str, val: &str) {
    for (k, v) in attrs.iter_mut() {
        if k == key {
            *v = val.to_string();
            return;
        }
    }
    attrs.push((key.to_string(), val.to_string()));
}

/// github-slugger-equivalent slugifier.
///
/// Lowercase, non-alphanumeric runs collapse to a single `-`, leading
/// and trailing `-` stripped. Whitespace and punctuation behave the
/// same way (single `-` between words). ASCII-only — non-ASCII
/// alphanumerics are dropped, mirroring github-slugger's regex.
#[must_use]
pub fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_dash = true;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            for c in ch.to_lowercase() {
                out.push(c);
            }
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    if out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(level: u8, text: &str) -> HastNode {
        HastNode::Element {
            tag: format!("h{level}"),
            attrs: vec![],
            children: vec![HastNode::Text(text.to_string())],
            void: false,
        }
    }

    fn root(children: Vec<HastNode>) -> HastNode {
        HastNode::Root { children }
    }

    fn first_attr<'a>(node: &'a HastNode, key: &str) -> Option<&'a str> {
        let HastNode::Element { attrs, .. } = node else {
            return None;
        };
        attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    #[test]
    fn adds_id_and_anchor_to_h2() {
        let mut tree = root(vec![h(2, "Hello World")]);
        HeadingLinksPlugin::new().visit(&mut tree);
        let HastNode::Root { children } = tree else {
            panic!()
        };
        assert_eq!(first_attr(&children[0], "id"), Some("hello-world"));

        let HastNode::Element {
            children: hc,
            ..
        } = &children[0]
        else {
            panic!()
        };
        let HastNode::Element {
            tag, attrs, ..
        } = &hc[0]
        else {
            panic!()
        };
        assert_eq!(tag, "a");
        assert!(attrs.contains(&("href".to_string(), "#hello-world".to_string())));
        assert!(attrs.contains(&("class".to_string(), "heading-anchor".to_string())));
    }

    #[test]
    fn h1_is_left_alone() {
        let mut tree = root(vec![h(1, "Page Title")]);
        let before = tree.clone();
        HeadingLinksPlugin::new().visit(&mut tree);
        assert_eq!(tree, before);
    }

    #[test]
    fn duplicate_slugs_are_numbered() {
        let mut tree = root(vec![h(2, "A"), h(2, "A"), h(2, "A")]);
        HeadingLinksPlugin::new().visit(&mut tree);
        let HastNode::Root { children } = tree else {
            panic!()
        };
        assert_eq!(first_attr(&children[0], "id"), Some("a"));
        assert_eq!(first_attr(&children[1], "id"), Some("a-1"));
        assert_eq!(first_attr(&children[2], "id"), Some("a-2"));
    }

    #[test]
    fn slugify_handles_punctuation_and_case() {
        assert_eq!(slugify("Hello, World!"), "hello-world");
        assert_eq!(slugify("  --weird-- "), "weird");
        assert_eq!(slugify("MixedCase 123"), "mixedcase-123");
    }

    #[test]
    fn empty_text_heading_skipped() {
        let mut tree = root(vec![h(3, "")]);
        let before = tree.clone();
        HeadingLinksPlugin::new().visit(&mut tree);
        assert_eq!(tree, before, "empty text → no slug → no mutation");
    }
}
