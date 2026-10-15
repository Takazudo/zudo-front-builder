//! HTML serializer. Implemented in Wave 3 / Sub 6.
//!
//! Turns a [`crate::pipeline::HastNode`] tree into an HTML fragment string.
//! Mirrors the `hast-util-to-html` role from the unified ecosystem but stays
//! intentionally minimal: no whitespace prettification, no DOCTYPE,
//! no per-tag content-model rules beyond the void-element list.
//!
//! Escaping rules (see [`serialize`] for the full contract):
//! - Text nodes escape `&`, `<`, `>`. Quotes are left alone in text content.
//! - Attribute values escape `&`, `<`, `>`, `"`. Always emitted with double
//!   quotes (`attr="value"`).
//! - Raw nodes are emitted verbatim — used for JSX components and
//!   pre-rendered HTML (e.g. syntect output).
//! - Comments emit as `<!-- body -->`. The body is sanitized to prevent
//!   premature comment termination by replacing every `--` with `- -`.
//! - Void elements (canonical HTML5 list, plus anything with `void=true`)
//!   self-close as `<tag attrs/>`.

use crate::pipeline::HastNode;

/// Canonical HTML5 void-element list. Matched case-insensitively.
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

/// Serialize a hast tree to an HTML fragment string.
///
/// See module docs for the escaping and void-element rules.
#[must_use]
pub fn serialize(node: &HastNode) -> String {
    let mut out = String::new();
    serialize_into(node, &mut out);
    out
}

/// Same as [`serialize`], but appends to an existing buffer.
///
/// Useful when assembling many fragments into one allocation.
pub fn serialize_into(node: &HastNode, out: &mut String) {
    match node {
        HastNode::Root { children } => {
            for c in children {
                serialize_into(c, out);
            }
        }
        HastNode::Element {
            tag,
            attrs,
            children,
            void,
        } => {
            let is_void = *void || is_void_tag(tag);
            out.push('<');
            out.push_str(tag);
            for (k, v) in attrs {
                out.push(' ');
                out.push_str(k);
                out.push_str("=\"");
                push_attr_escaped(v, out);
                out.push('"');
            }
            if is_void {
                out.push_str("/>");
            } else {
                out.push('>');
                for c in children {
                    serialize_into(c, out);
                }
                out.push_str("</");
                out.push_str(tag);
                out.push('>');
            }
        }
        HastNode::Text(t) => push_text_escaped(t, out),
        // Both Raw flavours are emitted verbatim — the JSX-vs-HTML
        // distinction matters only on the JSX emit path (#121); on the
        // HTML path, Raw and JsxRaw both stand for "trust the producer,
        // do not escape".
        HastNode::Raw(s) | HastNode::JsxRaw(s) => out.push_str(s),
        HastNode::Comment(body) => {
            out.push_str("<!--");
            push_comment_sanitized(body, out);
            out.push_str("-->");
        }
    }
}

/// True if `tag` is a canonical HTML5 void element (case-insensitive).
fn is_void_tag(tag: &str) -> bool {
    VOID_ELEMENTS.iter().any(|v| v.eq_ignore_ascii_case(tag))
}

/// Escape a text-node value: `&`, `<`, `>`. Quotes stay as-is.
fn push_text_escaped(s: &str, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
}

/// Escape an attribute value: `&`, `<`, `>`, `"`.
fn push_attr_escaped(s: &str, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
}

