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

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use markdown::mdast::{AttributeContent, AttributeValue, Node as MdastNode};
use sha2::{Digest, Sha256};

use crate::pipeline::{mdast_to_hast, HastNode, Pipeline, PipelineError};
use crate::plugins::heading_links::{next_slug, slugify};

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
/// This entry point does NOT run any pipeline visitors — it parses MDX
/// and emits JSX directly. Callers that need directive-style admonitions
/// (`:::note`), CJK-aware emphasis, or other mdast-phase plugins to fire
/// against the MDX content should use
/// [`mdx_to_jsx_module_with_pipeline`] instead. See zfb#116.
///
/// # Errors
/// Returns [`PipelineError::Parse`] if markdown-rs rejects the input.
/// The error message includes the line/column reported by markdown-rs.
pub fn mdx_to_jsx_module(input: &str, opts: MdxJsxOptions) -> Result<String, PipelineError> {
    mdx_to_jsx_module_inner(input, opts, None)
}

/// Compile an MDX source string into a JSX module string, running the
/// supplied pipeline's mdast AND hast visitors against the parsed tree
/// before JSX emission.
///
/// Use this entry point when MDX content must be transformed by
/// content-pipeline plugins (admonitions, CJK-friendly emphasis,
/// heading anchors, syntax highlighting, mermaid, image-enlarge,
/// strip-md-ext, …) before reaching the JSX emitter — typically by
/// passing a [`Pipeline::with_defaults`] from the loader.
///
/// Implementation note (#121): the JSX path internally takes a hast
/// detour — `mdast → mdast visitors → mdast_to_hast → hast visitors →
/// hast→JSX walker` — so the same five hast plugins
/// `Pipeline::with_defaults` ships (heading-links, code-title,
/// image-enlarge, mermaid, syntect) plus the opt-in strip-md-ext fire
/// on MDX content. Pre-#121 this entry point only applied mdast
/// visitors; the HTML serializer path ([`Pipeline::run`]) already ran
/// both phases. The two paths now exercise the same plugin chain.
///
/// # Errors
/// Returns [`PipelineError::Parse`] if markdown-rs rejects the input.
///
/// [`Pipeline::with_defaults`]: crate::pipeline::Pipeline::with_defaults
/// [`Pipeline::run`]: crate::pipeline::Pipeline::run
pub fn mdx_to_jsx_module_with_pipeline(
    input: &str,
    opts: MdxJsxOptions,
    pipeline: &mut Pipeline,
) -> Result<String, PipelineError> {
    mdx_to_jsx_module_inner(input, opts, Some(pipeline))
}

