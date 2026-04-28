//! mdast → hast pipeline + visitor framework.
//!
//! This module implements the markdown/MDX content pipeline used by zfb:
//!
//! 1. parse input → [`markdown::mdast::Node`] (mdast tree) using
//!    [`markdown::to_mdast`] with MDX-aware [`markdown::ParseOptions`] by
//!    default.
//! 2. run user-supplied [`MdastVisitor`]s over the mdast tree (mutation).
//! 3. transform the mutated mdast into [`HastNode`] via
//!    [`mdast_to_hast`] — a lightweight HTML AST defined in this module.
//! 4. run user-supplied [`HastVisitor`]s over the hast tree (mutation).
//!
//! Hast-to-HTML serialization is intentionally NOT implemented here; that
//! is the responsibility of the `serializer` module (Sub 6).
//!
//! markdown-rs (the `markdown` crate, v1.0) does not expose hast directly;
//! it parses to mdast and renders to HTML internally. To give downstream
//! plugins (Sub 4) a stable per-element hook point, we define our own
//! minimal hast representation here. This mirrors the
//! `remark` (mdast) → `rehype` (hast) split in the unified ecosystem.

use std::sync::Arc;

use markdown::mdast::{AttributeContent, AttributeValue, Node as MdastNode};

use crate::plugins::{
    AdmonitionsPlugin, CodeTitlePlugin, HeadingLinksPlugin, ImageEnlargePlugin, MermaidPlugin,
    SyntectPlugin,
};
use crate::syntect_highlight::Highlighter;

/// Lightweight HTML AST node.
///
/// Plugins (mdast and hast visitors) operate on this representation in
/// memory; the serializer turns it into an HTML string later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HastNode {
    /// Document root.
    Root {
        /// Top-level children.
        children: Vec<HastNode>,
    },
    /// HTML element.
    Element {
        /// Tag name (e.g. `"p"`, `"h1"`, `"a"`).
        tag: String,
        /// Attribute list as `(name, value)` pairs.
        ///
        /// Attribute order is preserved so the serializer produces stable
        /// output and so plugins can assert on ordering when useful.
        attrs: Vec<(String, String)>,
        /// Child nodes; empty for void elements.
        children: Vec<HastNode>,
        /// True for self-closing void elements (`img`, `br`, `hr`, etc.).
        void: bool,
    },
    /// Plain text content (escaped on serialization).
    Text(String),
    /// Raw HTML or JSX-component passthrough; the serializer emits this
    /// verbatim without escaping. Used for embedded HTML and MDX/JSX
    /// expressions and elements that downstream tooling will handle.
    Raw(String),
    /// HTML comment body (without the `<!--` / `-->` delimiters).
    Comment(String),
}

/// Mdast visitor: mutates an mdast tree in place.
///
/// Implementors typically call [`MdastNode::children_mut`] to recurse, or
/// implement their own walk. The pipeline does NOT auto-recurse for
/// visitors; each visitor decides its own traversal strategy.
pub trait MdastVisitor {
    /// Visit (and possibly mutate) `node`.
    fn visit(&mut self, node: &mut MdastNode);
}

/// Hast visitor: mutates a hast tree in place.
///
/// Same recursion contract as [`MdastVisitor`].
pub trait HastVisitor {
    /// Visit (and possibly mutate) `node`.
    fn visit(&mut self, node: &mut HastNode);
}

/// Pipeline error type.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// markdown-rs failed to parse the input.
    #[error("markdown parse error: {0}")]
    Parse(String),
}

/// Pipeline configuration: the chain of mdast + hast visitors and the
/// markdown-rs parse options used to produce the initial mdast tree.
pub struct Pipeline {
    mdast_visitors: Vec<Box<dyn MdastVisitor>>,
    hast_visitors: Vec<Box<dyn HastVisitor>>,
    parse_options: markdown::ParseOptions,
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl Pipeline {
    /// New pipeline with MDX-aware parsing (the project default).
    ///
    /// Equivalent to [`Pipeline::with_mdx`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_mdx()
    }

    /// New pipeline using MDX-aware [`markdown::ParseOptions`].
    #[must_use]
    pub fn with_mdx() -> Self {
        Self {
            mdast_visitors: Vec::new(),
            hast_visitors: Vec::new(),
            parse_options: markdown::ParseOptions::mdx(),
        }
    }

