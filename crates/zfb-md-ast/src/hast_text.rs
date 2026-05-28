//! Recursively extract text content from a hast tree.
//!
//! Equivalent to the unified ecosystem's `hast-util-to-string` /
//! `extractText` helper used by zudo-doc's md-plugins. We concatenate
//! every [`HastNode::Text`] payload encountered in document order, and
//! ignore [`HastNode::Raw`] / [`HastNode::JsxRaw`] / [`HastNode::Comment`]
//! (so JSX passthrough, raw HTML, and HTML comments do not bleed into
//! slugs / aria labels).
//!
//! Moved from `zfb-content::plugins::util::hast_text` to `zfb-md-ast` so
//! hast visitors in `zfb-md-extras` (which cannot depend on `zfb-content`)
//! can use it. `zfb-content` re-exports it from its original module path for
//! backwards compatibility.

use crate::HastNode;

/// Concatenate every [`HastNode::Text`] reachable from `node`.
///
/// Walks `Root` and `Element` children depth-first; treats `Raw` and
/// `Comment` as opaque (skipped).
#[must_use]
pub fn extract_text(node: &HastNode) -> String {
    let mut out = String::new();
    walk(node, &mut out);
    out
}

fn walk(node: &HastNode, out: &mut String) {
    match node {
        HastNode::Text(s) => out.push_str(s),
        HastNode::Root { children } => {
            for c in children {
                walk(c, out);
            }
        }
        HastNode::Element { children, .. } => {
            for c in children {
                walk(c, out);
            }
        }
        HastNode::Raw(_) | HastNode::JsxRaw(_) | HastNode::Comment(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(tag: &str, children: Vec<HastNode>) -> HastNode {
        HastNode::Element {
            tag: tag.to_string(),
            attrs: vec![],
            children,
            void: false,
        }
    }

    #[test]
    fn extracts_plain_text() {
        let n = HastNode::Text("hello".to_string());
        assert_eq!(extract_text(&n), "hello");
    }

    #[test]
    fn concatenates_nested_elements_in_order() {
        let n = el(
            "p",
            vec![
                HastNode::Text("a ".into()),
                el("em", vec![HastNode::Text("b ".into())]),
                HastNode::Text("c".into()),
            ],
        );
        assert_eq!(extract_text(&n), "a b c");
    }

    #[test]
    fn ignores_raw_and_comments() {
        let n = el(
            "p",
            vec![
                HastNode::Text("hi ".into()),
                HastNode::Raw("<Note>raw</Note>".into()),
                HastNode::Comment(" hidden ".into()),
                HastNode::Text("end".into()),
            ],
        );
        assert_eq!(extract_text(&n), "hi end");
    }

    #[test]
    fn walks_root() {
        let n = HastNode::Root {
            children: vec![
                el("h1", vec![HastNode::Text("Title".into())]),
                el("p", vec![HastNode::Text(" body".into())]),
            ],
        };
        assert_eq!(extract_text(&n), "Title body");
    }
}
