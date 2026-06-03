//! GitHub-style alert blocks (`> [!NOTE]`, `> [!WARNING]`, etc.).
//!
//! Rust port of the remark-github-alerts / `remark-github-blockquote-alert`
//! plugin. Scans the mdast tree for blockquotes whose first paragraph's first
//! text node begins with `[!TYPE]`, and rewrites them into
//! [`MdxJsxFlowElement`] nodes so downstream JSX rendering can map them to
//! styled alert components.
//!
//! # Recognised types
//!
//! | Markdown prefix | Component |
//! |-----------------|-----------|
//! | `[!NOTE]`       | `<Note>`  |
//! | `[!TIP]`        | `<Tip>`   |
//! | `[!IMPORTANT]`  | `<Important>` |
//! | `[!WARNING]`    | `<Warning>` |
//! | `[!CAUTION]`    | `<Caution>` |
//!
//! Matching is **case-insensitive**.
//!
//! # MDast shape of a GitHub alert
//!
//! CommonMark parsers (including the `markdown` crate) produce:
//!
//! ```text
//! Blockquote {
//!   children: [
//!     Paragraph {
//!       children: [
//!         Text("[!NOTE]\nFirst body line")
//!         // The prefix and the first body line share ONE text node,
//!         // separated by a single '\n'.
//!       ]
//!     },
//!     // Optional additional paragraphs...
//!   ]
//! }
//! ```
//!
//! # Output shape
//!
//! The rewritten node is `MdxJsxFlowElement` — the same type that
//! the directives step produces — so both features produce identical HTML
//! regardless of whether `directives` is also enabled.
//!
//! ```html
//! <Note>body text</Note>
//! ```
//!
//! # Wave 4 (#572)
//!
//! Initial port. Titles are intentionally not supported (the GFM spec for
//! alerts does not include a title extension in the `[!TYPE]` syntax).
//! Wire via `features.githubAlerts: true` in `zfb.config.ts`.

use markdown::mdast::{AttributeContent, MdxJsxFlowElement, Node as MdastNode};
use zfb_md_ast::MdastVisitor;

// ── Component name table ────────────────────────────────────────────────────

/// Map an uppercased alert type string to its JSX component name.
///
/// Returns `None` for unknown types — the blockquote is left unchanged.
fn component_for_type(ty: &str) -> Option<&'static str> {
    match ty {
        "NOTE" => Some("Note"),
        "TIP" => Some("Tip"),
        "IMPORTANT" => Some("Important"),
        "WARNING" => Some("Warning"),
        "CAUTION" => Some("Caution"),
        _ => None,
    }
}

// ── Prefix parsing ──────────────────────────────────────────────────────────

/// Try to parse `[!TYPE]` from the beginning of a text value.
///
/// Returns `Some((component_name, rest_of_text))` when the text starts with a
/// recognised alert prefix.  `rest_of_text` is the content *after* the prefix
/// and the immediately following `\n` (if present), or an empty string if
/// nothing follows.
fn parse_alert_prefix(text: &str) -> Option<(&'static str, &str)> {
    // Must start with `[!`
    let inner = text.strip_prefix("[!")?;
    // Find the closing `]`
    let close = inner.find(']')?;
    let type_str = &inner[..close];
    // Nothing else is allowed on the prefix line (e.g. `[!NOTE] title` is NOT
    // supported — the GFM alert spec has no inline title syntax).
    let after_bracket = &inner[close + 1..]; // everything after `]`
    // Tolerate a CRLF line ending (Windows-authored markdown): the `markdown`
    // crate preserves a literal `\r` in the text value, so `after_bracket` may
    // begin with `\r\n` rather than `\n`.
    let after = after_bracket.strip_prefix('\r').unwrap_or(after_bracket);
    // `after` must be either empty or start with `\n`
    if !after.is_empty() && !after.starts_with('\n') {
        return None;
    }
    let component = component_for_type(&type_str.to_ascii_uppercase())?;
    // Skip the leading `\r\n`/`\n` so callers receive the body text only.
    let body = after_bracket
        .strip_prefix("\r\n")
        .or_else(|| after_bracket.strip_prefix('\n'))
        .unwrap_or(after_bracket);
    Some((component, body))
}