    /// New pipeline preloaded with the project's default plugin chain.
    ///
    /// This is the entry point most orchestrator call sites want: it
    /// bundles the directive registry (via [`AdmonitionsPlugin`]) plus
    /// the five custom hast plugins so a `:::note` block compiles to
    /// `<Note>…</Note>`, headings get permalink anchors, code blocks
    /// get figure wrappers + syntect highlighting, mermaid blocks are
    /// flagged for client-side rendering, and width-bearing `<img>`
    /// tags become `<ImageEnlarge>` markers — all without manual
    /// plugin wiring at the call site.
    ///
    /// Callers that need a different mix should construct a pipeline
    /// via [`Pipeline::with_mdx`] (or [`Pipeline::new`]) and append
    /// only the visitors they want.
    ///
    /// # Visitor order
    ///
    /// The pipeline runs in two distinct phases — mdast (markdown AST,
    /// pre-HTML) then hast (HTML AST, post-conversion). Each plugin is
    /// registered in the phase that best matches the rewrite it does:
    ///
    /// **mdast phase** (run first, against the parsed markdown tree):
    ///
    /// 1. [`AdmonitionsPlugin`] — directive-style transforms run on
    ///    mdast because [`DirectiveRegistry`] folds runs of paragraphs
    ///    delimited by `:::name` … `:::` into a single
    ///    [`MdxJsxFlowElement`]. That collapsing has to happen before
    ///    the mdast→hast conversion, or each `:::` line would already
    ///    be its own `<p>` element and the collapse would have to walk
    ///    arbitrary HTML structure to recover the run.
    ///
    /// **hast phase** (run after mdast→hast conversion, in this order):
    ///
    /// 2. [`HeadingLinksPlugin`] — adds `id` + permalink anchor to
    ///    `<h2>`–`<h6>`. Runs first in the hast phase so subsequent
    ///    plugins that might rewrite headings (none today, but the
    ///    door is open) see the slugified ids.
    /// 3. [`CodeTitlePlugin`] — wraps `<pre>` with a titled `data-meta`
    ///    in `<figure>` + `<figcaption>`. Must run BEFORE
    ///    [`SyntectPlugin`] because syntect replaces the whole `<pre>`
    ///    with a [`HastNode::Raw`] HTML fragment; once that happens,
    ///    the `data-meta` attribute is no longer reachable.
    /// 4. [`ImageEnlargePlugin`] — replaces `<img>` elements that have
    ///    an explicit `width` attribute with `<ImageEnlarge>` JSX
    ///    markers. Order-independent relative to syntect/mermaid (it
    ///    only touches `<img>`).
    /// 5. [`MermaidPlugin`] — flags `<pre><code class="language-mermaid">`
    ///    blocks with `data-mermaid="true"`. Must run BEFORE
    ///    [`SyntectPlugin`] so the latter can identify and skip
    ///    mermaid blocks rather than syntect-highlighting them.
    /// 6. [`SyntectPlugin`] — replaces remaining fenced code blocks
    ///    with syntect-highlighted HTML. Runs last in the code-block
    ///    chain so the title-figure wrapper and the mermaid-skip
    ///    decision are already baked in.
    ///
    /// `ResolveLinksPlugin` and `StripMdExtensionPlugin` are NOT in
    /// the defaults: the former needs a project-specific path-to-URL
    /// `source_map` so the orchestrator constructs it explicitly, and
    /// the latter is opt-in for sites whose authors hand-write
    /// `[link](other.md)` style references.
    ///
    /// [`DirectiveRegistry`]: crate::plugins::DirectiveRegistry
    /// [`MdxJsxFlowElement`]: markdown::mdast::MdxJsxFlowElement
    #[must_use]
    pub fn with_defaults() -> Self {
        let highlighter = Arc::new(Highlighter::new());
        let mut p = Self::with_mdx();
        // mdast phase.
        p.add_mdast_visitor(Box::new(AdmonitionsPlugin::new()));
        // hast phase — ordering rationale lives in the doc comment above.
        p.add_hast_visitor(Box::new(HeadingLinksPlugin::new()));
        p.add_hast_visitor(Box::new(CodeTitlePlugin::new()));
        p.add_hast_visitor(Box::new(ImageEnlargePlugin::new()));
        p.add_hast_visitor(Box::new(MermaidPlugin::new()));
        p.add_hast_visitor(Box::new(SyntectPlugin::new(highlighter)));
        p
    }

