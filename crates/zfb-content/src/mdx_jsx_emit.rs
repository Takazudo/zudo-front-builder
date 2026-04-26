//! mdast → JSX-source emitter.
//!
//! This module turns an MDX source string into a self-contained JSX
//! module string. The output mirrors the `@mdx-js/mdx` public contract:
//! a default-exported `MDXContent({components}) → JSX` function backed
//! by a `_createMdxContent` helper that merges a default `_components`
//! map with caller overrides.
//!
//! ## Why emit JSX text instead of an ESTree
//!
//! `crates/zfb-render/src/swc_pipeline.rs` already understands TSX. By
//! emitting JSX *source* and feeding it through SWC, the JS-codegen
//! path stays unified — there is one place where JSX becomes JS.
//!
//! ## Coverage
//!
//! - Block: paragraph, heading (h1-h6), blockquote, list (ul/ol with
//!   optional `start`), list item, fenced code (with `lang` + `meta`),
//!   thematic break (hr), HTML literal (passed through).
//! - Inline: text, emphasis (em), strong, delete (del), inline code,
//!   link (with optional title), image (with alt + optional title),
//!   line break (br).
//! - MDX flow & text JSX elements: emitted as JSX with attribute
//!   reconstruction. Lowercase tags (`p`, `div`, …) route through the
//!   `_components.<tag>` map; PascalCase identifiers come from the
//!   caller's `components` prop and trigger an explicit
//!   `throw new Error(...)` if missing.
//! - MDX flow & text expressions: emitted verbatim inside `{...}`.
//! - HTML literals: wrapped in `dangerouslySetInnerHTML` on a span so
//!   the DOM gets the original markup. (Trade-off: the embedded HTML
//!   is escaped once for the JS string literal, then injected raw at
//!   runtime — visually faithful, no double-escape.)
//! - Frontmatter: NOT handled here. The caller is expected to strip
//!   YAML/TOML frontmatter via `crate::frontmatter::parse` first.
//!
//! ## Error path
//!
//! Malformed MDX (unterminated JSX, unbalanced braces) is reported by
//! markdown-rs with a `Message` that already carries line/column info.
//! We surface it as [`PipelineError::Parse`] verbatim — no panics.

use markdown::mdast::{AttributeContent, AttributeValue, Node as MdastNode};

use crate::pipeline::PipelineError;

/// Options controlling the emitted JSX module.
#[derive(Debug, Clone)]
pub struct MdxJsxOptions {
    /// Display name / path used for parse-error diagnostics.
    pub filename: String,
}

impl Default for MdxJsxOptions {
    fn default() -> Self {
        Self {
            filename: "<anonymous>.mdx".to_string(),
        }
    }
}

impl MdxJsxOptions {
    /// Set the filename used in error messages.
    #[must_use]
    pub fn with_filename(mut self, filename: impl Into<String>) -> Self {
        self.filename = filename.into();
        self
    }
}

/// Compile an MDX source string into a JSX module string.
///
/// The returned source is JSX text — feed it through SWC's TSX pass
/// (e.g. `zfb-render::SwcPipeline`) to get executable ES module JS.
///
/// # Errors
/// Returns [`PipelineError::Parse`] if markdown-rs rejects the input.
/// The error message includes the line/column reported by markdown-rs.
pub fn mdx_to_jsx_module(input: &str, opts: MdxJsxOptions) -> Result<String, PipelineError> {
    let parse_options = markdown::ParseOptions::mdx();
    let root = markdown::to_mdast(input, &parse_options).map_err(|m| {
        // markdown-rs's Display already emits "line:col-line:col: reason".
        PipelineError::Parse(format!("{}: {m}", opts.filename))
    })?;

    let children: Vec<MdastNode> = match root {
        MdastNode::Root(r) => r.children,
        // markdown-rs always produces a Root for to_mdast, but be
        // defensive — never panic on unexpected shape.
        other => vec![other],
    };

    let mut emitter = JsxEmitter::new();
    let body = emitter.emit_children_block(&children);

    let mut out = String::new();
    out.push_str("import { Fragment as _Fragment } from \"react/jsx-runtime\";\n\n");
    out.push_str("function _createMdxContent({components = {}} = {}) {\n");
    out.push_str("  const _components = {\n");

    // Stable, alphabetised default-tag list so the output is
    // deterministic across runs.
    let mut tags: Vec<&String> = emitter.html_tags.iter().collect();
    tags.sort();
    for tag in tags {
        out.push_str(&format!("    {tag}: \"{tag}\",\n"));
    }
    out.push_str("    ...components,\n");
    out.push_str("  };\n");

    // PascalCase identifiers — components the user must supply. Sorted
    // for deterministic output.
    let mut comps: Vec<&String> = emitter.component_names.iter().collect();
    comps.sort();
    for name in comps {
        out.push_str(&format!(
            "  const {name} = _components.{name} ?? components.{name};\n"
        ));
        out.push_str(&format!(
            "  if (!{name}) throw new Error(\"MDX requires `{name}` to be passed via the `components` prop\");\n"
        ));
    }

    out.push_str("  return (\n");
    out.push_str("    <_Fragment>\n");
    out.push_str(&body);
    out.push_str("    </_Fragment>\n");
    out.push_str("  );\n");
    out.push_str("}\n\n");
    out.push_str("export default function MDXContent(props = {}) {\n");
    out.push_str("  return _createMdxContent(props);\n");
    out.push_str("}\n");

    Ok(out)
}