// ── Blockquote rewrite ──────────────────────────────────────────────────────

/// Attempt to rewrite `node` (in-place) if it is a GitHub-style alert
/// blockquote.  Returns `true` when a rewrite was performed.
fn try_rewrite_blockquote(node: &mut MdastNode) -> bool {
    let MdastNode::Blockquote(bq) = node else {
        return false;
    };

    // The first child must be a Paragraph.
    let Some(first_child) = bq.children.first_mut() else {
        return false;
    };
    let MdastNode::Paragraph(first_para) = first_child else {
        return false;
    };

    // The first child of that paragraph must be a Text node beginning with
    // `[!TYPE]`.
    let Some(first_text_node) = first_para.children.first_mut() else {
        return false;
    };
    let MdastNode::Text(text_node) = first_text_node else {
        return false;
    };

    let Some((component, body)) = parse_alert_prefix(&text_node.value) else {
        return false;
    };

    // ── Build JSX children ────────────────────────────────────────────────
    // Strategy:
    //   1. Rewrite the first paragraph's first text node to hold the body
    //      text that follows the prefix line.  If there is no body text in
    //      the first text node, that text node is removed.
    //   2. The first paragraph (now without the `[!TYPE]` prefix) becomes a
    //      child of the JSX element if it still has content.
    //   3. Any subsequent blockquote children (additional paragraphs) are
    //      moved directly into the JSX element.

    // Clone out the body text now (before we mutate first_child).
    let body_owned = body.to_string();

    // Take ownership of all blockquote children.
    let mut bq_children = std::mem::take(&mut bq.children);

    // Mutate the first paragraph's first text node.
    let first_para_children = if let Some(MdastNode::Paragraph(ref mut p)) =
        bq_children.first_mut()
    {
        // Update the text value.
        if let Some(MdastNode::Text(ref mut t)) = p.children.first_mut() {
            t.value = body_owned;
        }
        // If the first text node is now empty AND there are no other children
        // in the first paragraph, drop the whole first paragraph.
        let first_text_empty = p
            .children
            .first()
            .map(|n| matches!(n, MdastNode::Text(t) if t.value.is_empty()))
            .unwrap_or(false);
        if first_text_empty && p.children.len() == 1 {
            // Drop the empty first paragraph entirely; return no children for
            // the first slot.
            bq_children.remove(0);
            // The remaining children (if any) become JSX children directly.
            bq_children
        } else {
            bq_children
        }
    } else {
        bq_children
    };

    // Build JSX element: no attributes (titles not supported in Wave 4).
    let jsx = MdastNode::MdxJsxFlowElement(MdxJsxFlowElement {
        name: Some(component.to_string()),
        attributes: Vec::<AttributeContent>::new(),
        children: first_para_children,
        position: None,
    });

    *node = jsx;
    true
}

// ── Visitor ─────────────────────────────────────────────────────────────────

/// Mdast visitor that rewrites GitHub-style alert blockquotes into
/// [`MdxJsxFlowElement`] nodes.
///
/// Handles alerts at any nesting level (top-level, inside lists, inside nested
/// blockquotes). Implements [`MdastVisitor`] — the caller is responsible for
/// plugging this into the pipeline's mdast phase **before**
/// the `DirectiveRegistry` step (if `directives` is also enabled).
#[derive(Debug, Default, Clone, Copy)]
pub struct GithubAlertsPlugin;

