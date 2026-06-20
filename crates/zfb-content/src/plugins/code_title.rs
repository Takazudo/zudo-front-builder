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
    // Scan for "title=" with a token-boundary check: the match is only
    // valid at index 0 or when the preceding byte is ASCII whitespace.
    // This prevents false hits on keys like "subtitle=" or "data-title=".
    let mut search_start = 0;
    loop {
        let Some(rel_idx) = meta[search_start..].find("title=") else {
            return meta.to_string();
        };
        let idx = search_start + rel_idx;
        let at_boundary = idx == 0 || meta.as_bytes()[idx - 1].is_ascii_whitespace();
        if at_boundary {
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
            return combined.trim().to_string();
        }
        // Not at a token boundary — advance past this occurrence and retry.
        search_start = idx + 1;
    }
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
///
/// Only accepts the match when it is at a token boundary: index 0 or
/// preceded by ASCII whitespace. This prevents false hits on keys such
/// as `subtitle=` or `data-title=`.
fn extract_title(meta: &str) -> Option<String> {
    let mut search_start = 0;
    loop {
        let rel_idx = meta[search_start..].find("title=")?;
        let idx = search_start + rel_idx;
        let at_boundary = idx == 0 || meta.as_bytes()[idx - 1].is_ascii_whitespace();
        if at_boundary {
            let rest = &meta[idx + "title=".len()..];
            let rest = rest.strip_prefix('"')?;
            // `end` is the byte offset of the closing ASCII `"` — always a
            // valid char boundary even when `rest` contains multibyte chars,
            // because `str::find` returns char-boundary offsets and `"` (0x22)
            // cannot appear as a continuation byte in UTF-8.
            let end = rest.find('"')?;
            return Some(rest[..end].to_string());
        }
        // Not at a token boundary — advance past this occurrence and retry.
        search_start = idx + 1;
    }
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

    // --- Token-boundary regression tests (fixes #728) ---

    #[test]
    fn subtitle_key_does_not_wrap() {
        // "subtitle=" contains "title=" as a substring; must NOT trigger wrap.
        let mut tree = root(vec![pre_with_meta(Some("subtitle=\"x\""))]);
        let original = tree.clone();
        CodeTitlePlugin::new().visit(&mut tree);
        assert_eq!(tree, original);
    }

    #[test]
    fn data_title_key_does_not_wrap() {
        // "data-title=" also contains "title=" as a substring; must NOT trigger wrap.
        let mut tree = root(vec![pre_with_meta(Some("data-title=\"x\""))]);
        let original = tree.clone();
        CodeTitlePlugin::new().visit(&mut tree);
        assert_eq!(tree, original);
    }

    #[test]
    fn title_after_non_title_key_extracted_correctly() {
        // The real "title=" appears after a "subtitle=" key; extraction must
        // skip the false hit and return the correct title.
        let mut tree = root(vec![pre_with_meta(Some(
            "foo=1 subtitle=\"no\" title=\"real.rs\"",
        ))]);
        CodeTitlePlugin::new().visit(&mut tree);
        let HastNode::Root { children } = tree else {
            unreachable!("expected HastNode::Root")
        };
        // Tree must have been wrapped (a real title= was present).
        let HastNode::Element { tag, children, .. } = &children[0] else {
            unreachable!("expected HastNode::Element")
        };
        assert_eq!(tag, "div");
        // First child is the title div; its text must be "real.rs", not "no".
        let HastNode::Element {
            children: title_children,
            ..
        } = &children[0]
        else {
            unreachable!("expected title div")
        };
        assert_eq!(title_children, &[HastNode::Text("real.rs".into())]);
    }

    #[test]
    fn extract_title_subtitle_returns_none() {
        assert_eq!(extract_title("subtitle=\"x\""), None);
    }

    #[test]
    fn remove_title_subtitle_unchanged() {
        assert_eq!(remove_title("subtitle=\"x\""), "subtitle=\"x\"");
    }

    // --- Multibyte title values ---

    #[test]
    fn extract_title_multibyte() {
        // CJK filename in title value: extract_title must return the full
        // multibyte string without panicking or truncating.
        assert_eq!(
            extract_title("title=\"ファイル名.rs\""),
            Some("ファイル名.rs".to_string()),
        );
    }

    #[test]
    fn remove_title_multibyte() {
        // remove_title must strip the CJK title and leave the remaining key
        // intact without corrupting the byte offsets.
        assert_eq!(remove_title("title=\"ファイル名.rs\" foo=1"), "foo=1",);
    }

    #[test]
    fn wraps_pre_with_multibyte_title() {
        // End-to-end: a <pre> with a multibyte title= value must be wrapped
        // and the title div must carry the full string.
        let mut tree = root(vec![pre_with_meta(Some("title=\"ファイル名.rs\""))]);
        CodeTitlePlugin::new().visit(&mut tree);
        let HastNode::Root { children } = tree else {
            unreachable!("expected HastNode::Root")
        };
        let HastNode::Element { tag, children, .. } = &children[0] else {
            unreachable!("expected HastNode::Element")
        };
        assert_eq!(tag, "div");
        let HastNode::Element {
            children: title_children,
            ..
        } = &children[0]
        else {
            unreachable!("expected title div")
        };
        assert_eq!(title_children, &[HastNode::Text("ファイル名.rs".into())]);
    }
}