/// Walks the mdast tree and accumulates JSX source plus the set of
/// referenced HTML tags and PascalCase components.
struct JsxEmitter {
    /// Lowercase HTML tags referenced anywhere in the output. Drives
    /// the default `_components` map at the top of the module.
    html_tags: std::collections::BTreeSet<String>,
    /// PascalCase component identifiers referenced anywhere. Each one
    /// gets a `const Name = _components.Name ?? components.Name; if (!Name) throw …`
    /// preamble.
    component_names: std::collections::BTreeSet<String>,
}

impl JsxEmitter {
    fn new() -> Self {
        Self {
            html_tags: std::collections::BTreeSet::new(),
            component_names: std::collections::BTreeSet::new(),
        }
    }

    /// Emit a series of block-level children, indented to live inside
    /// the `<_Fragment>` body. Each block goes on its own line.
    fn emit_children_block(&mut self, children: &[MdastNode]) -> String {
        let mut out = String::new();
        for child in children {
            let rendered = self.emit_node(child);
            // Skip nodes that emit nothing (e.g. unhandled types) so we
            // do not pollute the output with blank indentation.
            if rendered.trim().is_empty() {
                continue;
            }
            out.push_str("      ");
            out.push_str(&rendered);
            out.push('\n');
        }
        out
    }

    /// Emit a single mdast node as a JSX expression.
    fn emit_node(&mut self, node: &MdastNode) -> String {
        match node {
            MdastNode::Root(r) => {
                // Nested Root is unusual but render its children inline.
                self.emit_inline_children(&r.children)
            }
            MdastNode::Paragraph(p) => self.emit_html("p", &[], &p.children),
            MdastNode::Heading(h) => {
                let depth = h.depth.clamp(1, 6);
                let tag = format!("h{depth}");
                self.emit_html(&tag, &[], &h.children)
            }
            MdastNode::Text(t) => js_string_literal_in_braces(&t.value),
            MdastNode::Emphasis(e) => self.emit_html("em", &[], &e.children),
            MdastNode::Strong(s) => self.emit_html("strong", &[], &s.children),
            MdastNode::Delete(d) => self.emit_html("del", &[], &d.children),
            MdastNode::InlineCode(c) => {
                self.html_tags.insert("code".to_string());
                format!(
                    "<_components.code>{}</_components.code>",
                    js_string_literal_in_braces(&c.value),
                )
            }
            MdastNode::Code(c) => {
                self.html_tags.insert("pre".to_string());
                self.html_tags.insert("code".to_string());
                let mut attrs = String::new();
                if let Some(lang) = &c.lang {
                    attrs.push_str(&format!(
                        " className=\"language-{}\" data-lang={}",
                        escape_attr_literal(lang),
                        jsx_string_attr(lang),
                    ));
                }
                if let Some(meta) = &c.meta {
                    attrs.push_str(&format!(" data-meta={}", jsx_string_attr(meta)));
                }
                format!(
                    "<_components.pre><_components.code{attrs}>{}</_components.code></_components.pre>",
                    js_string_literal_in_braces(&c.value),
                )
            }
            MdastNode::Link(l) => {
                let mut attrs: Vec<(String, AttrVal)> = Vec::new();
                attrs.push(("href".into(), AttrVal::Str(l.url.clone())));
                if let Some(t) = &l.title {
                    attrs.push(("title".into(), AttrVal::Str(t.clone())));
                }
                self.emit_html("a", &attrs, &l.children)
            }
            MdastNode::Image(i) => {
                let mut attrs: Vec<(String, AttrVal)> = Vec::new();
                attrs.push(("src".into(), AttrVal::Str(i.url.clone())));
                attrs.push(("alt".into(), AttrVal::Str(i.alt.clone())));
                if let Some(t) = &i.title {
                    attrs.push(("title".into(), AttrVal::Str(t.clone())));
                }
                self.emit_html_void("img", &attrs)
            }
            MdastNode::List(l) => {
                let tag = if l.ordered { "ol" } else { "ul" };
                let mut attrs: Vec<(String, AttrVal)> = Vec::new();
                if l.ordered {
                    if let Some(start) = l.start {
                        if start != 1 {
                            attrs.push(("start".into(), AttrVal::Num(start as i64)));
                        }
                    }
                }
                self.emit_html(tag, &attrs, &l.children)
            }
            MdastNode::ListItem(li) => self.emit_html("li", &[], &li.children),
            MdastNode::Blockquote(b) => self.emit_html("blockquote", &[], &b.children),
            MdastNode::ThematicBreak(_) => self.emit_html_void("hr", &[]),
            MdastNode::Break(_) => self.emit_html_void("br", &[]),
            MdastNode::Html(h) => {
                // Wrap raw HTML in a span with dangerouslySetInnerHTML so
                // the DOM still receives the original markup. The JS-string
                // escape happens here; React/Preact then injects the raw
                // bytes at runtime — no double-escape.
                format!(
                    "<span dangerouslySetInnerHTML={{{{__html: {}}}}} />",
                    js_string_literal(&h.value),
                )
            }
            MdastNode::MdxJsxFlowElement(j) => {
                self.emit_jsx(j.name.as_deref(), &j.attributes, &j.children)
            }
            MdastNode::MdxJsxTextElement(j) => {
                self.emit_jsx(j.name.as_deref(), &j.attributes, &j.children)
            }
            MdastNode::MdxFlowExpression(e) => format!("{{{}}}", e.value),
            MdastNode::MdxTextExpression(e) => format!("{{{}}}", e.value),
            // Unhandled node kinds (tables, footnotes, definitions,
            // math, references, ESM, frontmatter, …) emit nothing
            // rather than panicking. Sub 4+ can broaden coverage.
            _ => String::new(),
        }
    }