    /// Append an mdast visitor; visitors run in insertion order.
    pub fn add_mdast_visitor(&mut self, v: Box<dyn MdastVisitor>) -> &mut Self {
        self.mdast_visitors.push(v);
        self
    }

    /// Append a hast visitor; visitors run in insertion order.
    pub fn add_hast_visitor(&mut self, v: Box<dyn HastVisitor>) -> &mut Self {
        self.hast_visitors.push(v);
        self
    }

    /// Run only the mdast visitor chain against an externally-parsed
    /// mdast tree.
    ///
    /// Sub 46 (#46) added this seam so the JSX emit path
    /// (`mdx_jsx_emit::compile_mdx_to_jsx_module_cached`) can apply the
    /// pipeline's mdast visitors without going through full
    /// [`Pipeline::run`] (which would also build a hast tree the JSX
    /// emitter does not consume). Hast visitors stay untouched here —
    /// they are applied by [`Pipeline::run`] only.
    pub fn apply_mdast_visitors(&mut self, node: &mut MdastNode) {
        for v in &mut self.mdast_visitors {
            v.visit(node);
        }
    }

    /// Parse `input` to mdast, run mdast visitors, transform to hast, run
    /// hast visitors. Returns the resulting hast root.
    ///
    /// # Errors
    /// Returns [`PipelineError::Parse`] if markdown-rs rejects the input.
    pub fn run(&mut self, input: &str) -> Result<HastNode, PipelineError> {
        let mut mdast = markdown::to_mdast(input, &self.parse_options)
            .map_err(|m| PipelineError::Parse(m.to_string()))?;

        for v in &mut self.mdast_visitors {
            v.visit(&mut mdast);
        }

        let mut hast = mdast_to_hast(&mdast);

        for v in &mut self.hast_visitors {
            v.visit(&mut hast);
        }

        Ok(hast)
    }
}

