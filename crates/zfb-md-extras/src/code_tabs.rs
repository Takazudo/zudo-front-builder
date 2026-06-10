//! Grouped code blocks rendered as tabs (`:::code-group` directive).
//!
//! Rust port of the code-groups pattern from zudo-doc. Inside a
//! `:::code-group` container, fenced code blocks become tab panels in a
//! single `<CodeGroup>` JSX element.
//!
//! # Markdown syntax
//!
//! ```text
//! :::code-group
//!
//! (backtick-fence)ts title="index.ts"
//! const x = 1;
//! (backtick-fence)
//!
//! (backtick-fence)js title="index.js"
//! const y = 2;
//! (backtick-fence)
//!
//! :::
//! ```
//!
//! Tab label priority: `title="..."` in the code fence meta string,
//! falling back to the language ID (e.g. `ts`), falling back to `"tab"`
//! when neither is available.
//!
//! # Output shape
//!
//! ```text
//! <CodeGroup tabs={["index.ts","index.js"]}>
//!   <pre data-lang="ts">const x = 1;</pre>
//!   <pre data-lang="js">const y = 2;</pre>
//! </CodeGroup>
//! ```
//!
//! The `tabs` attribute is a JSX expression (`AttributeValue::Expression`)
//! so downstream consumers receive a proper JS array, not a string.
//!
//! # Edge cases
//!
//! - **Empty `:::code-group`** (no `Code` children between opener and closer):
//!   the entire run is left as-is — opener, any inner body nodes, and the
//!   closer all remain in the tree as plain paragraphs. Nothing is dropped.
//! - **Non-`Code` children** inside `:::code-group`: only `Code` nodes
//!   contribute tab panels; other node types are silently dropped. Document
//!   to users that only fenced code blocks are supported inside code groups.
//! - **Nested `:::code-group`**: the inner `:::` closes the outer group per
//!   CommonMark parser semantics. The inner group is treated as an empty
//!   group and left unchanged.
//!
//! # Config
//!
//! Wire via `features.codeTabs: true` in `zfb.config.ts`.
//!
//! # Wave 5 (#576)
//!
//! Initial port. The `name` attribute is accepted on the opener
//! (e.g. `:::code-group name="shell"`) and validated as a string via
//! `AttrSchema` from #584, but currently has no effect on the emitted
//! JSX — it is reserved for future consumer-side use (e.g. persisting
//! the active tab across page navigation). Declared with
//! `AttrType::String` and `required: false`.

use markdown::mdast::{
    AttributeContent, AttributeValue, AttributeValueExpression, MdxJsxAttribute, MdxJsxFlowElement,
    Node as MdastNode, Text,
};
use zfb_md_ast::{AttrSchema, AttrType, DirectiveDef, MdastVisitor};

// ── Public API ──────────────────────────────────────────────────────────────

/// Return the `DirectiveDef` for the `:::code-group` container.
///
/// Callers that want typed-attribute validation (e.g. a user-facing
/// directive registry) can register this definition. The definition is
/// NOT registered with `DirectiveRegistry` at pipeline construction — the
/// code-tabs visitor handles the transformation directly.
///
/// Currently declared attributes:
/// - `name` (string, optional) — reserved for consumer-side tab persistence.
#[must_use]
pub fn code_group_directive_def() -> DirectiveDef {
    DirectiveDef::container("code-group", "CodeGroup").with_attrs(vec![AttrSchema {
        name: "name".to_string(),
        ty: AttrType::String,
        default: None,
        required: false,
    }])
}

/// Mdast visitor that rewrites `:::code-group … :::` containers into
/// [`MdxJsxFlowElement`] nodes.
///
/// Scans the children of any container node (Root, ListItem, Blockquote)
/// for the three-node pattern:
///
/// ```text
/// Paragraph(":::code-group")
/// Code* (one or more)
/// Paragraph(":::")
/// ```
///
/// When found, the entire run is replaced by a single
/// `MdxJsxFlowElement(CodeGroup)` with a `tabs={[...]}` Expression
/// attribute and `<pre data-lang="...">` children.
///
/// Must run in the mdast phase **before** the `DirectiveRegistry` step so
/// the opener paragraph is consumed before the registry sees `code-group`
/// as an unknown directive.
#[derive(Debug, Default, Clone, Copy)]
pub struct CodeTabsPlugin;