/// Internal core for [`mdx_to_jsx_module`] that optionally runs the
/// supplied [`Pipeline`]'s mdast AND hast visitors against the parsed
/// tree before JSX emission.
///
/// When `pipeline` is `None`, this is byte-for-byte identical to the
/// pre-Sub-46 behaviour: parse → match Root → emit straight from mdast.
/// Existing zfb-content unit tests assert exact substrings of the
/// no-pipeline output, so this path is preserved verbatim.
///
/// When `pipeline` is `Some`, the body takes a hast detour added in
/// #121: mdast visitors run first, then `mdast_to_hast` builds a hast
/// tree, then hast visitors run, then a hast→JSX walker emits the
/// module body. This lets hast-phase plugins from
/// [`Pipeline::with_defaults`] (heading-links, code-title,
/// image-enlarge, mermaid, syntect) plus opt-in strip-md-ext fire on
/// MDX content. The HTML serializer path ([`Pipeline::run`]) is
/// unchanged.
fn mdx_to_jsx_module_inner(
    input: &str,
    opts: MdxJsxOptions,
    pipeline: Option<&mut Pipeline>,
) -> Result<String, PipelineError> {
    // `ParseOptions::mdx()` enables the `mdx_esm` construct but does NOT
    // activate it: the construct's start state checks `mdx_esm_parse.is_some()`
    // before firing (see markdown-rs `crates/zfb-content/construct/mdx_esm.rs`).
    // Without a parse function, `import { Foo } from "pkg"` is parsed as a
    // Paragraph — the `{ Foo }` becomes an MdxTextExpression, leaving the
    // skeletal text visible in the rendered output.
    //
    // We supply a permissive parser that always returns Ok. zfb does not need
    // to validate ESM syntax here — the bundler (SWC) handles that later. All
    // we need is for import/export statements to be classified as MdxjsEsm
    // nodes so the emitter can silently drop them (they fall through to the
    // `_ => String::new()` catch-all in `emit_node`).
    // `math_flow` / `math_text` aren't part of `Constructs::mdx()` by
    // default. We turn them on so `$$...$$` and `$...$` blocks parse
    // into proper `Math` / `InlineMath` mdast nodes (see the dedicated
    // arms in `emit_node` below). Without these, the LaTeX content
    // would leak into the emitted JSX as bare expression containers
    // like `{\infty}` — syntactically invalid JS that esbuild rejects,
    // forcing the bundler's defensive skip in
    // `crates/zfb-build/src/bundler.rs` to fall the whole page back to
    // `<pre data-zfb-content-fallback>`. See zfb#93.
    //
    // Side effect: a literal `$` in prose is now parsed as the start
    // of inline math (markdown-rs's `math_text_single_dollar` defaults
    // to true). Authors who want a literal dollar sign must escape it
    // as `\$` — same convention as the upstream remark-math ecosystem.
    let parse_options = markdown::ParseOptions {
        constructs: markdown::Constructs {
            math_flow: true,
            math_text: true,
            ..markdown::Constructs::mdx()
        },
        mdx_esm_parse: Some(Box::new(|_value: &str| -> markdown::MdxSignal {
            markdown::MdxSignal::Ok
        })),
        ..markdown::ParseOptions::default()
    };
    let mut root = markdown::to_mdast(input, &parse_options).map_err(|m| {
        // markdown-rs's Display already emits "line:col-line:col: reason".
        PipelineError::Parse(format!("{}: {m}", opts.filename))
    })?;

    // When a pipeline is supplied, run the mdast visitors and detour
    // through hast (#121). Otherwise stay on the original mdast→JSX
    // path so existing no-pipeline output stays byte-identical.
    let mut pipeline_mut: Option<&mut Pipeline> = pipeline;
    let take_hast_detour = pipeline_mut.is_some();
    if let Some(p) = pipeline_mut.as_deref_mut() {
        p.apply_mdast_visitors(&mut root);
    }

    let children: Vec<MdastNode> = match root {
        MdastNode::Root(r) => r.children,
        // markdown-rs always produces a Root for to_mdast, but be
        // defensive — never panic on unexpected shape.
        other => vec![other],
    };

    // Heading metadata is collected from the post-mdast-visitor tree
    // either way — hast plugins like `HeadingLinksPlugin` mutate the
    // hast (slug → `id` attribute, anchor child) but the slug
    // computation in `collect_headings` shares the same `next_slug`
    // helper so the emitted `headings` array stays in lockstep with
    // the rendered `<hN id="…">`.
    let headings = collect_headings(&children);

    let (body, html_tags, component_names) = if take_hast_detour {
        // Wrap children back into a Root so `mdast_to_hast` can recurse
        // through them as a single node — its public signature takes
        // `&MdastNode`.
        let mdast_root = MdastNode::Root(markdown::mdast::Root {
            children,
            position: None,
        });
        let mut hast = mdast_to_hast(&mdast_root);
        if let Some(p) = pipeline_mut {
            p.apply_hast_visitors(&mut hast);
        }
        let mut bridge = HastJsxBridge::new();
        let body = bridge.emit_root(&hast);
        (body, bridge.html_tags, bridge.component_names)
    } else {
        let mut emitter = JsxEmitter::new();
        let body = emitter.emit_children_block(&children);
        (body, emitter.html_tags, emitter.component_names)
    };

    let mut out = String::new();
    out.push_str("import { Fragment as _Fragment } from \"react/jsx-runtime\";\n\n");
    out.push_str(&render_headings_export(&headings));
    out.push('\n');
    out.push_str("function _createMdxContent({components = {}} = {}) {\n");
    out.push_str("  const _components = {\n");

    // Stable, alphabetised default-tag list so the output is
    // deterministic across runs.
    let mut tags: Vec<&String> = html_tags.iter().collect();
    tags.sort();
    for tag in tags {
        out.push_str(&format!("    {tag}: \"{tag}\",\n"));
    }
    out.push_str("    ...components,\n");
    out.push_str("  };\n");

    // PascalCase identifiers — components the user must supply. Sorted
    // for deterministic output.
    let mut comps: Vec<&String> = component_names.iter().collect();
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

/// One entry of the emitted `headings` array.
///
/// Mirrors the per-heading record a TOC component consumes:
/// `{ depth, slug, text }`. Construction order matches document order
/// so callers can iterate without re-sorting.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HeadingEntry {
    /// `1`–`6`, matching the `<hN>` level the heading would render as.
    depth: u8,
    /// URL-safe identifier. For `<h2>`–`<h6>` this matches the `id`
    /// attribute that [`crate::plugins::heading_links`] emits on the
    /// rendered tag, including the per-document `-1`, `-2`, … numbering
    /// applied to repeated slugs. `<h1>` slugs are computed with the
    /// same algorithm but never participate in the dedup counter
    /// (heading_links does not touch `<h1>`).
    slug: String,
    /// Plain-text projection of the heading's inline children — the
    /// same projection [`crate::plugins::heading_links`] feeds into the
    /// slugger, so `slugify(text)` round-trips for the `<h1>` case and
    /// `slugify(text)` is the *base* slug (pre-dedup) for `<h2>`–`<h6>`.
    text: String,
}

/// Walk the parsed mdast and collect every heading in document order.
///
/// Slugs match what [`crate::plugins::heading_links`] would emit for
/// the same document: same `slugify`, same `-1`, `-2`, … numbering for
/// repeats. Per heading_links semantics, only `<h2>`–`<h6>` participate
/// in the dedup counter; `<h1>` slugs are emitted raw.
fn collect_headings(children: &[MdastNode]) -> Vec<HeadingEntry> {
    let mut out = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    walk_collect_headings(children, &mut out, &mut seen);
    out
}