/// Convert an mdast node into a hast node.
///
/// See the module docs for the full coverage list. Unhandled node types
/// degrade to [`HastNode::Raw("".into())`] so the pipeline never panics
/// on novel input — Sub 4 / Sub 6 can extend handling later.
#[must_use]
pub fn mdast_to_hast(node: &MdastNode) -> HastNode {
    match node {
        MdastNode::Root(r) => HastNode::Root {
            children: convert_children(&r.children),
        },
        MdastNode::Paragraph(p) => element("p", vec![], convert_children(&p.children)),
        MdastNode::Heading(h) => {
            let depth = h.depth.clamp(1, 6);
            let tag = format!("h{depth}");
            element(&tag, vec![], convert_children(&h.children))
        }
        MdastNode::Text(t) => HastNode::Text(t.value.clone()),
        MdastNode::Emphasis(e) => element("em", vec![], convert_children(&e.children)),
        MdastNode::Strong(s) => element("strong", vec![], convert_children(&s.children)),
        MdastNode::Delete(d) => element("del", vec![], convert_children(&d.children)),
        MdastNode::InlineCode(c) => element("code", vec![], vec![HastNode::Text(c.value.clone())]),
        MdastNode::Code(c) => {
            // Fenced code block. Wrap raw text in <pre><code>; expose
            // `lang` and `meta` as data-* attrs so Sub 4 plugins (e.g.
            // rehypeCodeTitle) and Sub 5 (syntect) can inspect them.
            let mut code_attrs: Vec<(String, String)> = Vec::new();
            if let Some(lang) = &c.lang {
                code_attrs.push(("class".to_string(), format!("language-{lang}")));
                code_attrs.push(("data-lang".to_string(), lang.clone()));
            }
            if let Some(meta) = &c.meta {
                code_attrs.push(("data-meta".to_string(), meta.clone()));
            }
            let code_el = HastNode::Element {
                tag: "code".to_string(),
                attrs: code_attrs,
                children: vec![HastNode::Text(c.value.clone())],
                void: false,
            };
            element("pre", vec![], vec![code_el])
        }
        MdastNode::Link(l) => {
            let mut attrs = vec![("href".to_string(), l.url.clone())];
            if let Some(title) = &l.title {
                attrs.push(("title".to_string(), title.clone()));
            }
            element("a", attrs, convert_children(&l.children))
        }
        MdastNode::Image(i) => {
            let mut attrs = vec![
                ("src".to_string(), i.url.clone()),
                ("alt".to_string(), i.alt.clone()),
            ];
            if let Some(title) = &i.title {
                attrs.push(("title".to_string(), title.clone()));
            }
            HastNode::Element {
                tag: "img".to_string(),
                attrs,
                children: vec![],
                void: true,
            }
        }
        MdastNode::List(l) => {
            let tag = if l.ordered { "ol" } else { "ul" };
            let mut attrs: Vec<(String, String)> = Vec::new();
            if l.ordered {
                if let Some(start) = l.start {
                    if start != 1 {
                        attrs.push(("start".to_string(), start.to_string()));
                    }
                }
            }
            element(tag, attrs, convert_children(&l.children))
        }
        MdastNode::ListItem(li) => element("li", vec![], convert_children(&li.children)),
        MdastNode::Blockquote(b) => element("blockquote", vec![], convert_children(&b.children)),
        MdastNode::ThematicBreak(_) => HastNode::Element {
            tag: "hr".to_string(),
            attrs: vec![],
            children: vec![],
            void: true,
        },
        MdastNode::Break(_) => HastNode::Element {
            tag: "br".to_string(),
            attrs: vec![],
            children: vec![],
            void: true,
        },
        MdastNode::Html(h) => HastNode::Raw(h.value.clone()),
        MdastNode::MdxJsxFlowElement(j) => HastNode::Raw(reconstruct_jsx(
            j.name.as_deref(),
            &j.attributes,
            &j.children,
        )),
        MdastNode::MdxJsxTextElement(j) => HastNode::Raw(reconstruct_jsx(
            j.name.as_deref(),
            &j.attributes,
            &j.children,
        )),
        MdastNode::MdxFlowExpression(e) => HastNode::Raw(format!("{{{}}}", e.value)),
        MdastNode::MdxTextExpression(e) => HastNode::Raw(format!("{{{}}}", e.value)),
        // Unhandled: degrade to empty Raw so we never crash on
        // unsupported input. Tables, footnotes, definitions, math,
        // reference links/images, ESM, frontmatter, etc. fall here. They
        // become passthrough holes that Sub 4 plugins can later fill in.
        _ => HastNode::Raw(String::new()),
    }
}

/// Convert a slice of mdast children into a vec of hast children.
fn convert_children(children: &[MdastNode]) -> Vec<HastNode> {
    children.iter().map(mdast_to_hast).collect()
}

/// Build a non-void element.
fn element(tag: &str, attrs: Vec<(String, String)>, children: Vec<HastNode>) -> HastNode {
    HastNode::Element {
        tag: tag.to_string(),
        attrs,
        children,
        void: false,
    }
}

/// Best-effort textual reconstruction of an MDX JSX element.
///
/// We do NOT try to round-trip every MDX construct losslessly here; the
/// goal is to produce a plausible source-level snippet so the serializer
/// can pass it through verbatim. Sub 4 plugins that synthesize JSX
/// elements (e.g. `<Note>`) typically build the [`HastNode::Raw`]
/// payload themselves and bypass this path.
fn reconstruct_jsx(
    name: Option<&str>,
    attrs: &[AttributeContent],
    children: &[MdastNode],
) -> String {
    let tag = name.unwrap_or("");
    let attrs_str = render_attrs(attrs);
    let space = if attrs_str.is_empty() { "" } else { " " };

    if children.is_empty() {
        // Self-closing.
        return format!("<{tag}{space}{attrs_str} />");
    }

    let inner: String = children
        .iter()
        .map(|c| match c {
            MdastNode::Text(t) => t.value.clone(),
            MdastNode::Html(h) => h.value.clone(),
            MdastNode::MdxFlowExpression(e) => format!("{{{}}}", e.value),
            MdastNode::MdxTextExpression(e) => format!("{{{}}}", e.value),
            MdastNode::MdxJsxFlowElement(j) => {
                reconstruct_jsx(j.name.as_deref(), &j.attributes, &j.children)
            }
            MdastNode::MdxJsxTextElement(j) => {
                reconstruct_jsx(j.name.as_deref(), &j.attributes, &j.children)
            }
            // Fallback: stringify the markdown text content. This loses
            // formatting but keeps content visible; downstream plugins
            // generally avoid putting markdown inside JSX bodies anyway.
            other => other.to_string(),
        })
        .collect();

    format!("<{tag}{space}{attrs_str}>{inner}</{tag}>")
}