/// Sanitize a comment body so it cannot terminate the comment early.
///
/// HTML comments cannot contain `--`. We replace each occurrence with
/// `- -`. We do not try to escape angle brackets — they are legal inside
/// comments.
fn push_comment_sanitized(s: &str, out: &mut String) {
    let mut prev_dash = false;
    for ch in s.chars() {
        if ch == '-' && prev_dash {
            // Break the run by inserting a space before this dash.
            // Keep prev_dash=true so the next consecutive dash is also
            // separated — without this, `a---b` would produce `a- --b`
            // which still contains `--`.
            out.push(' ');
            out.push('-');
            // prev_dash stays true: the just-pushed `-` counts as a dash
        } else {
            out.push(ch);
            prev_dash = ch == '-';
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(children: Vec<HastNode>) -> HastNode {
        HastNode::Root { children }
    }

    fn elem(tag: &str, attrs: Vec<(&str, &str)>, children: Vec<HastNode>) -> HastNode {
        HastNode::Element {
            tag: tag.to_string(),
            attrs: attrs
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            children,
            void: false,
        }
    }

    fn void_elem(tag: &str, attrs: Vec<(&str, &str)>) -> HastNode {
        HastNode::Element {
            tag: tag.to_string(),
            attrs: attrs
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            children: vec![],
            void: true,
        }
    }

    // 1. Empty Root → "".
    #[test]
    fn empty_root_serializes_to_empty_string() {
        assert_eq!(serialize(&root(vec![])), "");
    }

    // 2. Single text node → escaped text.
    #[test]
    fn text_node_is_escaped() {
        let h = root(vec![HastNode::Text("a & b < c > d".into())]);
        assert_eq!(serialize(&h), "a &amp; b &lt; c &gt; d");
    }

    // 3. Element with no children → <tag></tag>.
    #[test]
    fn element_with_no_children_emits_open_close() {
        let h = elem("div", vec![], vec![]);
        assert_eq!(serialize(&h), "<div></div>");
    }

    // 4. Void element (img with src + alt) → self-closing.
    #[test]
    fn void_img_self_closes() {
        let h = HastNode::Element {
            tag: "img".to_string(),
            attrs: vec![
                ("src".to_string(), "pic.png".to_string()),
                ("alt".to_string(), "x".to_string()),
            ],
            children: vec![],
            // Even with void=false, the canonical list forces self-close.
            void: false,
        };
        assert_eq!(serialize(&h), r#"<img src="pic.png" alt="x"/>"#);
    }

    // 5. Element.void=true on a non-standard tag forces void.
    #[test]
    fn explicit_void_forces_self_close_for_unknown_tag() {
        let h = void_elem("custom-thing", vec![("data-x", "1")]);
        assert_eq!(serialize(&h), r#"<custom-thing data-x="1"/>"#);
    }

    // 6. Attribute value escaping for & < > " .
    #[test]
    fn attribute_values_are_escaped() {
        let h = elem(
            "a",
            vec![("title", r#"a & b < c > d " e"#)],
            vec![HastNode::Text("link".into())],
        );
        assert_eq!(
            serialize(&h),
            r#"<a title="a &amp; b &lt; c &gt; d &quot; e">link</a>"#,
        );
    }

    // 7. Text node escaping covers & < > but not quotes.
    #[test]
    fn text_node_escaping_leaves_quotes_alone() {
        let h = HastNode::Text(r#"x & "y" < z"#.into());
        assert_eq!(serialize(&h), r#"x &amp; "y" &lt; z"#);
    }

    // 8. Raw passthrough — no escaping at all.
    #[test]
    fn raw_node_passes_through_verbatim() {
        let raw = "<script>alert('x & y')</script>";
        let h = HastNode::Raw(raw.into());
        assert_eq!(serialize(&h), raw);
    }

    // 9. Comment with `--` in body is sanitized to `- -`.
    #[test]
    fn comment_double_dash_is_broken_up() {
        let h = HastNode::Comment("hi -- there -- you".into());
        // Each `--` becomes `- -`.
        assert_eq!(serialize(&h), "<!--hi - - there - - you-->");

        // Three dashes in a row: every consecutive pair is broken up.
        // `a---b` → `a- - -b` (no `--` survives).
        let h = HastNode::Comment("a---b".into());
        assert_eq!(serialize(&h), "<!--a- - -b-->");
    }

    // 9b. Longer runs of dashes must not leave any `--` inside the comment body.
    #[test]
    fn comment_long_dash_run_has_no_double_dash() {
        for input in &["a----b", "a-----b"] {
            let h = HastNode::Comment((*input).into());
            let out = serialize(&h);
            // Strip the `<!--` prefix and `-->` suffix so we only check the body.
            let body = out
                .strip_prefix("<!--")
                .and_then(|s| s.strip_suffix("-->"))
                .unwrap_or(&out);
            assert!(
                !body.contains("--"),
                "comment body still contains `--`: {body:?} (input={input:?})"
            );
        }
    }

    // 10. Nested elements.
    #[test]
    fn nested_elements_render_correctly() {
        let h = elem(
            "div",
            vec![],
            vec![elem("p", vec![], vec![HastNode::Text("hi".into())])],
        );
        assert_eq!(serialize(&h), "<div><p>hi</p></div>");
    }

    // 11. Multiple attrs preserve input order.
    #[test]
    fn multiple_attrs_preserve_order() {
        let h = elem(
            "a",
            vec![
                ("href", "https://example.com"),
                ("rel", "noopener"),
                ("target", "_blank"),
            ],
            vec![HastNode::Text("ex".into())],
        );
        assert_eq!(
            serialize(&h),
            r#"<a href="https://example.com" rel="noopener" target="_blank">ex</a>"#,
        );
    }

    // 12. serialize_into appends rather than overwriting.
    #[test]
    fn serialize_into_appends_to_existing_buffer() {
        let mut buf = String::from("PREFIX:");
        let h = elem("span", vec![], vec![HastNode::Text("yo".into())]);
        serialize_into(&h, &mut buf);
        assert_eq!(buf, "PREFIX:<span>yo</span>");
    }

    // Bonus: Root containing many heterogeneous children — concatenated
    // with no wrapper.
    #[test]
    fn root_concatenates_children_with_no_wrapper() {
        let h = root(vec![
            elem("h1", vec![], vec![HastNode::Text("Title".into())]),
            elem("p", vec![], vec![HastNode::Text("body".into())]),
            HastNode::Raw("<!-- inline raw -->".into()),
        ]);
        assert_eq!(
            serialize(&h),
            "<h1>Title</h1><p>body</p><!-- inline raw -->",
        );
    }

    // Bonus: empty attribute value emits attr="".
    #[test]
    fn empty_attribute_value_emits_quotes() {
        let h = elem("input", vec![("disabled", "")], vec![]);
        // input is a void element, so it self-closes.
        assert_eq!(serialize(&h), r#"<input disabled=""/>"#);
    }

    // Bonus: void-element list match is case-insensitive (defensive).
    #[test]
    fn void_list_is_case_insensitive() {
        let h = elem("BR", vec![], vec![]);
        assert_eq!(serialize(&h), "<BR/>");
    }
}