fn walk_collect_headings(
    nodes: &[MdastNode],
    out: &mut Vec<HeadingEntry>,
    seen: &mut HashMap<String, usize>,
) {
    for node in nodes {
        match node {
            MdastNode::Heading(h) => {
                let depth = h.depth.clamp(1, 6);
                let text = mdast_inline_text(&h.children);
                let base = slugify(&text);
                // For h2-h6 we must mirror heading_links' next_slug()
                // exactly so `headings[i].slug` matches the rendered
                // `<hN id="…">`. h1 is left out of the dedup pool
                // because heading_links never sees it.
                let slug = if depth == 1 {
                    base
                } else {
                    next_slug(seen, &base)
                };
                out.push(HeadingEntry { depth, slug, text });
            }
            // Headings can legally nest inside blockquotes / list items,
            // so descend into block-level containers. We deliberately do
            // NOT descend into Paragraph/Heading children themselves —
            // headings cannot appear there.
            MdastNode::Root(r) => walk_collect_headings(&r.children, out, seen),
            MdastNode::Blockquote(b) => walk_collect_headings(&b.children, out, seen),
            MdastNode::List(l) => walk_collect_headings(&l.children, out, seen),
            MdastNode::ListItem(li) => walk_collect_headings(&li.children, out, seen),
            _ => {}
        }
    }
}

/// Plain-text projection of a sequence of inline mdast nodes.
///
/// Concatenates every reachable text payload in document order, which
/// matches what `hast-util-to-string` (and therefore
/// [`crate::plugins::heading_links`] via [`extract_text`](crate::plugins::util::hast_text::extract_text))
/// produces after the same heading is rendered to hast. Concretely:
///
/// - `Text(value)` → `value`
/// - `InlineCode(value)` → `value` (rendered as `<code>value</code>`,
///   which `extract_text` flattens back to `value`)
/// - `Emphasis` / `Strong` / `Delete` / `Link` → recurse into children
///   (formatting marks render as element wrappers, not extra text)
/// - `Image` → contributes its `alt` text (matching `<img alt>` →
///   `extract_text` would skip it, but `alt` is the closest plain-text
///   substitute and TOC consumers expect it)
/// - `Break` → single space (renders as `<br>` then a space-equivalent)
/// - MDX text expressions / JSX → contribute nothing (opaque to TOCs)
fn mdast_inline_text(children: &[MdastNode]) -> String {
    let mut out = String::new();
    for c in children {
        push_inline_text(c, &mut out);
    }
    out
}

fn push_inline_text(node: &MdastNode, out: &mut String) {
    match node {
        MdastNode::Text(t) => out.push_str(&t.value),
        MdastNode::InlineCode(c) => out.push_str(&c.value),
        MdastNode::Emphasis(e) => {
            for c in &e.children {
                push_inline_text(c, out);
            }
        }
        MdastNode::Strong(s) => {
            for c in &s.children {
                push_inline_text(c, out);
            }
        }
        MdastNode::Delete(d) => {
            for c in &d.children {
                push_inline_text(c, out);
            }
        }
        MdastNode::Link(l) => {
            for c in &l.children {
                push_inline_text(c, out);
            }
        }
        MdastNode::Image(i) => out.push_str(&i.alt),
        MdastNode::Break(_) => out.push(' '),
        // Math nodes contribute their raw LaTeX as plain text — best
        // available projection for a TOC entry like `## Limit as $x \to
        // \infty$`. `extract_text` over the rendered `<code class="math
        // math-inline">…raw LaTeX…</code>` would surface the same
        // bytes, so this keeps `headings[i].text` consistent with the
        // rendered DOM.
        MdastNode::Math(m) => out.push_str(&m.value),
        MdastNode::InlineMath(m) => out.push_str(&m.value),
        // Everything else (MDX expressions, raw HTML literals, JSX
        // elements, …) contributes no plain text — TOCs cannot do
        // anything useful with `{count}` or `<Note/>` tokens.
        _ => {}
    }
}