/// Render an MDX attribute list back to JSX-ish source text.
fn render_attrs(attrs: &[AttributeContent]) -> String {
    attrs
        .iter()
        .map(|a| match a {
            AttributeContent::Property(p) => match &p.value {
                None => p.name.clone(),
                Some(AttributeValue::Literal(s)) => format!("{}=\"{}\"", p.name, s),
                Some(AttributeValue::Expression(e)) => {
                    format!("{}={{{}}}", p.name, e.value)
                }
            },
            AttributeContent::Expression(e) => format!("{{...{}}}", e.value),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use markdown::mdast::{Node as MdastNode, Text};

    fn run(input: &str) -> HastNode {
        Pipeline::new().run(input).expect("parse ok")
    }

    fn root_children(node: &HastNode) -> &[HastNode] {
        match node {
            HastNode::Root { children } => children,
            _ => unreachable!("expected Root, got {node:?}"),
        }
    }

    fn first_child(node: &HastNode) -> &HastNode {
        &root_children(node)[0]
    }

    fn assert_element<'a>(
        node: &'a HastNode,
        expected_tag: &str,
    ) -> (&'a [(String, String)], &'a [HastNode], bool) {
        match node {
            HastNode::Element {
                tag,
                attrs,
                children,
                void,
            } => {
                assert_eq!(tag, expected_tag, "tag mismatch in {node:?}");
                (attrs.as_slice(), children.as_slice(), *void)
            }
            _ => unreachable!("expected Element<{expected_tag}>, got {node:?}"),
        }
    }

    // 1. Empty input → empty Root.
    #[test]
    fn empty_input_yields_empty_root() {
        let h = run("");
        assert_eq!(h, HastNode::Root { children: vec![] });
    }

    // 2. Plain paragraph.
    #[test]
    fn plain_paragraph() {
        let h = run("hello world");
        let (_, p_children, _) = assert_element(first_child(&h), "p");
        assert_eq!(p_children, &[HastNode::Text("hello world".into())]);
    }

    // 3. Heading levels 1-6.
    #[test]
    fn heading_levels_1_through_6() {
        for depth in 1..=6 {
            let hashes = "#".repeat(depth);
            let input = format!("{hashes} title {depth}");
            let h = run(&input);
            let expected_tag = format!("h{depth}");
            let (_, children, _) = assert_element(first_child(&h), &expected_tag);
            assert_eq!(
                children,
                &[HastNode::Text(format!("title {depth}"))],
                "depth {depth}"
            );
        }
    }

    // 4. Bold/italic/strikethrough/inline-code.
    //
    // Strikethrough (`~~x~~`) is a GFM construct; MDX ParseOptions does
    // not enable it. We test it explicitly with a Delete mdast node fed
    // through the converter so the mapping is still covered.
    #[test]
    fn inline_formatting() {
        // *em* and **strong** and `code` work under MDX parse options.
        let h = run("*a* **b** `c`");
        let (_, p_children, _) = assert_element(first_child(&h), "p");

        let (_, em_children, _) = assert_element(&p_children[0], "em");
        assert_eq!(em_children, &[HastNode::Text("a".into())]);

        // p_children[1] is a Text(" ") between inline elements.
        let (_, strong_children, _) = assert_element(&p_children[2], "strong");
        assert_eq!(strong_children, &[HastNode::Text("b".into())]);

        let (_, code_children, _) = assert_element(&p_children[4], "code");
        assert_eq!(code_children, &[HastNode::Text("c".into())]);

        // Strikethrough: feed a synthetic Delete node directly.
        let del = MdastNode::Delete(markdown::mdast::Delete {
            children: vec![MdastNode::Text(Text {
                value: "gone".into(),
                position: None,
            })],
            position: None,
        });
        let hast = mdast_to_hast(&del);
        let (_, del_children, _) = assert_element(&hast, "del");
        assert_eq!(del_children, &[HastNode::Text("gone".into())]);
    }

    // 5. Fenced code block preserves lang and meta as attrs.
    #[test]
    fn fenced_code_preserves_lang_and_meta() {
        // markdown-rs accepts arbitrary text after the lang token as `meta`.
        let h = run("```rust title=\"main.rs\"\nfn main() {}\n```\n");
        let (_, pre_children, _) = assert_element(first_child(&h), "pre");
        let (code_attrs, code_children, _) = assert_element(&pre_children[0], "code");

        let attr_map: std::collections::HashMap<&str, &str> = code_attrs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(attr_map.get("class"), Some(&"language-rust"));
        assert_eq!(attr_map.get("data-lang"), Some(&"rust"));
        assert_eq!(attr_map.get("data-meta"), Some(&"title=\"main.rs\""));

        assert_eq!(code_children, &[HastNode::Text("fn main() {}".into())]);
    }

    // 6. Link with and without title.
    #[test]
    fn link_with_and_without_title() {
        let h = run("[a](https://example.com)");
        let (_, p_children, _) = assert_element(first_child(&h), "p");
        let (a_attrs, a_children, _) = assert_element(&p_children[0], "a");
        assert_eq!(
            a_attrs,
            &[("href".to_string(), "https://example.com".to_string())]
        );
        assert_eq!(a_children, &[HastNode::Text("a".into())]);

        let h = run("[a](https://example.com \"hi\")");
        let (_, p_children, _) = assert_element(first_child(&h), "p");
        let (a_attrs, _, _) = assert_element(&p_children[0], "a");
        assert_eq!(
            a_attrs,
            &[
                ("href".to_string(), "https://example.com".to_string()),
                ("title".to_string(), "hi".to_string()),
            ]
        );
    }

    // 7. Image (void).
    #[test]
    fn image_is_void_element() {
        let h = run("![alt text](pic.png)");
        let (_, p_children, _) = assert_element(first_child(&h), "p");
        let (img_attrs, img_children, void) = assert_element(&p_children[0], "img");
        assert!(void, "img must be a void element");
        assert!(img_children.is_empty());
        assert_eq!(
            img_attrs,
            &[
                ("src".to_string(), "pic.png".to_string()),
                ("alt".to_string(), "alt text".to_string()),
            ]
        );

        let h = run("![alt](pic.png \"caption\")");
        let (_, p_children, _) = assert_element(first_child(&h), "p");
        let (img_attrs, _, _) = assert_element(&p_children[0], "img");
        assert!(img_attrs.contains(&("title".to_string(), "caption".to_string())));
    }

    // 8. Ordered + unordered lists.
    #[test]
    fn ordered_and_unordered_lists() {
        let h = run("- a\n- b\n");
        let (_, ul_children, _) = assert_element(first_child(&h), "ul");
        assert_eq!(ul_children.len(), 2);
        let (_, li0, _) = assert_element(&ul_children[0], "li");
        // The list item wraps a paragraph.
        let (_, p0, _) = assert_element(&li0[0], "p");
        assert_eq!(p0, &[HastNode::Text("a".into())]);

        let h = run("1. one\n2. two\n");
        let (_, ol_children, _) = assert_element(first_child(&h), "ol");
        assert_eq!(ol_children.len(), 2);
    }

    // 9. Nested blockquote.
    #[test]
    fn nested_blockquote() {
        let h = run("> outer\n>\n> > inner\n");
        let (_, bq_children, _) = assert_element(first_child(&h), "blockquote");
        // outer has a paragraph then an inner blockquote.
        let mut found_inner = false;
        for c in bq_children {
            if let HastNode::Element { tag, .. } = c {
                if tag == "blockquote" {
                    found_inner = true;
                }
            }
        }
        assert!(
            found_inner,
            "expected nested <blockquote>, got {bq_children:?}"
        );
    }

    // 10. MDX JSX element passes through as Raw.
    //
    // Walk the hast tree and collect every [`HastNode::Raw`] payload —
    // markdown-rs may parse JSX as either a flow element (top-level) or
    // a text element (inside a paragraph) depending on surrounding
    // whitespace. Either way the converter must produce Raw with the
    // original-ish source so the serializer passes it through.
    fn collect_raw(node: &HastNode, out: &mut Vec<String>) {
        match node {
            HastNode::Raw(s) => out.push(s.clone()),
            HastNode::Root { children } => {
                for c in children {
                    collect_raw(c, out);
                }
            }
            HastNode::Element { children, .. } => {
                for c in children {
                    collect_raw(c, out);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn mdx_jsx_passes_through_as_raw() {
        let h = run("<Note>hello</Note>\n");
        let mut raws = Vec::new();
        collect_raw(&h, &mut raws);
        assert!(
            raws.iter()
                .any(|r| r.contains("<Note") && r.contains("hello") && r.contains("</Note>")),
            "expected a Raw containing the <Note>…</Note> source, got raws={raws:?} from {h:?}"
        );

        // Self-closing.
        let h = run("<Hr />\n");
        let mut raws = Vec::new();
        collect_raw(&h, &mut raws);
        assert!(
            raws.iter().any(|r| r.contains("<Hr") && r.contains("/>")),
            "expected a self-closing <Hr /> Raw, got raws={raws:?}"
        );
    }

    // 11. mdast visitor mutation runs.
    struct UppercaseText;
    impl MdastVisitor for UppercaseText {
        fn visit(&mut self, node: &mut MdastNode) {
            if let MdastNode::Text(t) = node {
                t.value = t.value.to_uppercase();
            }
            if let Some(children) = node.children_mut() {
                for c in children {
                    self.visit(c);
                }
            }
        }
    }

    #[test]
    fn mdast_visitor_mutation_runs() {
        let mut p = Pipeline::new();
        p.add_mdast_visitor(Box::new(UppercaseText));
        let h = p.run("hello world").expect("parse ok");
        let (_, p_children, _) = assert_element(first_child(&h), "p");
        assert_eq!(p_children, &[HastNode::Text("HELLO WORLD".into())]);
    }

    // 12. hast visitor mutation runs.
    struct AddTouchedClass;
    impl HastVisitor for AddTouchedClass {
        fn visit(&mut self, node: &mut HastNode) {
            if let HastNode::Element {
                attrs, children, ..
            } = node
            {
                attrs.push(("class".to_string(), "touched".to_string()));
                for c in children {
                    self.visit(c);
                }
            } else if let HastNode::Root { children } = node {
                for c in children {
                    self.visit(c);
                }
            }
        }
    }

    // 13. with_defaults wires the directive registry — `:::note`
    // becomes `<Note>…</Note>` without manual plugin wiring.
    #[test]
    fn with_defaults_wires_directive_registry() {
        let mut p = Pipeline::with_defaults();
        let h = p
            .run(":::note\n\nbody\n\n:::\n")
            .expect("pipeline runs ok");
        let mut raws = Vec::new();
        collect_raw(&h, &mut raws);
        assert!(
            raws.iter()
                .any(|r| r.contains("<Note") && r.contains("</Note>")),
            "expected a <Note>…</Note> Raw block, got raws={raws:?}",
        );
    }

    // 14. Pipeline::new() / with_mdx() stay plugin-free so callers can
    // opt out of the defaults.
    #[test]
    fn new_and_with_mdx_have_no_plugins() {
        // `:::note` should NOT collapse into a <Note> element when the
        // caller picks the no-plugins constructor — the paragraph runs
        // through to hast as plain `<p>:::note</p>` etc.
        for mut p in [Pipeline::new(), Pipeline::with_mdx()] {
            let h = p
                .run(":::note\n\nbody\n\n:::\n")
                .expect("pipeline runs ok");
            let mut raws = Vec::new();
            collect_raw(&h, &mut raws);
            assert!(
                !raws.iter().any(|r| r.contains("<Note")),
                "no-plugins pipeline must not synthesize <Note>; got raws={raws:?}",
            );
        }
    }

    #[test]
    fn hast_visitor_mutation_runs() {
        let mut p = Pipeline::new();
        p.add_hast_visitor(Box::new(AddTouchedClass));
        let h = p.run("# heading\n\nbody").expect("parse ok");
        let children = root_children(&h);
        for c in children {
            if let HastNode::Element { attrs, .. } = c {
                assert!(
                    attrs.contains(&("class".to_string(), "touched".to_string())),
                    "expected class=touched on {c:?}"
                );
            } else {
                unreachable!("expected element, got {c:?}");
            }
        }
    }
}