impl CodeTabsPlugin {
    /// Create a new (stateless) plugin instance.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl MdastVisitor for CodeTabsPlugin {
    fn visit(&mut self, node: &mut MdastNode) {
        // Recurse into containers first so nested code-groups (if any)
        // are processed innermost-first. Then attempt to rewrite siblings
        // inside this container.
        match node {
            MdastNode::Root(root) => {
                for child in &mut root.children {
                    self.visit(child);
                }
                rewrite_code_groups(&mut root.children);
            }
            MdastNode::ListItem(item) => {
                for child in &mut item.children {
                    self.visit(child);
                }
                rewrite_code_groups(&mut item.children);
            }
            MdastNode::Blockquote(bq) => {
                for child in &mut bq.children {
                    self.visit(child);
                }
                rewrite_code_groups(&mut bq.children);
            }
            _ => {}
        }
    }
}

// ── Core transformation ─────────────────────────────────────────────────────

/// Walk `children` looking for `:::code-group … :::` runs and replace
/// each with a `CodeGroup` JSX element. Modifies the slice in place.
fn rewrite_code_groups(children: &mut Vec<MdastNode>) {
    let mut i = 0;
    while i < children.len() {
        if !is_code_group_open(&children[i]) {
            i += 1;
            continue;
        }

        // Found opener at `i`. Find the matching `:::` close.
        let Some(close_idx) = (i + 1..children.len()).find(|j| is_container_close(&children[*j]))
        else {
            // Unclosed `:::code-group` — leave as-is and move on.
            i += 1;
            continue;
        };

        // Peek at the body (i+1 .. close_idx-1) to check for Code nodes
        // BEFORE mutating the children vec. If there are no Code nodes,
        // leave the entire run (opener + body + closer) as plain paragraphs.
        let has_code = children[i + 1..close_idx]
            .iter()
            .any(|n| matches!(n, MdastNode::Code(_)));

        if !has_code {
            // Skip past the closer so we don't re-examine opener on next
            // iteration; the run stays as plain paragraphs.
            i = close_idx + 1;
            continue;
        }

        // Collect the body nodes (between opener and closer).
        // Extract from close_idx down to i+1 (inclusive) to avoid index
        // shifting issues.
        let body: Vec<MdastNode> = children.drain(i + 1..=close_idx).collect();

        // Remove the opener (now at index i).
        children.remove(i);

        // `body` now contains everything between opener and closer
        // (the closer `Paragraph(":::") ` is the last element).
        // Strip the closer from body.
        let inner: Vec<MdastNode> = body[..body.len() - 1].to_vec();

        // Collect only Code children. Non-Code nodes are dropped with a
        // note in the module doc: only fenced code blocks are meaningful
        // inside a code group.
        let codes: Vec<&MdastNode> = inner
            .iter()
            .filter(|n| matches!(n, MdastNode::Code(_)))
            .collect();

        // Build tab labels.
        let tabs: Vec<String> = codes
            .iter()
            .map(|n| {
                let MdastNode::Code(c) = n else {
                    unreachable!()
                };
                tab_label_for_code(c)
            })
            .collect();

        // Build JSX children: one <pre data-lang="...">…</pre> per Code.
        let jsx_children: Vec<MdastNode> = codes
            .iter()
            .map(|n| {
                let MdastNode::Code(c) = n else {
                    unreachable!()
                };
                build_pre_element(c)
            })
            .collect();

        // Build `tabs={["a","b",...]}` attribute.
        let tabs_attr = build_tabs_attr(&tabs);

        let jsx = MdastNode::MdxJsxFlowElement(MdxJsxFlowElement {
            name: Some("CodeGroup".to_string()),
            attributes: vec![tabs_attr],
            children: jsx_children,
            position: None,
        });

        children.insert(i, jsx);
        i += 1;
    }
}