/// Render the `export const headings = [...];` line.
///
/// Always emitted — even an empty document produces
/// `export const headings = [];` so callers can rely on the binding's
/// existence without optional-chaining at the import site.
fn render_headings_export(headings: &[HeadingEntry]) -> String {
    let mut out = String::from("export const headings = [");
    if !headings.is_empty() {
        out.push('\n');
        for h in headings {
            out.push_str(&format!(
                "  {{ depth: {}, slug: {}, text: {} }},\n",
                h.depth,
                js_string_literal(&h.slug),
                js_string_literal(&h.text),
            ));
        }
    }
    out.push_str("];\n");
    out
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
            // remark-math `$$...$$` block. Mirrors the shape
            // markdown-rs's HTML serializer (`on_enter_raw_flow`)
            // produces — `<pre><code class="language-math math-display">`
            // — routed through `_components` so MDX consumers can still
            // override `<pre>` / `<code>`. The LaTeX body goes in as a
            // JS string literal (`{"…"}`) so backslash sequences never
            // surface as raw JSX expressions, which is the bug in #93.
            // Client-side KaTeX auto-render keys on `math-display`.
            MdastNode::Math(m) => {
                self.html_tags.insert("pre".to_string());
                self.html_tags.insert("code".to_string());
                format!(
                    "<_components.pre><_components.code className=\"language-math math-display\">{}</_components.code></_components.pre>",
                    js_string_literal_in_braces(&m.value),
                )
            }
            // remark-math `$...$` inline. Same shape as inline code
            // with an added `language-math math-inline` class — the
            // companion to `Math` above and to markdown-rs's
            // `on_enter_raw_text` HTML output.
            MdastNode::InlineMath(m) => {
                self.html_tags.insert("code".to_string());
                format!(
                    "<_components.code className=\"language-math math-inline\">{}</_components.code>",
                    js_string_literal_in_braces(&m.value),
                )
            }
            // Unhandled node kinds (tables, footnotes, definitions,
            // references, ESM, frontmatter, …) emit nothing rather
            // than panicking. Sub 4+ can broaden coverage.
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

/// Walks a [`HastNode`] tree and emits JSX source — the post-#121
/// counterpart to [`JsxEmitter`].
///
/// The mdast→hast detour means hast plugins (heading-links, syntect,
/// mermaid, image-enlarge, code-title, strip-md-ext) have already
/// rewritten the tree by the time this bridge runs. The bridge keeps
/// the same component-routing contract as [`JsxEmitter`]:
///
/// - Lowercase [`HastNode::Element`] tags → `<_components.<tag>>` so
///   callers can override every default tag.
/// - Plain text → JS string literal in braces (so JSX never sees raw
///   `<` / `>` in content).
/// - [`HastNode::Raw`] (HTML — produced by syntect, `MdastNode::Html`,
///   etc.) → wrapped in a span with `dangerouslySetInnerHTML` so the
///   DOM still receives the original markup. JSX cannot embed
///   arbitrary HTML such as inline `<span style="...">` verbatim
///   because the JSX transform would treat unknown attribute shapes as
///   syntax errors.
/// - [`HastNode::JsxRaw`] (MDX JSX, `{…}` expressions) → emitted
///   verbatim so PascalCase component references and expression
///   containers survive untouched. PascalCase identifiers in the
///   payload are scanned and added to `component_names` so the module
///   preamble emits the `const Name = _components.Name ?? components.Name`
///   guard.
/// - [`HastNode::Comment`] → JSX comment `{/* body */}`.
struct HastJsxBridge {
    html_tags: std::collections::BTreeSet<String>,
    component_names: std::collections::BTreeSet<String>,
}

impl HastJsxBridge {
    fn new() -> Self {
        Self {
            html_tags: std::collections::BTreeSet::new(),
            component_names: std::collections::BTreeSet::new(),
        }
    }

    /// Emit the body of `<_Fragment>`. Each top-level child of `Root`
    /// goes on its own line so the generated module stays readable.
    fn emit_root(&mut self, node: &HastNode) -> String {
        let HastNode::Root { children } = node else {
            // Defensive: hast plugins should never replace Root with a
            // non-Root, but if they did, fall back to single-node emit.
            let mut out = String::new();
            let rendered = self.emit_node(node);
            if !rendered.trim().is_empty() {
                out.push_str("      ");
                out.push_str(&rendered);
                out.push('\n');
            }
            return out;
        };
        let mut out = String::new();
        for child in children {
            let rendered = self.emit_node(child);
            if rendered.trim().is_empty() {
                continue;
            }
            out.push_str("      ");
            out.push_str(&rendered);
            out.push('\n');
        }
        out
    }

    /// Emit a single hast node as a JSX expression.
    fn emit_node(&mut self, node: &HastNode) -> String {
        match node {
            HastNode::Root { children } => self.emit_inline_children(children),
            HastNode::Element {
                tag,
                attrs,
                children,
                void,
            } => self.emit_element(tag, attrs, children, *void),
            HastNode::Text(s) => js_string_literal_in_braces(s),
            // HTML passthrough — wrap in dangerouslySetInnerHTML so the
            // browser receives the original markup untouched. JSX
            // cannot inline arbitrary HTML safely.
            HastNode::Raw(s) => {
                if s.is_empty() {
                    String::new()
                } else {
                    format!(
                        "<span dangerouslySetInnerHTML={{{{__html: {}}}}} />",
                        js_string_literal(s),
                    )
                }
            }
            // JSX-shaped passthrough (MDX components, `{…}` expressions)
            // — embed verbatim, after recording any PascalCase
            // identifiers so the module preamble can declare them.
            HastNode::JsxRaw(s) => {
                collect_jsx_component_names(s, &mut self.component_names);
                s.clone()
            }
            HastNode::Comment(body) => format!("{{/* {} */}}", body.replace("*/", "* /")),
        }
    }

    fn emit_inline_children(&mut self, children: &[HastNode]) -> String {
        children
            .iter()
            .map(|c| self.emit_node(c))
            .collect::<String>()
    }

    fn emit_element(
        &mut self,
        tag: &str,
        attrs: &[(String, String)],
        children: &[HastNode],
        void: bool,
    ) -> String {
        // Some tags emitted by hast plugins (e.g. <svg>, <polygon>,
        // <button>, <figure>) are not in the React-DOM "always
        // override-able" set we expose by default, but the contract
        // remains: route every lowercase tag through `_components.<tag>`
        // so authors can override anything. Pascal-case tags do not
        // appear in hast Element nodes (mdast emits them as JsxRaw).
        self.html_tags.insert(tag.to_string());
        let attrs_str = render_hast_attrs(attrs);
        if void || is_void_html_tag(tag) {
            return format!("<_components.{tag}{attrs_str} />");
        }
        if children.is_empty() {
            return format!("<_components.{tag}{attrs_str}></_components.{tag}>");
        }
        let inner = self.emit_inline_children(children);
        format!("<_components.{tag}{attrs_str}>{inner}</_components.{tag}>")
    }
}

/// True for canonical HTML5 void elements (matched case-insensitively).
///
/// Mirrors `serializer::is_void_tag`'s list so the bridge self-closes
/// the same set of tags the HTML serializer would.
fn is_void_html_tag(tag: &str) -> bool {
    matches!(
        tag.to_ascii_lowercase().as_str(),
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "source"
            | "track"
            | "wbr"
    )
}

/// Render hast `(name, value)` attribute pairs as JSX attribute text.
///
/// The `class` attribute name is preserved verbatim — both Preact and
/// React (via SWC's classic JSX transform) accept the HTML-style name.
/// Attribute values are HTML-escaped to keep the JSX parser happy
/// (mirrors `jsx_string_attr` in [`JsxEmitter`]).
///
/// Empty-valued attributes (e.g. `data-mermaid=""` synthesized by
/// `MermaidPlugin`) emit as `attr=""` to keep the attribute present
/// and serializable.
fn render_hast_attrs(attrs: &[(String, String)]) -> String {
    if attrs.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for (k, v) in attrs {
        out.push(' ');
        out.push_str(k);
        out.push('=');
        out.push_str(&jsx_string_attr(v));
    }
    out
}

/// Scan a JSX-shaped string for PascalCase opening-tag identifiers and
/// add them to `out`. The bridge feeds [`HastNode::JsxRaw`] payloads
/// here so the module preamble emits the
/// `const Name = _components.Name ?? components.Name` guard for every
/// referenced component, just like the mdast-only path does via
/// `JsxEmitter::emit_jsx`.
///
/// We do NOT try to be a full JSX parser — the rule is:
///   - find `<` not preceded by `<` (skips `<<` artefacts);
///   - the next char must be ASCII uppercase;
///   - subsequent chars are alphanumeric / `_` / `$` / `.` (dotted
///     names like `<Foo.Bar>` are tracked under their head, which is
///     all the preamble emits anyway);
///   - skip closing tags (`</Name>`), they're picked up by the
///     matching open.
///
/// The logic is intentionally permissive — false positives only mean
/// the module declares a `const Name = …` it never uses, which is
/// harmless dead code. False negatives (missing a real reference)
/// would surface as a `ReferenceError` at runtime, which the existing
/// `if (!Name) throw new Error(...)` preamble already converts into a
/// readable diagnostic.
fn collect_jsx_component_names(jsx: &str, out: &mut std::collections::BTreeSet<String>) {
    let bytes = jsx.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        // Skip closing tag — its open-tag counterpart will already
        // have been recorded.
        if j < bytes.len() && bytes[j] == b'/' {
            i = j + 1;
            continue;
        }
        if j >= bytes.len() {
            break;
        }
        let first = bytes[j];
        if !first.is_ascii_uppercase() {
            i = j;
            continue;
        }
        let start = j;
        while j < bytes.len() {
            let c = bytes[j];
            if c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c == b'.' {
                j += 1;
            } else {
                break;
            }
        }
        let name = &jsx[start..j];
        // Track only the head identifier — `Foo.Bar` registers as
        // `Foo`, matching what the preamble actually declares.
        let head = name.split('.').next().unwrap_or(name);
        if !head.is_empty() {
            out.insert(head.to_string());
        }
        i = j;
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

// -----------------------------------------------------------------------------
// Module-specifier + cache surface (Sub 2)
//
// `compile_mdx_to_jsx_module` wraps `mdx_to_jsx_module` with two extras the
// renderer (Sub 3+) needs to dedupe compiled modules:
//
//   1. A content hash of the JSX source — first 8 hex chars of SHA-256, the
//      same dialect `zfb-css::pipeline::hash_8` already speaks.
//   2. A stable `mdx://<collection>/<slug>#<hash8>` specifier the loader can
//      route on. `<collection>` is the parent directory name of `file_path`,
//      `<slug>` is the file stem.
//
// The cache is opt-in: callers without a cache reference get a fresh
// compilation every call. This keeps unit tests hermetic and makes the cost
// of caching a deliberate, visible decision at the call site.
// -----------------------------------------------------------------------------

/// Output of [`compile_mdx_to_jsx_module`] — JSX source plus the metadata
/// the renderer needs to address and dedupe the compiled module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledMdx {
    /// Self-contained JSX module string (same shape as `mdx_to_jsx_module`).
    pub jsx_source: String,
    /// First 8 lowercase-hex chars of `sha256(jsx_source)`.
    pub content_hash: String,
    /// Stable `mdx://<collection>/<slug>#<hash8>` URL for routing.
    pub specifier: String,
}

/// Parsed view of an `mdx://<collection>/<slug>#<hash8>` URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdxModuleSpecifier {
    pub collection: String,
    pub slug: String,
    pub content_hash: String,
}