impl GithubAlertsPlugin {
    /// Create a new (stateless) plugin instance.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl MdastVisitor for GithubAlertsPlugin {
    fn visit(&mut self, node: &mut MdastNode) {
        // First try to rewrite this node if it is a blockquote alert.  If it
        // was rewritten we do NOT recurse into the new JSX element (its
        // children have already been moved from the original blockquote and do
        // not need alert-rewriting).
        if try_rewrite_blockquote(node) {
            return;
        }

        // Recurse into containers that may hold nested alerts.
        match node {
            MdastNode::Root(root) => {
                for child in &mut root.children {
                    self.visit(child);
                }
            }
            MdastNode::Blockquote(bq) => {
                // Not an alert (try_rewrite_blockquote returned false above).
                for child in &mut bq.children {
                    self.visit(child);
                }
            }
            MdastNode::List(list) => {
                for item in &mut list.children {
                    self.visit(item);
                }
            }
            MdastNode::ListItem(item) => {
                for child in &mut item.children {
                    self.visit(child);
                }
            }
            // Other node types (Paragraph, Heading, Code, …) cannot contain
            // blockquotes so we skip them.
            _ => {}
        }
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use markdown::mdast::{Paragraph, Text};

    use super::*;

    // ── parse_alert_prefix ────────────────────────────────────────────────

    #[test]
    fn parse_note_no_body() {
        let (comp, body) = parse_alert_prefix("[!NOTE]").unwrap();
        assert_eq!(comp, "Note");
        assert_eq!(body, "");
    }

    #[test]
    fn parse_note_with_body() {
        let (comp, body) = parse_alert_prefix("[!NOTE]\nhello world").unwrap();
        assert_eq!(comp, "Note");
        assert_eq!(body, "hello world");
    }

    #[test]
    fn parse_note_with_crlf_body() {
        // Regression: Windows-authored markdown keeps a literal `\r`, so the
        // prefix line ends `[!NOTE]\r\n`. The alert must still be recognized
        // and the body must not carry a leading `\r`.
        let (comp, body) = parse_alert_prefix("[!NOTE]\r\nhello world").unwrap();
        assert_eq!(comp, "Note");
        assert_eq!(body, "hello world");
    }

    #[test]
    fn parse_case_insensitive() {
        assert_eq!(parse_alert_prefix("[!note]").unwrap().0, "Note");
        assert_eq!(parse_alert_prefix("[!Warning]").unwrap().0, "Warning");
        assert_eq!(parse_alert_prefix("[!TIP]").unwrap().0, "Tip");
        assert_eq!(parse_alert_prefix("[!important]").unwrap().0, "Important");
        assert_eq!(parse_alert_prefix("[!CAUTION]").unwrap().0, "Caution");
    }

    #[test]
    fn parse_unknown_type_returns_none() {
        assert!(parse_alert_prefix("[!CUSTOM]").is_none());
    }

    #[test]
    fn parse_inline_title_not_supported() {
        // `[!NOTE] My Title` must NOT be recognised — no title support in Wave 4.
        assert!(parse_alert_prefix("[!NOTE] My Title").is_none());
    }

    #[test]
    fn parse_plain_text_returns_none() {
        assert!(parse_alert_prefix("hello").is_none());
    }

    // ── component_for_type ────────────────────────────────────────────────

    #[test]
    fn all_five_types_map_to_components() {
        assert_eq!(component_for_type("NOTE"), Some("Note"));
        assert_eq!(component_for_type("TIP"), Some("Tip"));
        assert_eq!(component_for_type("IMPORTANT"), Some("Important"));
        assert_eq!(component_for_type("WARNING"), Some("Warning"));
        assert_eq!(component_for_type("CAUTION"), Some("Caution"));
        assert_eq!(component_for_type("UNKNOWN"), None);
    }

    // ── try_rewrite_blockquote ────────────────────────────────────────────

    fn make_blockquote_alert(prefix: &str, body: &str) -> MdastNode {
        let text_value = if body.is_empty() {
            prefix.to_string()
        } else {
            format!("{prefix}\n{body}")
        };
        MdastNode::Blockquote(markdown::mdast::Blockquote {
            children: vec![MdastNode::Paragraph(Paragraph {
                children: vec![MdastNode::Text(Text {
                    value: text_value,
                    position: None,
                })],
                position: None,
            })],
            position: None,
        })
    }

    #[test]
    fn rewrites_note_blockquote_to_jsx() {
        let mut node = make_blockquote_alert("[!NOTE]", "hello");
        let changed = try_rewrite_blockquote(&mut node);
        assert!(changed);
        let MdastNode::MdxJsxFlowElement(jsx) = node else {
            panic!("expected MdxJsxFlowElement");
        };
        assert_eq!(jsx.name.as_deref(), Some("Note"));
        assert!(jsx.attributes.is_empty());
    }

    #[test]
    fn leaves_regular_blockquote_unchanged() {
        let mut node = make_blockquote_alert("This is not an alert", "");
        let changed = try_rewrite_blockquote(&mut node);
        assert!(!changed);
        assert!(matches!(node, MdastNode::Blockquote(_)));
    }

    #[test]
    fn body_text_is_preserved_in_jsx_child() {
        let mut node = make_blockquote_alert("[!TIP]", "Use this feature");
        try_rewrite_blockquote(&mut node);
        let MdastNode::MdxJsxFlowElement(jsx) = &node else {
            panic!("expected MdxJsxFlowElement");
        };
        // The body text should appear somewhere in children.
        let body: Vec<_> = jsx
            .children
            .iter()
            .filter_map(|c| {
                if let MdastNode::Paragraph(p) = c {
                    p.children.iter().find_map(|n| {
                        if let MdastNode::Text(t) = n {
                            Some(t.value.as_str())
                        } else {
                            None
                        }
                    })
                } else {
                    None
                }
            })
            .collect();
        assert!(body.contains(&"Use this feature"), "body: {body:?}");
    }

    // ── GithubAlertsPlugin visitor ────────────────────────────────────────

    fn make_root(children: Vec<MdastNode>) -> MdastNode {
        MdastNode::Root(markdown::mdast::Root {
            children,
            position: None,
        })
    }

    #[test]
    fn visitor_rewrites_top_level_alert() {
        let mut tree = make_root(vec![make_blockquote_alert("[!WARNING]", "be careful")]);
        GithubAlertsPlugin::new().visit(&mut tree);
        let MdastNode::Root(root) = &tree else {
            panic!("expected Root");
        };
        assert!(
            matches!(root.children[0], MdastNode::MdxJsxFlowElement(_)),
            "expected MdxJsxFlowElement, got {:?}",
            root.children[0]
        );
    }

    #[test]
    fn visitor_is_idempotent() {
        let mut tree = make_root(vec![make_blockquote_alert("[!NOTE]", "body")]);
        GithubAlertsPlugin::new().visit(&mut tree);
        let after_first = tree.clone();
        GithubAlertsPlugin::new().visit(&mut tree);
        // Tree must not change on second pass (JSX is not a blockquote).
        assert_eq!(format!("{tree:?}"), format!("{after_first:?}"));
    }

    #[test]
    fn visitor_recurses_into_lists() {
        let alert = make_blockquote_alert("[!CAUTION]", "nested alert");
        let list_item = MdastNode::ListItem(markdown::mdast::ListItem {
            children: vec![alert],
            position: None,
            spread: false,
            checked: None,
        });
        let list = MdastNode::List(markdown::mdast::List {
            children: vec![list_item],
            position: None,
            ordered: false,
            start: None,
            spread: false,
        });
        let mut tree = make_root(vec![list]);
        GithubAlertsPlugin::new().visit(&mut tree);
        // Navigate to the list item child.
        let MdastNode::Root(root) = &tree else {
            panic!("expected Root");
        };
        let MdastNode::List(list) = &root.children[0] else {
            panic!("expected List");
        };
        let MdastNode::ListItem(item) = &list.children[0] else {
            panic!("expected ListItem");
        };
        assert!(
            matches!(item.children[0], MdastNode::MdxJsxFlowElement(_)),
            "nested alert must be rewritten"
        );
    }
}