// ── Node predicates ─────────────────────────────────────────────────────────

/// Returns `true` if `node` is a single-text paragraph whose first line
/// begins with `:::code-group` (case-sensitive).
///
/// Accepts both the bare opener `:::code-group` and the attribute-bearing
/// form `:::code-group name="shell"` (any trailing whitespace + key="value"
/// attrs are tolerated). Attribute parsing/validation itself happens via
/// [`code_group_directive_def`]/`AttrSchema`; this predicate only needs to
/// recognise the opener so the rewrite can fire.
fn is_code_group_open(node: &MdastNode) -> bool {
    let MdastNode::Paragraph(p) = node else {
        return false;
    };
    let Some(MdastNode::Text(t)) = p.children.first() else {
        return false;
    };
    let line = first_line(&t.value).trim();
    let Some(rest) = line.strip_prefix(":::code-group") else {
        return false;
    };
    // Either bare `:::code-group` or `:::code-group<whitespace>...`.
    // Reject `:::code-groupx` (no boundary after `code-group`).
    rest.is_empty() || rest.starts_with(char::is_whitespace)
}

/// Returns `true` if `node` is a single-text paragraph whose first line
/// is exactly `:::` — the standard directive close.
fn is_container_close(node: &MdastNode) -> bool {
    let MdastNode::Paragraph(p) = node else {
        return false;
    };
    let Some(MdastNode::Text(t)) = p.children.first() else {
        return false;
    };
    first_line(&t.value).trim() == ":::"
}

fn first_line(s: &str) -> &str {
    match s.find('\n') {
        Some(i) => &s[..i],
        None => s,
    }
}

// ── Tab label extraction ─────────────────────────────────────────────────────

/// Derive the tab label for a code block.
///
/// Priority order:
/// 1. `title="..."` in `Code.meta`
/// 2. `Code.lang` (the language ID)
/// 3. Literal `"tab"` as last resort
fn tab_label_for_code(code: &markdown::mdast::Code) -> String {
    // 1. Explicit title from meta string.
    if let Some(meta) = &code.meta {
        if let Some(title) = extract_title(meta) {
            return title;
        }
    }
    // 2. Language ID.
    if let Some(lang) = &code.lang {
        if !lang.is_empty() {
            return lang.clone();
        }
    }
    // 3. Fallback.
    "tab".to_string()
}