impl MdxModuleSpecifier {
    /// Render back to canonical `mdx://<collection>/<slug>#<hash8>` form.
    #[must_use]
    pub fn to_url(&self) -> String {
        format!(
            "mdx://{c}/{s}#{h}",
            c = self.collection,
            s = self.slug,
            h = self.content_hash
        )
    }
}

/// Errors that can come out of [`parse_mdx_specifier`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SpecifierError {
    #[error("expected `mdx://` scheme, got: {0}")]
    BadScheme(String),
    #[error("missing `<collection>/<slug>` segment in: {0}")]
    MissingPath(String),
    #[error("missing `#<hash8>` fragment in: {0}")]
    MissingHash(String),
    #[error("hash fragment must be 8 lowercase hex chars, got: {0}")]
    BadHash(String),
}

/// Parse an `mdx://<collection>/<slug>#<hash8>` URL.
///
/// `<collection>` and `<slug>` must both be non-empty; `<hash8>` must be
/// exactly 8 lowercase hex chars (matching what
/// [`compile_mdx_to_jsx_module`] emits).
///
/// # Errors
/// Returns the corresponding [`SpecifierError`] variant on any structural
/// problem.
pub fn parse_mdx_specifier(input: &str) -> Result<MdxModuleSpecifier, SpecifierError> {
    let rest = input
        .strip_prefix("mdx://")
        .ok_or_else(|| SpecifierError::BadScheme(input.to_string()))?;

    // Split on '#' first so a slug containing nothing weird can't swallow
    // the fragment.
    let (path, hash) = rest
        .split_once('#')
        .ok_or_else(|| SpecifierError::MissingHash(input.to_string()))?;

    let (collection, slug) = path
        .split_once('/')
        .ok_or_else(|| SpecifierError::MissingPath(input.to_string()))?;

    if collection.is_empty() || slug.is_empty() {
        return Err(SpecifierError::MissingPath(input.to_string()));
    }
    if hash.len() != 8
        || !hash
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err(SpecifierError::BadHash(hash.to_string()));
    }

    Ok(MdxModuleSpecifier {
        collection: collection.to_string(),
        slug: slug.to_string(),
        content_hash: hash.to_string(),
    })
}