    /// Emit children that live inline inside a JSX element body — each
    /// child goes through `emit_node` and the results are concatenated.
    fn emit_inline_children(&mut self, children: &[MdastNode]) -> String {
        children
            .iter()
            .map(|c| self.emit_node(c))
            .collect::<String>()
    }

    /// Emit a non-void HTML element routed through `_components.<tag>`.
    fn emit_html(
        &mut self,
        tag: &str,
        attrs: &[(String, AttrVal)],
        children: &[MdastNode],
    ) -> String {
        self.html_tags.insert(tag.to_string());
        let attrs_str = render_attrs(attrs);
        let inner = self.emit_inline_children(children);
        format!("<_components.{tag}{attrs_str}>{inner}</_components.{tag}>")
    }

    /// Emit a void HTML element (no closing tag) routed through `_components.<tag>`.
    fn emit_html_void(&mut self, tag: &str, attrs: &[(String, AttrVal)]) -> String {
        self.html_tags.insert(tag.to_string());
        let attrs_str = render_attrs(attrs);
        format!("<_components.{tag}{attrs_str} />")
    }

    /// Emit an MDX JSX element. Lowercase names route through the
    /// `_components` map (so users can override `<p>` etc. inside
    /// MDX JSX as well); PascalCase names are looked up on the caller's
    /// `components` prop and require an explicit identifier preamble.
    fn emit_jsx(
        &mut self,
        name: Option<&str>,
        attrs: &[AttributeContent],
        children: &[MdastNode],
    ) -> String {
        let tag = name.unwrap_or("");
        let attrs_str = render_jsx_attrs(attrs);
        let inner = self.emit_inline_children(children);

        let (open_name, close_name) = if tag.is_empty() {
            // Unnamed JSX is a fragment.
            ("_Fragment".to_string(), "_Fragment".to_string())
        } else if is_component_identifier(tag) {
            self.component_names.insert(tag.to_string());
            (tag.to_string(), tag.to_string())
        } else {
            self.html_tags.insert(tag.to_string());
            (format!("_components.{tag}"), format!("_components.{tag}"))
        };

        if children.is_empty() {
            format!("<{open_name}{attrs_str} />")
        } else {
            format!("<{open_name}{attrs_str}>{inner}</{close_name}>")
        }
    }
}

/// Attribute value carried by HTML-element synthesis (links, images,
/// lists, etc.). MDX JSX attributes go through a different code path
/// because they may be expressions (`{...}`) that we must preserve
/// verbatim.
enum AttrVal {
    Str(String),
    Num(i64),
}

/// Format an attribute list as JSX text (leading space if non-empty).
fn render_attrs(attrs: &[(String, AttrVal)]) -> String {
    if attrs.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for (k, v) in attrs {
        out.push(' ');
        out.push_str(k);
        out.push('=');
        match v {
            AttrVal::Str(s) => out.push_str(&jsx_string_attr(s)),
            AttrVal::Num(n) => out.push_str(&format!("{{{n}}}")),
        }
    }
    out
}