/// Extract `title="..."` from a meta string like `title="index.ts" foo=1`.
fn extract_title(meta: &str) -> Option<String> {
    let idx = meta.find("title=")?;
    let rest = &meta[idx + "title=".len()..];
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

// ── JSX node builders ────────────────────────────────────────────────────────

/// Build the `tabs={["a","b",...]}` attribute.
///
/// Each label is JSON-encoded (double-quoted, with `"` and `\` escaped).
/// The resulting expression is a JS array literal like `["a","b"]`.
fn build_tabs_attr(labels: &[String]) -> AttributeContent {
    let elems: Vec<String> = labels
        .iter()
        .map(|s| format!("\"{}\"", js_string_escape(s)))
        .collect();
    let expr = format!("[{}]", elems.join(","));
    AttributeContent::Property(MdxJsxAttribute {
        name: "tabs".to_string(),
        value: Some(AttributeValue::Expression(AttributeValueExpression {
            value: expr,
            stops: Vec::new(),
        })),
    })
}

/// Build `<pre data-lang="lang">code_value</pre>` as an
/// [`MdxJsxFlowElement`].
///
/// The code value is stored as a [`Text`] child. `reconstruct_jsx`
/// passes `Text` children verbatim (no escaping) on the HTML path, which
/// is correct for our test fixtures whose code content does not contain
/// `<`, `>`, `{`, or `&`. For production MDX compilation the JSX-path
/// renderer handles its own escaping.
fn build_pre_element(code: &markdown::mdast::Code) -> MdastNode {
    let lang = code.lang.as_deref().unwrap_or("");
    let data_lang_attr = AttributeContent::Property(MdxJsxAttribute {
        name: "data-lang".to_string(),
        value: Some(AttributeValue::Literal(lang.to_string())),
    });
    let code_text = MdastNode::Text(Text {
        value: code.value.clone(),
        position: None,
    });
    MdastNode::MdxJsxFlowElement(MdxJsxFlowElement {
        name: Some("pre".to_string()),
        attributes: vec![data_lang_attr],
        children: vec![code_text],
        position: None,
    })
}

/// Escape a string for use as a JS string literal value.
///
/// Escapes `\`, `"`, and the control characters that would otherwise
/// break a single-line JS string literal (`\n`, `\r`, `\t`, plus a
/// `\u00XX` fallback for any other character < 0x20). A pathological
/// `title="line1\nline2"` in fenced-code meta would otherwise produce
/// invalid JS output once embedded in the `tabs={[...]}` expression.
fn js_string_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            other => out.push(other),
        }
    }
    out
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── extract_title ─────────────────────────────────────────────────────

    #[test]
    fn extract_title_quoted_value() {
        assert_eq!(
            extract_title("title=\"index.ts\""),
            Some("index.ts".to_string())
        );
    }

    #[test]
    fn extract_title_with_surrounding_keys() {
        assert_eq!(
            extract_title("foo=1 title=\"my.ts\" bar=2"),
            Some("my.ts".to_string())
        );
    }

    #[test]
    fn extract_title_missing_returns_none() {
        assert_eq!(extract_title("foo=bar"), None);
    }

    #[test]
    fn extract_title_empty_meta_returns_none() {
        assert_eq!(extract_title(""), None);
    }

    // ── tab_label_for_code ────────────────────────────────────────────────

    fn make_code(lang: Option<&str>, meta: Option<&str>, value: &str) -> markdown::mdast::Code {
        markdown::mdast::Code {
            lang: lang.map(|s| s.to_string()),
            meta: meta.map(|s| s.to_string()),
            value: value.to_string(),
            position: None,
        }
    }

    #[test]
    fn tab_label_uses_title_first() {
        let code = make_code(Some("ts"), Some("title=\"index.ts\""), "");
        assert_eq!(tab_label_for_code(&code), "index.ts");
    }

    #[test]
    fn tab_label_falls_back_to_lang() {
        let code = make_code(Some("ts"), None, "");
        assert_eq!(tab_label_for_code(&code), "ts");
    }

    #[test]
    fn tab_label_falls_back_to_tab_when_no_lang() {
        let code = make_code(None, None, "");
        assert_eq!(tab_label_for_code(&code), "tab");
    }

    // ── js_string_escape ──────────────────────────────────────────────────

    #[test]
    fn js_escape_plain_string() {
        assert_eq!(js_string_escape("index.ts"), "index.ts");
    }

    #[test]
    fn js_escape_double_quote() {
        assert_eq!(js_string_escape("a\"b"), "a\\\"b");
    }

    #[test]
    fn js_escape_backslash() {
        assert_eq!(js_string_escape("a\\b"), "a\\\\b");
    }

    #[test]
    fn js_escape_newline() {
        assert_eq!(js_string_escape("a\nb"), "a\\nb");
    }

    #[test]
    fn js_escape_carriage_return_and_tab() {
        assert_eq!(js_string_escape("a\rb\tc"), "a\\rb\\tc");
    }

    #[test]
    fn js_escape_control_char_uses_unicode_fallback() {
        // 0x01 (SOH) is below 0x20 and not one of the named escapes.
        assert_eq!(js_string_escape("a\u{0001}b"), "a\\u0001b");
    }

    // ── is_code_group_open ────────────────────────────────────────────────

    fn para(text: &str) -> MdastNode {
        MdastNode::Paragraph(markdown::mdast::Paragraph {
            children: vec![MdastNode::Text(Text {
                value: text.to_string(),
                position: None,
            })],
            position: None,
        })
    }

    fn code_node(lang: &str, value: &str) -> MdastNode {
        MdastNode::Code(markdown::mdast::Code {
            lang: Some(lang.to_string()),
            meta: None,
            value: value.to_string(),
            position: None,
        })
    }

    #[test]
    fn is_code_group_open_detects_opener() {
        assert!(is_code_group_open(&para(":::code-group")));
    }

    #[test]
    fn is_code_group_open_detects_opener_with_name_attr() {
        // The documented `name` attribute (declared via AttrSchema) must
        // not block the rewrite from firing.
        assert!(is_code_group_open(&para(":::code-group name=\"shell\"")));
    }

    #[test]
    fn is_code_group_open_detects_opener_with_trailing_whitespace() {
        assert!(is_code_group_open(&para(":::code-group   ")));
    }

    #[test]
    fn is_code_group_open_rejects_prefix_collision() {
        // `:::code-groupx` is NOT a code-group opener — the directive name
        // must end at a word boundary.
        assert!(!is_code_group_open(&para(":::code-groupx")));
        assert!(!is_code_group_open(&para(":::code-group-extra")));
    }

    #[test]
    fn is_code_group_open_rejects_other_text() {
        assert!(!is_code_group_open(&para(":::note")));
        assert!(!is_code_group_open(&para("hello")));
    }

    #[test]
    fn is_container_close_detects_closer() {
        assert!(is_container_close(&para(":::")));
    }

    #[test]
    fn is_container_close_rejects_other_text() {
        assert!(!is_container_close(&para(":::code-group")));
    }

    // ── rewrite_code_groups ───────────────────────────────────────────────

    fn make_root(children: Vec<MdastNode>) -> MdastNode {
        MdastNode::Root(markdown::mdast::Root {
            children,
            position: None,
        })
    }

    #[test]
    fn rewrites_code_group_with_name_attr_opener() {
        // End-to-end smoke test: `:::code-group name="shell"` must trigger
        // the rewrite, not be left as a stray paragraph.
        let mut tree = make_root(vec![
            para(":::code-group name=\"shell\""),
            code_node("ts", "const x = 1;"),
            para(":::"),
        ]);
        CodeTabsPlugin::new().visit(&mut tree);

        let MdastNode::Root(root) = &tree else {
            panic!("expected Root");
        };
        assert_eq!(root.children.len(), 1, "should collapse to 1 JSX node");
        let MdastNode::MdxJsxFlowElement(jsx) = &root.children[0] else {
            panic!("expected MdxJsxFlowElement, got {:?}", root.children[0]);
        };
        assert_eq!(jsx.name.as_deref(), Some("CodeGroup"));
    }

    #[test]
    fn rewrites_simple_code_group() {
        let mut tree = make_root(vec![
            para(":::code-group"),
            code_node("ts", "const x = 1;"),
            code_node("js", "const y = 2;"),
            para(":::"),
        ]);
        CodeTabsPlugin::new().visit(&mut tree);

        let MdastNode::Root(root) = &tree else {
            panic!("expected Root");
        };
        assert_eq!(root.children.len(), 1, "should be collapsed to 1 node");
        let MdastNode::MdxJsxFlowElement(jsx) = &root.children[0] else {
            panic!("expected MdxJsxFlowElement");
        };
        assert_eq!(jsx.name.as_deref(), Some("CodeGroup"));
        assert_eq!(jsx.children.len(), 2);
    }

    #[test]
    fn tabs_attr_contains_lang_fallback_labels() {
        let mut tree = make_root(vec![
            para(":::code-group"),
            code_node("ts", "x"),
            code_node("js", "y"),
            para(":::"),
        ]);
        CodeTabsPlugin::new().visit(&mut tree);

        let MdastNode::Root(root) = &tree else {
            panic!("expected Root");
        };
        let MdastNode::MdxJsxFlowElement(jsx) = &root.children[0] else {
            panic!("expected MdxJsxFlowElement");
        };
        // The tabs attribute must be an Expression containing ["ts","js"].
        let attr = &jsx.attributes[0];
        let AttributeContent::Property(p) = attr else {
            panic!("expected Property");
        };
        assert_eq!(p.name, "tabs");
        let Some(AttributeValue::Expression(expr)) = &p.value else {
            panic!("expected Expression");
        };
        assert_eq!(expr.value, r#"["ts","js"]"#);
    }

    #[test]
    fn unclosed_code_group_is_left_unchanged() {
        // No `:::` closer — opener stays as paragraph.
        let mut tree = make_root(vec![para(":::code-group"), code_node("ts", "x")]);
        CodeTabsPlugin::new().visit(&mut tree);

        let MdastNode::Root(root) = &tree else {
            panic!("expected Root");
        };
        // opener + code = 2 nodes still present (no rewrite)
        assert_eq!(root.children.len(), 2);
        assert!(matches!(root.children[0], MdastNode::Paragraph(_)));
    }

    #[test]
    fn multiple_code_groups_in_document() {
        let mut tree = make_root(vec![
            para(":::code-group"),
            code_node("ts", "a"),
            para(":::"),
            para(":::code-group"),
            code_node("js", "b"),
            para(":::"),
        ]);
        CodeTabsPlugin::new().visit(&mut tree);

        let MdastNode::Root(root) = &tree else {
            panic!("expected Root");
        };
        assert_eq!(root.children.len(), 2, "two code groups → two JSX nodes");
    }

    #[test]
    fn code_group_preceded_by_other_content() {
        let mut tree = make_root(vec![
            para("Some intro text"),
            para(":::code-group"),
            code_node("ts", "x"),
            para(":::"),
            para("Trailing paragraph"),
        ]);
        CodeTabsPlugin::new().visit(&mut tree);

        let MdastNode::Root(root) = &tree else {
            panic!("expected Root");
        };
        assert_eq!(root.children.len(), 3);
        assert!(matches!(root.children[0], MdastNode::Paragraph(_)));
        assert!(matches!(root.children[1], MdastNode::MdxJsxFlowElement(_)));
        assert!(matches!(root.children[2], MdastNode::Paragraph(_)));
    }

    #[test]
    fn visitor_is_idempotent() {
        let mut tree = make_root(vec![
            para(":::code-group"),
            code_node("ts", "x"),
            para(":::"),
        ]);
        CodeTabsPlugin::new().visit(&mut tree);
        let snapshot = format!("{tree:?}");
        CodeTabsPlugin::new().visit(&mut tree);
        assert_eq!(format!("{tree:?}"), snapshot, "second pass must be no-op");
    }

    // ── empty code group ──────────────────────────────────────────────────

    #[test]
    fn empty_code_group_is_left_unchanged() {
        // A `:::code-group` with no fenced code blocks between the opener
        // and closer must not be rewritten — the 3 nodes remain as-is.
        let mut tree = make_root(vec![para(":::code-group"), para(":::")]);
        CodeTabsPlugin::new().visit(&mut tree);

        let MdastNode::Root(root) = &tree else {
            panic!("expected Root");
        };
        assert_eq!(root.children.len(), 2, "opener + closer must remain");
        assert!(
            matches!(root.children[0], MdastNode::Paragraph(_)),
            "opener paragraph must be preserved"
        );
        assert!(
            matches!(root.children[1], MdastNode::Paragraph(_)),
            "closer paragraph must be preserved"
        );
    }

    // ── DirectiveDef / AttrSchema ─────────────────────────────────────────

    #[test]
    fn directive_def_validates_name_attr() {
        // Exercises the AttrSchema API (#584) that declares the optional
        // `name` attribute on the `:::code-group` directive.
        // validate_attrs returns (Result<..., ...>, Vec<warnings>).
        let def = code_group_directive_def();
        let (result, warnings) =
            def.validate_attrs(&[("name".to_string(), "my-group".to_string())]);
        assert!(
            result.is_ok(),
            "valid `name` attribute must pass validation: {result:?}"
        );
        assert!(warnings.is_empty(), "no warnings expected: {warnings:?}");
    }

    #[test]
    fn directive_def_accepts_empty_attrs() {
        // `name` is optional, so an empty attr list is also valid.
        let def = code_group_directive_def();
        let (result, warnings) = def.validate_attrs(&[]);
        assert!(
            result.is_ok(),
            "empty attr list must be valid (name is optional): {result:?}"
        );
        assert!(warnings.is_empty(), "no warnings expected: {warnings:?}");
    }
}