/// Hash of the JSX source — first 8 lowercase hex chars of SHA-256.
///
/// Matches the dialect of `zfb-css::pipeline::hash_8`: same algorithm,
/// same width. The fixed width keeps generated specifiers a constant
/// length and makes them easy to spot in logs / dev tools.
fn hash_8(jsx_source: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(jsx_source.as_bytes());
    let digest = hasher.finalize();
    let full = hex::encode(digest);
    full[..8].to_string()
}

/// Derive `(collection, slug)` from a content-collection file path.
///
/// Convention: `<root>/<collection>/<slug>.<ext>`. We grab the immediate
/// parent directory name as the collection, and the file stem as the slug.
/// Falls back to `"_"` if either is missing — never panics, never returns
/// an empty segment (parser would reject those).
fn collection_and_slug(file_path: &Path) -> (String, String) {
    let collection = file_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("_")
        .to_string();
    let slug = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("_")
        .to_string();
    (collection, slug)
}

/// In-memory cache of compiled MDX modules, keyed by the SHA-256 of the
/// raw input source.
///
/// **Opt-in.** Callers that don't pass a `&MdxModuleCache` to
/// [`compile_mdx_to_jsx_module_cached`] get a fresh compilation every
/// call. This keeps unit tests hermetic — they never share state with
/// each other or with a long-running renderer process — and makes the
/// caching cost (memory + a hash lookup) a visible decision at the call
/// site.
///
/// The cache key is `sha256(input)` (full hex), NOT the 8-char content
/// hash on the output. Two different MDX bodies that happen to produce
/// JSX with a colliding 8-hex prefix are still distinct entries.
#[derive(Debug, Default)]
pub struct MdxModuleCache {
    inner: Mutex<HashMap<String, CompiledMdx>>,
}

impl MdxModuleCache {
    /// Construct an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of cached entries (mainly useful from tests).
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// True when the cache holds no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// Drop all cached entries.
    pub fn clear(&self) {
        self.lock().clear();
    }