/// Format a parsed MDX JSX attribute list as JSX text.
fn render_jsx_attrs(attrs: &[AttributeContent]) -> String {
    if attrs.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for a in attrs {
        out.push(' ');
        match a {
            AttributeContent::Property(p) => {
                out.push_str(&p.name);
                match &p.value {
                    None => {} // boolean attribute
                    Some(AttributeValue::Literal(s)) => {
                        out.push('=');
                        out.push_str(&jsx_string_attr(s));
                    }
                    Some(AttributeValue::Expression(e)) => {
                        out.push('=');
                        out.push('{');
                        out.push_str(&e.value);
                        out.push('}');
                    }
                }
            }
            AttributeContent::Expression(e) => {
                // Spread attribute. markdown-rs delivers the value
                // already containing the leading `...` (e.g. "...rest"
                // for `<a {...rest} />`), so just wrap it back in `{}`.
                out.push('{');
                out.push_str(&e.value);
                out.push('}');
            }
        }
    }
    out
}

/// Wrap `s` in a JSX attribute string literal: `"escaped value"`.
fn jsx_string_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    out.push_str(&escape_attr_literal(s));
    out.push('"');
    out
}

/// Escape characters that must not appear inside a JSX `"…"` attribute
/// literal: `&`, `<`, `>`, `"` (and the JS line terminators that would
/// break a single-line string).
fn escape_attr_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            other => out.push(other),
        }
    }
    out
}

/// Wrap `s` as a JSX expression containing a JS string literal:
/// `{"escaped value"}`. Used for text content so that downstream JSX
/// processing does not have to special-case whitespace or `<`/`>`.
fn js_string_literal_in_braces(s: &str) -> String {
    format!("{{{}}}", js_string_literal(s))
}

/// Format `s` as a JS string literal (double-quoted, with the minimal
/// set of escapes needed to keep the literal a valid one-liner).
fn js_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // U+2028 / U+2029 break JS string literals; escape them.
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            other if (other as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", other as u32));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// True if `name` looks like a JSX component identifier (starts with an
/// ASCII uppercase letter). Lowercase / dotted names are treated as
/// HTML tag references that resolve through the `_components` map.
fn is_component_identifier(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emit(src: &str) -> String {
        mdx_to_jsx_module(src, MdxJsxOptions::default()).expect("emit ok")
    }

    #[test]
    fn empty_input_produces_minimal_module() {
        let out = emit("");
        // No tags referenced, no components required.
        assert!(out.contains("function _createMdxContent"));
        assert!(out.contains("const _components = {\n    ...components,\n  };"));
        assert!(out.contains("export default function MDXContent"));
        assert!(out.contains("<_Fragment>\n    </_Fragment>"));
    }

    #[test]
    fn paragraph_routes_through_components() {
        let out = emit("hello");
        assert!(out.contains("p: \"p\","));
        assert!(out.contains("<_components.p>{\"hello\"}</_components.p>"));
    }

    #[test]
    fn js_string_escapes_quotes_and_newlines() {
        // Backslash → \\, quote → \\\", control → \\uXXXX
        assert_eq!(js_string_literal("a\"b"), "\"a\\\"b\"");
        assert_eq!(js_string_literal("a\\b"), "\"a\\\\b\"");
        assert_eq!(js_string_literal("a\nb"), "\"a\\nb\"");
        assert_eq!(js_string_literal("a\u{0001}b"), "\"a\\u0001b\"");
    }

    #[test]
    fn is_component_identifier_distinguishes_case() {
        assert!(is_component_identifier("Note"));
        assert!(is_component_identifier("MyComp"));
        assert!(!is_component_identifier("note"));
        assert!(!is_component_identifier("p"));
        assert!(!is_component_identifier(""));
    }

    /// Exercise the `MdastNode::Html` arm directly. MDX parse options
    /// disable raw HTML at the source level (HTML comments are a parse
    /// error suggesting `{/* … */}`), so this code path is unreachable
    /// from `mdx_to_jsx_module(str)`. The arm still exists because
    /// upstream callers may swap parse options later — keep it under
    /// test so the behaviour stays defined.
    #[test]
    fn html_node_emits_dangerously_set_inner_html() {
        use markdown::mdast::Html;
        let mut emitter = JsxEmitter::new();
        let node = MdastNode::Html(Html {
            value: "<style>.x{color:red}</style>".to_string(),
            position: None,
        });
        let out = emitter.emit_node(&node);
        assert!(
            out.contains("dangerouslySetInnerHTML={{__html:"),
            "got: {out}"
        );
        // Source survives, double-quote-escaped inside the JS literal.
        assert!(out.contains("<style>"), "got: {out}");
    }
}