    /// Lock-or-recover: a poisoned mutex still yields a valid guard via
    /// `into_inner`, so cache reads do not panic on a prior writer crash.
    fn lock(&self) -> MutexGuard<'_, HashMap<String, CompiledMdx>> {
        match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// Compile MDX source into a [`CompiledMdx`] in one call.
///
/// Equivalent to `compile_mdx_to_jsx_module_cached(input, file_path, None, None)`
/// — provided as a thin convenience so callers that don't want a cache
/// or a pipeline don't have to write the `None`s themselves.
///
/// # Errors
/// Forwards [`PipelineError::Parse`] from the underlying emitter.
pub fn compile_mdx_to_jsx_module(
    input: &str,
    file_path: &Path,
) -> Result<CompiledMdx, PipelineError> {
    compile_mdx_to_jsx_module_cached(input, file_path, None, None)
}

/// Compile MDX with optional in-memory caching keyed by `sha256(input)`,
/// optionally running a [`Pipeline`]'s mdast visitors before JSX emission.
///
/// When `cache` is `Some(_)` and the input has been compiled before, the
/// cached [`CompiledMdx`] is cloned out and returned without invoking the
/// emitter. When `cache` is `None`, every call compiles fresh.
///
/// The on-miss insert is best-effort: if two threads race on the same
/// input they may both compile (and the second insert simply overwrites
/// the first identical value). We don't hold the cache lock across
/// compilation to avoid serialising CPU work.
///
/// `pipeline` was added by Sub 46 (#46) so future callers can thread a
/// configured pipeline (with directive-registry-driven mdast visitors,
/// see Sub 4b #47) through the JSX path. Today's call sites all pass
/// `None`; behaviour is byte-for-byte identical to pre-Sub-46. When a
/// pipeline IS supplied, the cache is bypassed because the simple
/// `sha256(input)` key cannot distinguish identical inputs run through
/// different pipelines — Sub 4b will revisit cache keying alongside
/// real pipeline usage.
///
/// # Errors
/// Forwards [`PipelineError::Parse`] from the underlying emitter. A parse
/// failure is never cached — the next call will retry.
pub fn compile_mdx_to_jsx_module_cached(
    input: &str,
    file_path: &Path,
    cache: Option<&MdxModuleCache>,
    pipeline: Option<&mut Pipeline>,
) -> Result<CompiledMdx, PipelineError> {
    // Pipeline-driven transforms can change the JSX output for a given
    // input, but the existing cache keys only on `sha256(input)`. To
    // avoid aliasing, bypass the cache entirely when a pipeline is
    // supplied. Sub 4b will revisit this once real pipelines start
    // firing.
    let cache_for_lookup = if pipeline.is_some() { None } else { cache };

    // Cache lookup first — `sha256(input)` (full hex) is the key. Using
    // the full digest here, not the 8-char prefix, so distinct sources
    // that happen to share an 8-hex prefix on their *output* don't
    // clobber each other in the cache.
    let input_key = if cache_for_lookup.is_some() {
        let mut h = Sha256::new();
        h.update(input.as_bytes());
        Some(hex::encode(h.finalize()))
    } else {
        None
    };

    if let (Some(c), Some(key)) = (cache_for_lookup, input_key.as_ref()) {
        if let Some(hit) = c.lock().get(key) {
            return Ok(hit.clone());
        }
    }

    let (collection, slug) = collection_and_slug(file_path);

    let opts = MdxJsxOptions::default().with_filename(file_path.display().to_string());
    let jsx_source = mdx_to_jsx_module_inner(input, opts, pipeline)?;
    let content_hash = hash_8(&jsx_source);
    let specifier = format!("mdx://{collection}/{slug}#{content_hash}");

    let compiled = CompiledMdx {
        jsx_source,
        content_hash,
        specifier,
    };

    if let (Some(c), Some(key)) = (cache_for_lookup, input_key) {
        c.lock().insert(key, compiled.clone());
    }

    Ok(compiled)
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

    /// Three headings → 3-element `headings` array exported alongside
    /// `MDXContent`. This is the headline acceptance test for T4.
    #[test]
    fn three_headings_export_three_element_array() {
        let src = "# Intro\n\n## Setup\n\n## Usage\n";
        let out = emit(src);
        assert!(
            out.contains("export const headings = ["),
            "missing headings export: {out}"
        );
        // Each entry is a single-line `{ depth, slug, text }` object.
        assert!(
            out.contains("{ depth: 1, slug: \"intro\", text: \"Intro\" }"),
            "h1 entry missing: {out}"
        );
        assert!(
            out.contains("{ depth: 2, slug: \"setup\", text: \"Setup\" }"),
            "first h2 entry missing: {out}"
        );
        assert!(
            out.contains("{ depth: 2, slug: \"usage\", text: \"Usage\" }"),
            "second h2 entry missing: {out}"
        );
        // Sanity: exactly three commas at end of entry lines (one per
        // entry). Counting the per-entry trailing `},\n` lines avoids
        // accidentally matching `...components,\n`.
        let entry_terminators = out.matches(" },\n").count();
        assert_eq!(entry_terminators, 3, "expected 3 heading entries: {out}");
    }

    /// Empty document still emits an empty `headings = []` so callers
    /// can `import { headings } from './post.mdx'` unconditionally.
    #[test]
    fn empty_document_exports_empty_headings() {
        let out = emit("");
        assert!(
            out.contains("export const headings = [];"),
            "expected empty headings export: {out}"
        );
    }

    /// Slug parity contract with the rehype `heading_links` plugin:
    /// every emitted slug must equal what `slugify` (the same algorithm
    /// heading_links uses) produces from the heading's plain text. For
    /// repeated `<h2>`–`<h6>` titles, the `-1` / `-2` numbering must
    /// match too.
    #[test]
    fn heading_slugs_match_heading_links_algorithm() {
        use crate::plugins::heading_links::slugify;

        let src = "## Hello, World!\n\n## Hello, World!\n\n## Hello, World!\n";
        let out = emit(src);

        let base = slugify("Hello, World!");
        assert_eq!(base, "hello-world");
        // First occurrence: bare slug. Subsequent occurrences: `-1`,
        // `-2`. Same numbering scheme `HeadingLinksPlugin::next_slug`
        // applies to rendered `id` attributes.
        assert!(
            out.contains("slug: \"hello-world\","),
            "first slug missing: {out}"
        );
        assert!(
            out.contains("slug: \"hello-world-1\","),
            "second slug missing: {out}"
        );
        assert!(
            out.contains("slug: \"hello-world-2\","),
            "third slug missing: {out}"
        );
    }

    /// h1 stays out of the dedup pool because heading_links never sees
    /// it. So an `# A` followed by `## A` must produce slugs `a` and
    /// `a` (not `a-1`) — this is the only way the array's slug matches
    /// the rendered `<h2 id="a">`.
    #[test]
    fn h1_does_not_consume_dedup_counter() {
        let src = "# A\n\n## A\n";
        let out = emit(src);
        assert!(
            out.contains("{ depth: 1, slug: \"a\", text: \"A\" }"),
            "h1 should slug as 'a': {out}"
        );
        assert!(
            out.contains("{ depth: 2, slug: \"a\", text: \"A\" }"),
            "h2 must also slug as 'a' (parity with heading_links): {out}"
        );
    }

    /// Plain-text projection rule: inline marks (`**`, `_`, `~~`,
    /// inline code, links) are flattened to their text content. This
    /// matches the projection `heading_links` would feed into the
    /// slugger after the heading is rendered to hast.
    #[test]
    fn heading_text_uses_plain_text_projection() {
        let src = "## Hello **world**\n";
        let out = emit(src);
        assert!(
            out.contains("text: \"Hello world\""),
            "bold marks should flatten: {out}"
        );
        assert!(
            out.contains("slug: \"hello-world\""),
            "slug derives from flattened text: {out}"
        );
    }

    /// Inline code, links, and emphasis all contribute their inner
    /// text — same rule the rehype `heading_links` plugin applies.
    #[test]
    fn heading_text_flattens_inline_code_link_and_emphasis() {
        let src = "## use `npm` to [install](/x) it\n";
        let out = emit(src);
        assert!(
            out.contains("text: \"use npm to install it\""),
            "expected flat plain text: {out}"
        );
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

    /// Block math (`$$...$$`) emits a `<pre><code class="language-math
    /// math-display">…</code></pre>` shape with the LaTeX wrapped as a
    /// JS string literal — never as a bare `{\foo}` JSX expression.
    /// This is the headline regression test for zfb#93.
    #[test]
    fn block_math_emits_safe_jsx_with_display_class() {
        let src = "$$\n\\int_{-\\infty}^{\\infty} f(x)\\,dx\n$$\n";
        let out = emit(src);
        assert!(
            out.contains("language-math math-display"),
            "missing math-display class: {out}"
        );
        assert!(
            out.contains("<_components.pre><_components.code"),
            "math should route through _components.pre/code: {out}"
        );
        // The LaTeX body is wrapped in a JS string literal — the
        // backslashes must be doubled, not bare. If any bare `{\letter}`
        // pattern leaked through, the bundler heuristic
        // `jsx_likely_breaks_downstream_parser` would skip the bridge.
        assert!(
            out.contains("\\\\int_"),
            "expected backslash-escaped LaTeX inside JS string: {out}"
        );
        assert!(
            !out.contains("{\\int") && !out.contains("{-\\infty}"),
            "raw LaTeX leaked into JSX expression containers: {out}"
        );
    }

    /// Inline math (`$x$`) emits a single `<code class="language-math
    /// math-inline">…</code>` with the LaTeX as a JS string literal.
    #[test]
    fn inline_math_emits_safe_jsx_with_inline_class() {
        let src = "When $x \\to \\infty$ the limit holds.\n";
        let out = emit(src);
        assert!(
            out.contains("language-math math-inline"),
            "missing math-inline class: {out}"
        );
        assert!(
            out.contains("<_components.code className=\"language-math math-inline\">"),
            "inline math should route through _components.code with class: {out}"
        );
        // Same bare-backslash safety check as block math.
        assert!(
            !out.contains("{\\to") && !out.contains("{\\infty}"),
            "raw LaTeX leaked into JSX expression containers: {out}"
        );
    }

    /// Headings with inline math contribute the raw LaTeX as plain
    /// text in the `headings` projection. Without this, the slug for
    /// `## Limit as $x \\to \\infty$` would drop the math entirely
    /// (everything after `Limit as `), producing a slug that no longer
    /// matches what a runtime TOC consumer expects.
    #[test]
    fn heading_with_inline_math_keeps_latex_in_text_projection() {
        let src = "## Limit as $x \\to \\infty$\n";
        let out = emit(src);
        // The plain-text projection includes the raw LaTeX body.
        assert!(
            out.contains("text: \"Limit as x \\\\to \\\\infty\""),
            "expected LaTeX in heading text projection: {out}"
        );
    }
}
