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
//!   thematic break (hr), HTML literal (passed through), GFM pipe-table
//!   (`<table><thead>…</thead><tbody>…</tbody></table>` with per-column
//!   `style="text-align: …"` from the alignment array).
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
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};

use markdown::mdast::{AlignKind, AttributeContent, AttributeValue, Node as MdastNode};
use sha2::{Digest, Sha256};
use zfb_md_ast::diagnostics::{CollectingSink, MarkdownDiagnostic};
use zfb_md_ast::heading_registry::{HeadingEntry as RegistryHeadingEntry, HeadingRegistry};
use zfb_md_ast::{BuildContext, CrossFileLinkCandidate, FileHeadings};
use zfb_types::normalize_path_lexical;

use crate::dep_manifest::DependencyManifest;
#[cfg(test)]
use crate::footnotes::FOOTNOTE_LABEL_STYLE;
use crate::footnotes::{FootnoteEntry, FootnoteRef, FOOTNOTE_LABEL_ID};
use crate::pipeline::{
    code_block_hast, constructs_for_jsx_emit, mdast_to_hast_with, FootnoteRenderCtx, HastNode,
    HastVisitor, JsxEmitStrategy, Pipeline, PipelineError, ResolvedGfmConstructs,
};
use crate::plugins::heading_links::{slugify, HeadingIdStrategy, SlugAllocator};
use crate::plugins::BrokenLinkDiagnostic;

/// Options controlling the emitted JSX module.
#[derive(Debug, Clone)]
pub struct MdxJsxOptions {
    /// Display name / path used for parse-error diagnostics.
    pub filename: String,
    /// Absolute path of the source file being compiled, threaded into
    /// the per-file `BuildContext` when the supplied pipeline armed
    /// context threading via `Pipeline::set_build_context_roots`
    /// (zfb#944) — context-aware feature plugins (transclude,
    /// imageDimensions, linkValidation) resolve file-relative references
    /// against its parent directory. `None` (the default) leaves
    /// `BuildContext::source_path` unset; without armed roots it is
    /// ignored entirely.
    pub source_path: Option<PathBuf>,
}

impl Default for MdxJsxOptions {
    fn default() -> Self {
        Self {
            filename: "<anonymous>.mdx".to_string(),
            source_path: None,
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

    /// Set the source path threaded into the per-file `BuildContext`
    /// (zfb#944) — see the field docs on [`MdxJsxOptions::source_path`].
    #[must_use]
    pub fn with_source_path(mut self, source_path: impl Into<PathBuf>) -> Self {
        self.source_path = Some(source_path.into());
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
/// heading anchors, syntax highlighting, mermaid,
/// strip-md-ext, …) before reaching the JSX emitter — typically by
/// passing a [`Pipeline::with_defaults`] from the loader.
///
/// Implementation note (#121): the JSX path internally takes a hast
/// detour — `mdast → mdast visitors → mdast_to_hast → hast visitors →
/// hast→JSX walker` — so the same four hast plugins
/// `Pipeline::with_defaults` ships (heading-links, code-title,
/// mermaid, syntect) plus the opt-in strip-md-ext fire
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
/// mermaid, syntect) plus opt-in strip-md-ext fire on
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
    //
    // The GFM constructs come from the supplied pipeline (the bundler
    // / dev loader / snapshot bridge resolve them from
    // `zfb.config.ts#markdown.gfm` and thread them through). Without a
    // pipeline (the no-`pipeline` legacy entry point) we fall back to
    // the conservative default — strikethrough + table on, every other
    // GFM construct off — so the no-pipeline path doesn't silently
    // strip strikethrough either. Both paths route through
    // `constructs_for_jsx_emit` so math (`math_flow` / `math_text`)
    // stays hard-coded ON regardless of the GFM choice; math
    // constructs are not part of the new `markdown.gfm` config
    // surface, and the JSX emitter has dedicated arms for them
    // (zfb#93).
    let resolved_gfm: ResolvedGfmConstructs = pipeline
        .as_deref()
        .map(|p| p.gfm_constructs())
        .unwrap_or(ResolvedGfmConstructs::CONSERVATIVE);
    let parse_options = markdown::ParseOptions {
        constructs: constructs_for_jsx_emit(resolved_gfm),
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

    // Per-file BuildContext threading (zfb#944): when the pipeline armed
    // context roots (`Pipeline::set_build_context_roots`), both visitor
    // phases run through their `*_with_context` variants so the
    // context-aware feature plugins (transclude, imageDimensions,
    // linkValidation) actually fire — resolving file-relative references
    // against `opts.source_path` and reporting their reads to the
    // pipeline's recorder. Diagnostics they emit are collected into a
    // local sink and flushed into the pipeline's buffer afterwards so the
    // compile cache can store/replay them. Visitors that don't override
    // `visit_with_context` fall back to plain `visit` (the trait
    // default), so output is byte-identical for every other plugin.
    // Without armed roots this is all skipped — context-free behaviour,
    // byte-for-byte.
    //
    // Cache-safety of anchor verdicts: the per-compile-local registry
    // (seeded below from `collect_headings`) derives only from the
    // post-mdast tree of THIS compile — input bytes + config fingerprint
    // + transcluded content already covered by the read-recorder manifest.
    // Anchor verdicts are therefore pure functions of the cache key and
    // replay correctly. BUILD-scoped cross-file registry remains deferred
    // (#960).
    let context_roots = pipeline_mut.as_deref().and_then(|p| {
        p.build_context_roots()
            .map(|(root, public)| (root.to_path_buf(), public.to_path_buf()))
    });
    let mut context_sink = CollectingSink::new();
    // Per-compile-local registry for same-file anchor validation (#954).
    // Cross-file fragment verdicts degrade to existence-only here and are
    // recorded as candidates for the post-compile check (#960 / #977).
    let mut local_heading_registry = HeadingRegistry::new();
    // Per-compile candidate buffer (#977): collected locally and flushed
    // into the pipeline's side channel after the visitor chains run —
    // same discipline as `context_sink` — so the compile cache can slice
    // off exactly what THIS compile recorded.
    let mut local_cross_file_links: Vec<CrossFileLinkCandidate> = Vec::new();
    let mut build_ctx = context_roots.map(|(project_root, public_dir)| BuildContext {
        source_path: opts.source_path.clone(),
        project_root,
        public_dir,
        heading_registry: Some(&mut local_heading_registry),
        diagnostics: Some(&mut context_sink),
        cross_file_links: Some(&mut local_cross_file_links),
    });

    if let Some(p) = pipeline_mut.as_deref_mut() {
        match build_ctx.as_mut() {
            Some(ctx) => p.apply_mdast_visitors_with_context(&mut root, ctx),
            None => p.apply_mdast_visitors(&mut root),
        }
    }

    let all_children: Vec<MdastNode> = match root {
        MdastNode::Root(r) => r.children,
        // markdown-rs always produces a Root for to_mdast, but be
        // defensive — never panic on unexpected shape.
        other => vec![other],
    };

    // Partition out synthesized module-level exports injected by mdast
    // visitors (e.g. ReadingTimePlugin). These carry a `/* zfb-synth-export */`
    // marker so they can be distinguished from user-authored MDX ESM nodes
    // (which the emitter drops because the bundler handles them). Synthesized
    // exports are lifted to the module scope alongside `headings`.
    let (synth_exports, children): (Vec<MdastNode>, Vec<MdastNode>) =
        all_children.into_iter().partition(|n| {
            if let MdastNode::MdxjsEsm(esm) = n {
                esm.value.contains("/* zfb-synth-export */")
            } else {
                false
            }
        });

    // Heading metadata is collected from the post-mdast-visitor tree
    // either way — hast plugins like `HeadingLinksPlugin` mutate the
    // hast (slug → `id` attribute, anchor child) but the slug
    // computation in `collect_headings` shares the same `SlugAllocator`
    // so the emitted `headings` array stays in lockstep with
    // the rendered `<hN id="…">`. Custom hast visitors that rewrite
    // heading text or remove headings would diverge — none ship with
    // `Pipeline::with_defaults()` today; if a future plugin needs to
    // rewrite headings, this collection should move post-hast.
    //
    // The heading-ID strategy comes from the pipeline (set by
    // `with_defaults_and_full_config` from `features.headingIds`,
    // zfb#871). No pipeline → flat, matching the legacy constructors.
    let strategy = pipeline_mut
        .as_deref()
        .map(|p| p.heading_id_strategy())
        .unwrap_or_default();
    let CollectedHeadings {
        entries: headings,
        nested_slugs,
    } = collect_headings(&children, strategy);

    // Seed the per-compile registry from collect_headings' canonical
    // walk: it covers JSX-nested headings (rendered ids stamped by
    // jsx_render_child) that hast-phase HeadingLinksPlugin never sees.
    // h1 entries are seeded too: a top-level h1 renders NO id, so its
    // anchor passes silently rather than false-positively (h1 inside a
    // JSX body DOES render the id, and the slug here matches it).
    //
    // The same canonical entry set is surfaced as the per-file headings
    // side channel (#960 / #977) so the post-compile cross-file anchor
    // check sees exactly what a local registry lookup would have seen.
    // Gated on `linkValidation` being enabled — the channel exists
    // solely for that check, and configs without it must record nothing.
    // Recorded even when the file has zero headings: "compiled with zero
    // headings" is a meaningful verdict, distinct from "never compiled".
    // The path is normalised with the shared
    // `zfb_types::normalize_path_lexical` helper — the key contract
    // `CrossFileLinkCandidate::target_path` documents.
    let record_file_headings = pipeline_mut
        .as_deref()
        .is_some_and(Pipeline::link_validation_enabled);
    let mut compiled_file_headings: Option<FileHeadings> = None;
    if let Some(ctx) = build_ctx.as_mut() {
        if let (Some(reg), Some(src)) =
            (ctx.heading_registry.as_deref_mut(), ctx.source_path.clone())
        {
            // Mark the file tracked before seeding individual headings so a
            // document with zero headings still gets a `Some(&[])` entry —
            // `LinkValidationPlugin` then validates its bare `#anchor` links
            // as broken instead of silently skipping (zfb#1093). Mirrors the
            // `HeadingLinksPlugin::visit_with_context` mark on the
            // run_with_context path; keeps both compile paths in lockstep.
            reg.mark_tracked(src.clone());
            let mut channel_entries: Vec<RegistryHeadingEntry> = Vec::new();
            for h in &headings {
                if !h.slug.is_empty() {
                    let entry = RegistryHeadingEntry {
                        id: h.slug.clone(),
                        text: h.text.clone(),
                        depth: h.depth,
                    };
                    if record_file_headings {
                        channel_entries.push(entry.clone());
                    }
                    reg.insert(src.clone(), entry);
                }
            }
            if record_file_headings {
                compiled_file_headings = Some(FileHeadings {
                    source_path: normalize_path_lexical(&src),
                    headings: channel_entries,
                });
            }
        }
    }
    // (`headings` is only borrowed above; its later use for the export is unaffected.
    // HeadingLinksPlugin's `visit_with_context` will ALSO insert top-level h2–h6 entries
    // during the hast pass — exact duplicates by the slug-lockstep invariant; harmless.)

    let (body, html_tags, component_names, hoisted_esm) = if take_hast_detour {
        // Wrap children back into a Root so `mdast_to_hast_with` can
        // recurse through them as a single node — its public signature
        // takes `&MdastNode`.
        let mdast_root = MdastNode::Root(markdown::mdast::Root {
            children,
            position: None,
        });
        // Use the JSX-aware strategy so MDX JSX bodies preserve
        // markdown formatting recursively (`<Note>**bold**</Note>` →
        // `<Note><strong>bold</strong></Note>`). The HTML serializer
        // path keeps using the lossy fallback to preserve its
        // long-standing snapshot output.
        //
        // Nested-render context: the strategy callback fires once per
        // top-level MDX JSX node, in document order, with no shared
        // mutable state (it is a `&dyn Fn`). We give it interior
        // mutability via a `Cell<usize>` cursor over `nested_slugs` —
        // the document-order list of slugs `collect_headings` assigned
        // to JSX-nested headings. `jsx_render_child` pops the next slug
        // each time it renders a `MdastNode::Heading`, so the `id` it
        // stamps matches the TOC export exactly. The cursor persists
        // across all top-level JSX nodes because the closure captures
        // it by ref.
        //
        // The ctx also OWNS the nested-code render chain (#2207),
        // fetched here through `&Pipeline` — an owned chain on the ctx
        // is what lets the strategy closure highlight JSX-nested fences
        // without borrowing the pipeline, which `apply_hast_visitors`
        // below still needs mutably.
        let slug_ctx = NestedRenderCtx {
            nested_slugs: &nested_slugs,
            cursor: std::cell::Cell::new(0),
            code_chain: pipeline_mut
                .as_deref()
                .and_then(Pipeline::nested_code_render_chain)
                .map(std::cell::RefCell::new),
        };
        let strategy_fn = |node: &MdastNode, fc: &FootnoteRenderCtx<'_>| -> String {
            jsx_raw_recursive(node, &slug_ctx, fc)
        };
        let strategy = JsxEmitStrategy::JsxPath(&strategy_fn);
        let mut hast = mdast_to_hast_with(&mdast_root, &strategy);
        // The emitter must have consumed every nested-heading slug in
        // lockstep with `collect_headings`. A mismatch means the emit
        // walk order drifted from `walk_collect_headings`' descent set.
        debug_assert_eq!(
            slug_ctx.cursor.get(),
            nested_slugs.len(),
            "jsx_render_child consumed {} nested-heading slugs but collect_headings recorded {}",
            slug_ctx.cursor.get(),
            nested_slugs.len(),
        );
        if let Some(p) = pipeline_mut.as_deref_mut() {
            match build_ctx.as_mut() {
                Some(ctx) => p.apply_hast_visitors_with_context(&mut hast, ctx),
                None => p.apply_hast_visitors(&mut hast),
            }
        }
        let mut bridge = HastJsxBridge::new();
        let body = bridge.emit_root(&hast);
        (
            body,
            bridge.html_tags,
            bridge.component_names,
            bridge.hoisted_esm,
        )
    } else {
        let mut emitter = JsxEmitter::new();
        let body = emitter.emit_children_block(&children);
        // The no-pipeline JsxEmitter path is byte-stable and does not hoist
        // hast-phase JsxRaw nodes (it never runs hast visitors). Return an
        // empty Vec so the tuple shape is uniform.
        (body, emitter.html_tags, emitter.component_names, Vec::new())
    };

    // Flush context-plugin diagnostics — and the #977 side channels
    // (cross-file link candidates, per-file headings) — into the
    // pipeline's buffers (zfb#944) so the compile cache can slice off
    // what THIS compile appended, and call sites can drain via
    // `Pipeline::take_markdown_diagnostics` /
    // `take_cross_file_link_candidates` / `take_file_headings`. The
    // explicit drop releases the sink/buffer borrows held by the context.
    drop(build_ctx);
    let context_diags = context_sink.take();
    if let Some(p) = pipeline_mut {
        if !context_diags.is_empty() {
            p.extend_markdown_diagnostics(context_diags);
        }
        if !local_cross_file_links.is_empty() {
            p.extend_cross_file_link_candidates(local_cross_file_links);
        }
        if let Some(fh) = compiled_file_headings {
            p.extend_file_headings(vec![fh]);
        }
    }

    let mut out = String::new();
    out.push_str("import { Fragment as _Fragment } from \"react/jsx-runtime\";\n\n");
    out.push_str(&render_headings_export(&headings));
    out.push('\n');
    // Emit synthesized module-level exports (e.g. readingTimeMinutes).
    // These were injected by mdast visitors and partitioned out above.
    // Strip the `/* zfb-synth-export */` marker prefix before emitting.
    for esm_node in &synth_exports {
        if let MdastNode::MdxjsEsm(esm) = esm_node {
            // The marker is always at the start; strip it plus the space after.
            let export_stmt = esm
                .value
                .trim_start_matches("/* zfb-synth-export */")
                .trim_start();
            out.push_str(export_stmt);
            out.push('\n');
        }
    }
    if !synth_exports.is_empty() {
        out.push('\n');
    }
    // Emit module-level ESM statements hoisted from hast-phase JsxRaw nodes
    // (e.g. `export const toc = …` from TocExportPlugin). These were
    // collected by HastJsxBridge::emit_root and must appear at column 0,
    // at module scope, before `function _createMdxContent`.
    for stmt in &hoisted_esm {
        out.push_str(stmt);
        out.push('\n');
    }
    if !hoisted_esm.is_empty() {
        out.push('\n');
    }
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

/// Result of the single canonical heading walk.
///
/// `entries` drives the `export const headings = […]` TOC array.
/// `nested_slugs` lists — in document order — the slug assigned to each
/// heading that lives *inside* an MDX JSX element body. The JSX emitter
/// (`jsx_render_child`) replays the same document-order walk and pops
/// these slugs in sequence so the `id` it stamps on a nested
/// `<_components.hN>` is byte-identical to the slug the TOC recorded.
///
/// Both lists come from ONE walk against ONE `seen` dedup map, so the
/// numbering can never drift between the TOC export and the rendered
/// nested heading.
struct CollectedHeadings {
    entries: Vec<HeadingEntry>,
    nested_slugs: Vec<String>,
}

/// Walk the parsed mdast and collect every heading in document order.
///
/// Slugs match what [`crate::plugins::heading_links`] would emit for
/// the same document and strategy: the walk shares the plugin's
/// [`SlugAllocator`], so flat mode gets the same `slugify` + `-1`, `-2`,
/// … numbering, and hierarchical mode (zfb#871) gets the same
/// ancestor-prefixed candidates. Per heading_links semantics, only
/// `<h2>`–`<h6>` participate in the allocator; `<h1>` slugs are emitted
/// raw.
///
/// ## Dedup ordering across the two render passes
///
/// On the production hast-detour path, top-level headings stay real
/// hast `<hN>` elements and get their `id` from `HeadingLinksPlugin`
/// (its own `SlugAllocator`). Headings nested inside an MDX JSX body are
/// rendered to an opaque JsxRaw string before hast visitors run, so
/// `HeadingLinksPlugin` never sees them — `jsx_render_child` must stamp
/// their `id` instead. This walk is the single source of truth that
/// sees BOTH, in document order, so the TOC export is always internally
/// consistent and the nested-heading `id`s match the TOC.
///
/// Known asymmetry (out of scope for #477): because
/// `HeadingLinksPlugin` keeps an independent `seen` map, a top-level
/// heading's rendered `id` reflects a top-level-only count. If a
/// JSX-nested heading shares its text and appears *earlier* in document
/// order, this walk's combined count and HeadingLinksPlugin's
/// top-level-only count can disagree for that top-level heading.
/// Reconciling it would require seeding HeadingLinksPlugin's `seen` from
/// this walk through the pipeline/bridge — invasive and not required by
/// the issue (its acceptance case is top-level-first). The common case
/// (top-level heading before any same-text nested heading) is exact.
///
/// The hierarchical strategy has the analogous asymmetry: a JSX-nested
/// heading joins this walk's ancestor stack but not the plugin's, so a
/// *top-level* heading that is deeper than a preceding JSX-nested one
/// gets a different prefix in the two passes. Same root cause, same
/// accepted scope — plain-markdown outlines (the zfb#871 consumer's
/// case) never hit it.
fn collect_headings(children: &[MdastNode], strategy: HeadingIdStrategy) -> CollectedHeadings {
    let mut out = CollectedHeadings {
        entries: Vec::new(),
        nested_slugs: Vec::new(),
    };
    let mut slugs = SlugAllocator::new(strategy);
    // `in_jsx == false` at the top level; flips to true once we descend
    // into an MDX JSX element body so we can route those headings' slugs
    // into `nested_slugs` for the emitter to replay.
    walk_collect_headings(children, &mut out, &mut slugs, false);
    out
}

fn walk_collect_headings(
    nodes: &[MdastNode],
    out: &mut CollectedHeadings,
    slugs: &mut SlugAllocator,
    in_jsx: bool,
) {
    for node in nodes {
        match node {
            MdastNode::Heading(h) => {
                let depth = h.depth.clamp(1, 6);
                let text = mdast_inline_text(&h.children);
                let base = slugify(&text);
                // For h2-h6 we must mirror heading_links' SlugAllocator
                // exactly so `headings[i].slug` matches the rendered
                // `<hN id="…">`. h1 is left out of the allocator
                // because heading_links never sees it.
                let slug = if depth == 1 {
                    base
                } else {
                    slugs.allocate(depth, &base)
                };
                // A heading inside an MDX JSX body is emitted by
                // `jsx_render_child`, not by HeadingLinksPlugin — record
                // its slug so the emitter can stamp the matching `id`.
                // Empty-text headings still consume a slot (the emitter
                // visits them too) but carry an empty slug → no `id`,
                // mirroring HeadingLinksPlugin's skip-empty behaviour.
                if in_jsx {
                    out.nested_slugs.push(slug.clone());
                }
                out.entries.push(HeadingEntry { depth, slug, text });
            }
            // Headings can legally nest inside blockquotes / list items,
            // so descend into block-level containers. We deliberately do
            // NOT descend into Paragraph/Heading children themselves —
            // headings cannot appear there.
            MdastNode::Root(r) => walk_collect_headings(&r.children, out, slugs, in_jsx),
            MdastNode::Blockquote(b) => walk_collect_headings(&b.children, out, slugs, in_jsx),
            MdastNode::List(l) => walk_collect_headings(&l.children, out, slugs, in_jsx),
            MdastNode::ListItem(li) => walk_collect_headings(&li.children, out, slugs, in_jsx),
            // Descend into MDX JSX element bodies (e.g. `<Outro>## …`).
            // Everything below here is rendered by `jsx_render_child`, so
            // flip `in_jsx` on so the slugs land in `nested_slugs`. The
            // descent set below MUST stay in lockstep with the container
            // arms `jsx_render_child` recurses through, or the emitter's
            // pop order will drift from this walk (debug_assert in
            // `mdx_to_jsx_module*` guards against that).
            MdastNode::MdxJsxFlowElement(j) => {
                walk_collect_headings(&j.children, out, slugs, true);
            }
            MdastNode::MdxJsxTextElement(j) => {
                walk_collect_headings(&j.children, out, slugs, true);
            }
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
            MdastNode::ListItem(li) => {
                self.html_tags.insert("li".to_string());
                let inner = match li.checked {
                    Some(checked) => {
                        self.html_tags.insert("input".to_string());
                        // The checkbox is hosted by the item's leading
                        // paragraph, so `p` is emitted here even though
                        // the `Paragraph` arm never runs for it.
                        self.html_tags.insert("p".to_string());
                        task_list_item_jsx(&li.children, checked, |nodes| {
                            self.emit_inline_children(nodes)
                        })
                    }
                    None => self.emit_inline_children(&li.children),
                };
                format!("<_components.li>{inner}</_components.li>")
            }
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
            MdastNode::MdxFlowExpression(e) => emit_mdx_expression_braced(&e.value),
            MdastNode::MdxTextExpression(e) => emit_mdx_expression_braced(&e.value),
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
            // GFM pipe-table: map first row to <thead><tr><th>…</th></tr></thead>
            // and remaining rows to <tbody><tr><td>…</td></tr></tbody>.
            // Per-column alignment from `align[]` is applied as
            // `style="text-align: left|right|center"` on each th/td.
            MdastNode::Table(t) => {
                for tag in ["table", "thead", "tbody", "tr", "th", "td"] {
                    self.html_tags.insert(tag.to_string());
                }
                emit_table_jsx(self, &t.children, &t.align)
            }
            // Unhandled node kinds (footnotes, definitions,
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

/// Map a [`markdown::mdast::AlignKind`] to the CSS `text-align` value
/// string, or `None` when the column has no alignment hint.
fn align_style(align: &AlignKind) -> Option<&'static str> {
    match align {
        AlignKind::Left => Some("left"),
        AlignKind::Right => Some("right"),
        AlignKind::Center => Some("center"),
        AlignKind::None => None,
    }
}

/// Emit a GFM pipe-table as nested `_components.*` JSX elements.
///
/// Shape emitted:
/// ```text
/// <_components.table>
///   <_components.thead><_components.tr>
///     <_components.th style="text-align: left">…</_components.th>
///   </_components.tr></_components.thead>
///   <_components.tbody>
///     <_components.tr>
///       <_components.td>…</_components.td>
///     </_components.tr>
///   </_components.tbody>
/// </_components.table>
/// ```
///
/// Matches the canonical shape from zfb#136 / issue #193.
fn emit_table_jsx(emitter: &mut JsxEmitter, rows: &[MdastNode], align: &[AlignKind]) -> String {
    let mut out = String::new();

    // Build a style attr string for column index `col`. Object-valued,
    // matching the other two emit paths — a string `style` prop makes
    // React throw. See `jsx_style_attr`.
    let style_attr = |col: usize| -> String {
        align
            .get(col)
            .and_then(align_style)
            .map(|v| jsx_style_attr(&format!("text-align: {v}")))
            .unwrap_or_default()
    };

    // Emit a single row as <tr><th|td …>…</th|td></tr>.
    let emit_row = |emitter: &mut JsxEmitter, row: &MdastNode, cell_tag: &str| -> String {
        let MdastNode::TableRow(tr) = row else {
            return String::new();
        };
        let mut row_out = String::new();
        row_out.push_str("<_components.tr>");
        for (col, cell) in tr.children.iter().enumerate() {
            let MdastNode::TableCell(tc) = cell else {
                continue;
            };
            let style = style_attr(col);
            let inner = emitter.emit_inline_children(&tc.children);
            row_out.push_str(&format!(
                "<_components.{cell_tag}{style}>{inner}</_components.{cell_tag}>"
            ));
        }
        row_out.push_str("</_components.tr>");
        row_out
    };

    out.push_str("<_components.table>");

    // First row → <thead>.
    if let Some(head_row) = rows.first() {
        out.push_str("<_components.thead>");
        out.push_str(&emit_row(emitter, head_row, "th"));
        out.push_str("</_components.thead>");
    }

    // Remaining rows → <tbody>.
    let body_rows = if rows.len() > 1 { &rows[1..] } else { &[] };
    if !body_rows.is_empty() {
        out.push_str("<_components.tbody>");
        for row in body_rows {
            out.push_str(&emit_row(emitter, row, "td"));
        }
        out.push_str("</_components.tbody>");
    }

    out.push_str("</_components.table>");
    out
}

/// Returns `true` if `s` is a module-level ESM statement that must appear
/// at column 0 in the emitted JS module.
///
/// Input domain (why the covered set is deliberately bounded): the ONLY
/// strings reaching this classifier are the payloads of
/// hast-plugin-injected `HastNode::JsxRaw` nodes (see
/// [`HastJsxBridge::emit_root`]). User-authored MDX ESM never reaches here —
/// it is partitioned out and DROPPED upstream (the bundler handles it), and
/// synthesized `/* zfb-synth-export */` nodes take a separate emit path. The
/// only shipped injector is `zfb-md-extras`'s `TocExportPlugin`, which emits
/// `export const toc = …`; the broader prefix list below is defensive
/// headroom for future zfb-internal hast plugins that inject other
/// value/re-export/import forms.
///
/// COVERED forms (each prefix requires a trailing space / `{` / `*` so a
/// JsxRaw node that merely mentions "export" in a comment is not
/// mis-hoisted):
/// - `export const` / `export let` / `export var` — variable exports
/// - `export function` / `export async function` — NON-generator function
///   exports (`export default function`/`… async function` are covered by
///   the `export default` prefix below)
/// - `export class` — class export (`export default class` likewise covered
///   by `export default`)
/// - `export default` — default export
/// - `export {` — re-export / named-export shorthand
/// - `export *` — star re-export (`export * from "mod"`,
///   `export * as ns from "mod"`)
/// - `import ` — import declaration (trailing space avoids matching
///   `importFoo`)
///
/// INTENTIONALLY NOT covered (no shipped or planned hast plugin injects
/// them, so classifying them would be dead code): generator exports
/// (`export function* …` / `export async function* …` — note there is no
/// space after `function`, so the function prefixes above deliberately miss
/// them) and TS type-only exports (`export type` / `export interface` /
/// `export enum`, erased before runtime). If a future hast plugin ever
/// injects one of these at module level, add its prefix here AND extend the
/// test — otherwise the declaration would be emitted indented inside the
/// Fragment body instead of hoisted to column 0.
///
/// The declaration-keyword prefixes (`export let`/`var`/`function`/`async
/// function`/`class`) mirror the list in
/// `zfb_diagnostics::locate_export_ident` (`crates/zfb-diagnostics/src/lib.rs`
/// ~:360-370) — that function's own doc comment notes `export * from` is
/// deliberately unmatched THERE because a star re-export has no local
/// binding name to locate; this function has no such constraint (it only
/// asks "is this a module-level statement", not "where is `ident`
/// declared"), so `export *` is matched here too.
fn is_module_level_esm(s: &str) -> bool {
    let trimmed = s.trim_start();
    trimmed.starts_with("export const ")
        || trimmed.starts_with("export let ")
        || trimmed.starts_with("export var ")
        || trimmed.starts_with("export function ")
        || trimmed.starts_with("export async function ")
        || trimmed.starts_with("export class ")
        || trimmed.starts_with("export default ")
        || trimmed.starts_with("export {")
        || trimmed.starts_with("export * ")
        || trimmed.starts_with("import ")
}

/// Walks a [`HastNode`] tree and emits JSX source — the post-#121
/// counterpart to [`JsxEmitter`].
///
/// The mdast→hast detour means hast plugins (heading-links, syntect,
/// mermaid, code-title, strip-md-ext) have already
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
    /// Module-level ESM statements hoisted out of the Fragment body.
    ///
    /// When `emit_root` sees a top-level `HastNode::JsxRaw` whose payload
    /// is a module-level ESM statement (`export const`, `export default`,
    /// `export {`, or `import `), it collects the payload here instead of
    /// placing it inside the `<_Fragment>` body (which would indent it ~6
    /// spaces and make MDX parsers treat it as content rather than a
    /// module declaration). The outer assembler emits these at column 0,
    /// before `function _createMdxContent`, so the emitted module is valid
    /// ESM regardless of which hast plugin injected the node.
    hoisted_esm: Vec<String>,
}

impl HastJsxBridge {
    fn new() -> Self {
        Self {
            html_tags: std::collections::BTreeSet::new(),
            component_names: std::collections::BTreeSet::new(),
            hoisted_esm: Vec::new(),
        }
    }

    /// Emit the body of `<_Fragment>`. Each top-level child of `Root`
    /// goes on its own line so the generated module stays readable.
    ///
    /// Top-level `HastNode::JsxRaw` nodes that contain a module-level ESM
    /// statement (`export const`, `export default`, `export {`, `import `)
    /// are collected into `self.hoisted_esm` instead of being placed in the
    /// Fragment body — those declarations must appear at column 0 in the
    /// emitted module, not indented inside JSX. The caller retrieves them
    /// via `self.hoisted_esm` and emits them before `_createMdxContent`.
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
            // Detect root-level JsxRaw nodes that carry a module-level ESM
            // statement and hoist them to module scope instead of emitting
            // them indented inside the Fragment body. This is necessary
            // because MDX (and tools like esbuild) require `export`/`import`
            // declarations at column 0 to recognise them as module-level ESM
            // rather than treating them as content.
            if let HastNode::JsxRaw(s) = child {
                if is_module_level_esm(s) {
                    self.hoisted_esm.push(s.trim_end().to_string());
                    continue;
                }
            }
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
                } else if starts_with_block_level_tag(s) {
                    // Block-level raw HTML (e.g. <pre>, <table>, <div>, <ul>) cannot
                    // be wrapped in a <span> per HTML5 content model. Use <div> so
                    // preact-render-to-string emits <div>…</div>, which is flow
                    // content and may contain block elements.
                    format!(
                        "<div dangerouslySetInnerHTML={{{{__html: {}}}}} />",
                        js_string_literal(s),
                    )
                } else {
                    // Inline raw HTML stays in a <span> — same shape as today.
                    format!(
                        "<span dangerouslySetInnerHTML={{{{__html: {}}}}} />",
                        js_string_literal(s),
                    )
                }
            }
            // JSX-shaped passthrough (MDX components, `{…}` expressions)
            // — embed verbatim, after recording any PascalCase
            // identifiers so the module preamble can declare them and
            // any `<_components.<tag>>` lowercase routes so the default
            // `_components` map gets the fallback string for that tag.
            HastNode::JsxRaw(s) => {
                collect_jsx_component_names(s, &mut self.component_names);
                collect_components_tag_names(s, &mut self.html_tags);
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
/// `MermaidPlugin`, `data-footnote-ref=""` by the footnote emitter) emit
/// as `attr=""` to keep the attribute present and serializable.
///
/// The one exception is an empty-valued **HTML boolean attribute** —
/// [`is_html_boolean_attr`] — which emits BARE (JSX `true`). hast has no
/// way to spell a bare attribute (its values are plain `String`s), so the
/// HTML serializer's `disabled=""` is the only spelling available on that
/// side; carrying the empty string straight into JSX would hand
/// React/Preact a falsy prop, and a task-list checkbox would hydrate
/// enabled and unchecked. Data attributes keep `=""` — they are ordinary
/// strings, and `true` would serialize as the visibly different
/// `data-footnote-ref="true"`.
fn render_hast_attrs(attrs: &[(String, String)]) -> String {
    if attrs.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for (k, v) in attrs {
        if k == "style" {
            out.push_str(&jsx_style_attr(v));
            continue;
        }
        out.push(' ');
        out.push_str(k);
        if v.is_empty() && is_html_boolean_attr(k) {
            continue;
        }
        out.push('=');
        out.push_str(&jsx_string_attr(v));
    }
    out
}

/// Render a CSS declaration string as a JSX `style` **object** prop:
/// ` style={{"position": "absolute", …}}`. Returns the empty string when
/// nothing parses, so the attribute is omitted entirely.
///
/// React throws outright on a string-valued `style` prop ("The `style`
/// prop expects a mapping from style properties to values, not a string"),
/// which would take down every page carrying one. hast stores attribute
/// values as plain `String`s, so the CSS text has to be converted here, at
/// the JSX boundary — the HTML serializer keeps writing the string form
/// untouched.
///
/// Hyphenated property names are camelCased (`white-space` → `whiteSpace`)
/// because React ignores the hyphenated spelling with a warning. Custom
/// properties (`--shiki-dark-bg`, emitted by `SyntectPlugin`'s dual-theme
/// mode) keep their exact name — both React and Preact read those verbatim.
///
/// Splitting is deliberately simple: `;` separates declarations and the
/// FIRST `:` separates name from value. That is sufficient for every
/// declaration this crate emits (footnote label hiding, table
/// `text-align`, syntect colors) and for ordinary author CSS. A value
/// containing a literal `;` — a `url(data:…;base64,…)` — would split
/// wrongly; no producer here emits one.
fn jsx_style_attr(css: &str) -> String {
    let mut props: Vec<String> = Vec::new();
    for decl in css.split(';') {
        let decl = decl.trim();
        if decl.is_empty() {
            continue;
        }
        let Some((name, value)) = decl.split_once(':') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() || value.is_empty() {
            continue;
        }
        let key = if name.starts_with("--") {
            name.to_string()
        } else {
            camel_case_css_property(name)
        };
        props.push(format!(
            "{}: {}",
            js_string_literal(&key),
            js_string_literal(value)
        ));
    }
    if props.is_empty() {
        return String::new();
    }
    format!(" style={{{{{}}}}}", props.join(", "))
}

/// `white-space` → `whiteSpace`. Leaves an already-camelCase or
/// single-word name untouched.
fn camel_case_css_property(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper_next = false;
    for ch in name.chars() {
        if ch == '-' {
            upper_next = true;
        } else if upper_next {
            out.extend(ch.to_uppercase());
            upper_next = false;
        } else {
            out.push(ch);
        }
    }
    out
}

/// True for HTML attributes whose presence alone means "true" and which
/// React/Preact expose as boolean props.
///
/// Only consulted for EMPTY-valued attributes (see [`render_hast_attrs`]);
/// an explicit value is always preserved verbatim.
///
/// Deliberately NOT the full HTML boolean-attribute set. The list is the
/// two attributes this crate itself synthesizes with an empty value
/// (`task_list_checkbox_hast`). `plugins::directives` also records an
/// author-written valueless attribute as `key=""`, so widening this list
/// (to `open`, `hidden`, …) would change directive output that is outside
/// the scope this was added for — a deliberate call, not an oversight.
fn is_html_boolean_attr(name: &str) -> bool {
    matches!(name, "checked" | "disabled")
}

/// Return true if the trimmed-leading raw HTML string begins with a
/// block-level element tag. Detection is purely lexical — it inspects
/// the first opening tag and matches against a static block-element
/// allowlist. No HTML parsing.
fn starts_with_block_level_tag(s: &str) -> bool {
    // Block-level elements per HTML5 content model. Conservative list
    // — anything not on it falls through to <span> (the inline default).
    const BLOCK_TAGS: &[&str] = &[
        "address",
        "article",
        "aside",
        "blockquote",
        "details",
        "dialog",
        "div",
        "dl",
        "dt",
        "dd",
        "fieldset",
        "figcaption",
        "figure",
        "footer",
        "form",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "header",
        "hgroup",
        "hr",
        "li",
        "main",
        "nav",
        "ol",
        "p",
        "pre",
        "section",
        "summary",
        "table",
        "thead",
        "tbody",
        "tfoot",
        "tr",
        "td",
        "th",
        "ul",
    ];
    let trimmed = s.trim_start();
    let bytes = trimmed.as_bytes();
    if bytes.first() != Some(&b'<') {
        return false;
    }
    let after_lt = &trimmed[1..];
    let tag_end = after_lt
        .find(|c: char| [' ', '>', '/', '\t', '\n', '\r'].contains(&c))
        .unwrap_or(after_lt.len());
    let tag = after_lt[..tag_end].to_ascii_lowercase();
    BLOCK_TAGS.contains(&tag.as_str())
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

/// Scan a JSX-shaped string for `<_components.<tag>` opening-tag
/// references and add the lowercase tag names to `out`. The
/// JSX-text recursive renderer (`jsx_element_text`) routes lowercase
/// MDX JSX tags through `_components.<tag>` so callers can override
/// them, but because that emission happens inside a JsxRaw payload
/// (the bridge cannot intercept it), the bridge has to harvest the
/// tag set after the fact so the module preamble emits the matching
/// default fallback (`tag: "tag"`) in the `_components` map.
///
/// We do not try to be a full JSX parser — the rule is:
///   - find a `<_components.` prefix;
///   - the next chars are alphanumeric / `_` / `$` (HTML tag names
///     never contain `.`, so the head identifier ends at the first
///     non-identifier character);
///   - skip empty matches.
fn collect_components_tag_names(jsx: &str, out: &mut std::collections::BTreeSet<String>) {
    const PREFIX: &str = "<_components.";
    let bytes = jsx.as_bytes();
    let mut i = 0;
    while let Some(off) = jsx[i..].find(PREFIX) {
        let start = i + off + PREFIX.len();
        let mut j = start;
        while j < bytes.len() {
            let c = bytes[j];
            if c.is_ascii_alphanumeric() || c == b'_' || c == b'$' {
                j += 1;
            } else {
                break;
            }
        }
        if j > start {
            out.insert(jsx[start..j].to_string());
        }
        i = j.max(start + 1);
    }
}

/// Per-document context threaded through the JSX-text recursive
/// renderer (the [`JsxEmitStrategy::JsxPath`] strategy) — nested-heading
/// slugs plus the nested-code render chain.
///
/// `nested_slugs` is the document-order list of slugs `collect_headings`
/// assigned to headings that live inside an MDX JSX element body, and
/// `cursor` is the running index into it. Each time `jsx_render_child`
/// renders a `MdastNode::Heading` it pops `nested_slugs[cursor]` and
/// advances — so the `id` it stamps is byte-identical to the slug the
/// TOC export recorded. The renderer's heading-visit order matches
/// `walk_collect_headings`' descent order (both are mdast document-order
/// walks reaching the same heading set), so the pop sequence aligns.
///
/// `code_chain` is the OWNED code-block hast chain
/// ([`Pipeline::nested_code_render_chain`], #2207) used to highlight a
/// fenced code block nested inside an MDX JSX body exactly like a
/// top-level fence. Owning the chain here — instead of borrowing the
/// pipeline inside the strategy closure — is what avoids a borrow
/// conflict with the later `apply_hast_visitors(&mut Pipeline)` call in
/// `mdx_to_jsx_module_inner`. `None` when the pipeline exposes no chain
/// (no syntect configured): the `Code` arm then keeps its byte-stable
/// fallback emission.
///
/// `Cell` / `RefCell` give interior mutability without `unsafe` /
/// `thread_local`: the `&dyn Fn` strategy callback cannot carry `&mut`
/// state, but a `&Cell` / `&RefCell` captured by the closure can. The
/// `RefCell` borrow is non-reentrant by construction — the chain
/// visitors are pure hast walkers that never call back into this
/// renderer.
struct NestedRenderCtx<'a> {
    nested_slugs: &'a [String],
    cursor: std::cell::Cell<usize>,
    code_chain: Option<std::cell::RefCell<Vec<Box<dyn HastVisitor>>>>,
}

impl NestedRenderCtx<'_> {
    /// Pop the slug for the next nested heading in document order.
    ///
    /// Returns `None` once the precomputed list is exhausted — a guard
    /// against over-walking (the `debug_assert` at the call site checks
    /// the inverse, that every slug was consumed).
    fn next_heading_slug(&self) -> Option<&str> {
        let i = self.cursor.get();
        let slug = self.nested_slugs.get(i).map(String::as_str);
        if slug.is_some() {
            self.cursor.set(i + 1);
        }
        slug
    }
}

/// Render one JSX-nested fenced code block through the pipeline's
/// code-block hast chain and the shared [`HastJsxBridge`] node emitter,
/// so nested output converges byte-for-byte with a top-level fence's
/// (#2207).
///
/// Steps mirror the top-level path exactly:
///
/// 1. Build the SAME mini-hast node a top-level fence gets, via the
///    shared [`code_block_hast`] constructor —
///    `<pre><code class="language-{lang}" data-lang data-meta>Text</code></pre>`
///    — including the `data-lang` / `data-meta` attributes the legacy
///    fallback emission drops (they are what the chain plugins key on).
/// 2. Run the owned chain in the documented ordering contract
///    (code-title → mermaid → syntect → code-enrichment) against a
///    detached `Root` wrapper, since every chain plugin rewrites
///    matching nodes among the CHILDREN of the node it visits.
/// 3. Emit the rewritten children through a [`HastJsxBridge`] — the same
///    `emit_node` code top-level blocks go through — and embed the
///    result in the surrounding JsxRaw payload. The local bridge's
///    harvested `html_tags` / `component_names` are deliberately
///    dropped: the outer bridge re-harvests both from the final JsxRaw
///    payload string (`collect_components_tag_names` /
///    `collect_jsx_component_names` in its `JsxRaw` arm), exactly as it
///    already does for every other `<_components.…>` reference this
///    renderer emits. `hoisted_esm` cannot arise here — only
///    `emit_root` hoists, and no chain plugin injects module-level ESM.
///
/// Returns `None` — caller falls back to the byte-stable legacy
/// emission — when the ctx carries no chain, or when the chain left the
/// mini-tree UNTOUCHED (no `data-lang` to key on, or a swallowed
/// per-block highlight error): for an untouched tree the bridge would
/// emit the code text as a `{"…"}` JS string literal whose contents are
/// invisible to `jsx_text_escape`'s `<`-escaping, and there is nothing
/// to gain over the established fallback bytes.
fn nested_code_via_chain(c: &markdown::mdast::Code, ctx: &NestedRenderCtx) -> Option<String> {
    let chain = ctx.code_chain.as_ref()?;
    let mini = HastNode::Root {
        children: vec![code_block_hast(c)],
    };
    let mut rewritten = mini.clone();
    for visitor in chain.borrow_mut().iter_mut() {
        visitor.visit(&mut rewritten);
    }
    if rewritten == mini {
        return None;
    }
    let HastNode::Root { children } = &rewritten else {
        // Defensive: no chain plugin replaces the Root itself.
        return None;
    };
    let mut bridge = HastJsxBridge::new();
    Some(children.iter().map(|n| bridge.emit_node(n)).collect())
}

/// Build the `id="…"` attribute plus trailing hash-link anchor for a
/// nested heading, mirroring [`crate::plugins::heading_links`] so a
/// JSX-nested `<_components.hN>` renders identically to a top-level one.
///
/// `text` is the same plain-text projection `collect_headings` recorded
/// for the heading (`mdast_inline_text`), so the `aria-label` matches
/// what HeadingLinksPlugin would emit. An empty slug (empty-text
/// heading) yields no id / no anchor, mirroring HeadingLinksPlugin's
/// skip-empty behaviour. Returns `(attrs, anchor_jsx)`.
fn nested_heading_id_and_anchor(slug: &str, text: &str) -> (String, String) {
    if slug.is_empty() {
        return (String::new(), String::new());
    }
    // Use `escape_attr_literal` (not `jsx_attr_escape`) and the verbatim
    // `class` attribute name so a nested anchor is byte-identical to a
    // top-level one: the top-level path renders the same
    // `HeadingLinksPlugin::anchor` hast node through `render_hast_attrs`
    // → `jsx_string_attr` → `escape_attr_literal`, keeping `class` as-is
    // (see `render_hast_attrs` docs). The lowercase `a` routes through
    // `_components.a` exactly as the bridge's `emit_element` would.
    let attrs = format!(" id=\"{}\"", escape_attr_literal(slug));
    // Empty-body anchor; the `#` glyph is a CSS `::after`. Mirrors
    // `HeadingLinksPlugin::anchor`'s href / class / aria-label shape and
    // attribute order.
    let anchor = format!(
        "<_components.a href=\"#{}\" class=\"hash-link\" aria-label=\"Direct link to {}\"></_components.a>",
        escape_attr_literal(slug),
        escape_attr_literal(text),
    );
    (attrs, anchor)
}

/// JSX-text recursive renderer plugged into `mdast_to_hast_with` via
/// [`JsxEmitStrategy::JsxPath`]. Produces JSX-shaped source for the
/// MDX JSX, MDX expression, and remark-math arms while preserving
/// markdown formatting inside MDX bodies (`<Note>**bold**</Note>` →
/// `<Note><strong>bold</strong></Note>`).
///
/// The HTML serializer path keeps using `pipeline::reconstruct_jsx`'s
/// flat-text fallback so existing snapshots stay byte-stable. Picking
/// recursion only on the JSX path is safe because the bridge already
/// embeds the resulting JsxRaw payload verbatim — JSX accepts plain
/// HTML tags inside MDX JSX bodies.
fn jsx_raw_recursive(
    node: &MdastNode,
    ctx: &NestedRenderCtx,
    fc: &FootnoteRenderCtx<'_>,
) -> String {
    match node {
        MdastNode::MdxJsxFlowElement(j) => {
            jsx_element_text(j.name.as_deref(), &j.attributes, &j.children, ctx, fc)
        }
        MdastNode::MdxJsxTextElement(j) => {
            jsx_element_text(j.name.as_deref(), &j.attributes, &j.children, ctx, fc)
        }
        MdastNode::MdxFlowExpression(e) => emit_mdx_expression_braced(&e.value),
        MdastNode::MdxTextExpression(e) => emit_mdx_expression_braced(&e.value),
        // Defensive — `mdast_to_hast_with`'s strategy callback only
        // fires for the JSX-shaped arms above, but if the contract
        // ever changes we fall back to the recursive child renderer
        // rather than dropping the node.
        other => jsx_render_child(other, ctx, fc),
    }
}

fn jsx_element_text(
    name: Option<&str>,
    attrs: &[AttributeContent],
    children: &[MdastNode],
    ctx: &NestedRenderCtx,
    fc: &FootnoteRenderCtx<'_>,
) -> String {
    let attrs_str = render_jsx_attrs(attrs);
    // Choose the open/close JSX tag name. `name == None` is the MDX
    // fragment shorthand `<></>` — emit `_Fragment` so the JSX parser
    // accepts it (a bare `< />` is invalid). PascalCase names go through
    // verbatim; lowercase HTML tags route through `_components.<tag>`
    // so callers can override them via the `components` prop, matching
    // `JsxEmitter::emit_jsx`'s contract on the non-pipeline path.
    let (open_name, close_name) = match name {
        None | Some("") => ("_Fragment".to_string(), "_Fragment".to_string()),
        Some(n) if is_component_identifier(n) => (n.to_string(), n.to_string()),
        Some(n) => (format!("_components.{n}"), format!("_components.{n}")),
    };
    if children.is_empty() {
        // Self-close PascalCase / `_components.<tag>` and emit an
        // explicit empty `_Fragment` body so SWC accepts the result —
        // `<_Fragment />` is fine, but keeping the symmetric pattern
        // matches `JsxEmitter::emit_jsx`'s output.
        return format!("<{open_name}{attrs_str} />");
    }
    let inner: String = children
        .iter()
        .map(|c| jsx_render_child(c, ctx, fc))
        .collect();
    format!("<{open_name}{attrs_str}>{inner}</{close_name}>")
}

/// Recursively render an mdast child as JSX-shaped source.
///
/// Mirrors the coverage of [`mdast_to_hast`](crate::pipeline::mdast_to_hast)
/// — every node kind that has a meaningful HTML rendering produces a
/// matching JSX tag. Markdown nodes living inside an MDX JSX block (e.g.
/// a `## heading` or a list inside `<Outro>…</Outro>`) route through
/// `_components.<tag>` so callers can override `<p>`/`<h2>`/etc. via the
/// `components` prop, matching `JsxEmitter::emit_jsx`'s contract on the
/// non-pipeline path. These tags are emitted into a JsxRaw payload; the
/// bridge's `collect_components_tag_names` scan (see `emit_node` for the
/// `HastNode::JsxRaw` arm) harvests the `<_components.<tag>` references
/// afterwards so the module preamble registers each tag's default
/// fallback in the `_components` map.
fn jsx_render_child(node: &MdastNode, ctx: &NestedRenderCtx, fc: &FootnoteRenderCtx<'_>) -> String {
    match node {
        MdastNode::Text(t) => jsx_text_escape(&t.value),
        MdastNode::Html(h) => h.value.clone(),
        MdastNode::MdxFlowExpression(e) => emit_mdx_expression_braced(&e.value),
        MdastNode::MdxTextExpression(e) => emit_mdx_expression_braced(&e.value),
        MdastNode::MdxJsxFlowElement(j) => {
            jsx_element_text(j.name.as_deref(), &j.attributes, &j.children, ctx, fc)
        }
        MdastNode::MdxJsxTextElement(j) => {
            jsx_element_text(j.name.as_deref(), &j.attributes, &j.children, ctx, fc)
        }
        MdastNode::Paragraph(p) => jsx_wrap_children("p", "", &p.children, ctx, fc),
        MdastNode::Heading(h) => {
            let depth = h.depth.clamp(1, 6);
            // This heading lives inside an MDX JSX body, so
            // HeadingLinksPlugin (a hast visitor) never sees it — stamp
            // the slug `id` + hash-link anchor here instead. The slug
            // comes from `collect_headings`' canonical document-order
            // walk via the cursor, so it matches the TOC export and the
            // dedup numbering of any top-level headings. `next_heading_slug`
            // returns `None` only if the precomputed list is exhausted
            // (the debug_assert at the strategy call site guards that);
            // fall back to no id/anchor rather than panicking in release.
            let slug = ctx.next_heading_slug().unwrap_or("");
            let text = mdast_inline_text(&h.children);
            let (id_attr, anchor) = nested_heading_id_and_anchor(slug, &text);
            let tag = format!("h{depth}");
            let inner: String = h
                .children
                .iter()
                .map(|c| jsx_render_child(c, ctx, fc))
                .collect();
            format!("<_components.{tag}{id_attr}>{inner}{anchor}</_components.{tag}>")
        }
        MdastNode::Emphasis(e) => jsx_wrap_children("em", "", &e.children, ctx, fc),
        MdastNode::Strong(s) => jsx_wrap_children("strong", "", &s.children, ctx, fc),
        MdastNode::Delete(d) => jsx_wrap_children("del", "", &d.children, ctx, fc),
        MdastNode::InlineCode(c) => format!(
            "<_components.code>{}</_components.code>",
            jsx_text_escape(&c.value),
        ),
        // Fenced code inside an MDX JSX body / directive (#2207): route
        // through the pipeline's code-block chain + the shared bridge
        // emitter so the fence is highlighted exactly like a top-level
        // one (full parity: title wrapper, mermaid div, syntect
        // markup, enrichment attributes). Fallback — no chain on the
        // ctx, or the chain left the block untouched — keeps the
        // established byte-stable unhighlighted emission below;
        // exact-substring tests depend on those bytes.
        MdastNode::Code(c) => match nested_code_via_chain(c, ctx) {
            Some(rendered) => rendered,
            None => {
                let mut attrs = String::new();
                if let Some(lang) = &c.lang {
                    attrs.push_str(&format!(" class=\"language-{}\"", jsx_attr_escape(lang)));
                }
                format!(
                    "<_components.pre><_components.code{attrs}>{}</_components.code></_components.pre>",
                    jsx_text_escape(&c.value),
                )
            }
        },
        MdastNode::Link(l) => {
            let mut attrs = format!(" href=\"{}\"", jsx_attr_escape(&l.url));
            if let Some(title) = &l.title {
                attrs.push_str(&format!(" title=\"{}\"", jsx_attr_escape(title)));
            }
            format!(
                "<_components.a{attrs}>{}</_components.a>",
                l.children
                    .iter()
                    .map(|c| jsx_render_child(c, ctx, fc))
                    .collect::<String>(),
            )
        }
        MdastNode::Image(i) => {
            let mut attrs = format!(
                " src=\"{}\" alt=\"{}\"",
                jsx_attr_escape(&i.url),
                jsx_attr_escape(&i.alt),
            );
            if let Some(title) = &i.title {
                attrs.push_str(&format!(" title=\"{}\"", jsx_attr_escape(title)));
            }
            format!("<_components.img{attrs} />")
        }
        MdastNode::List(l) => {
            let tag = if l.ordered { "ol" } else { "ul" };
            let mut attrs = String::new();
            if l.ordered {
                if let Some(start) = l.start {
                    if start != 1 {
                        attrs.push_str(&format!(" start={{{start}}}"));
                    }
                }
            }
            format!(
                "<_components.{tag}{attrs}>{}</_components.{tag}>",
                l.children
                    .iter()
                    .map(|c| jsx_render_child(c, ctx, fc))
                    .collect::<String>(),
            )
        }
        MdastNode::ListItem(li) => {
            let render = |nodes: &[MdastNode]| -> String {
                nodes.iter().map(|c| jsx_render_child(c, ctx, fc)).collect()
            };
            let inner = match li.checked {
                Some(checked) => task_list_item_jsx(&li.children, checked, render),
                None => render(&li.children),
            };
            format!("<_components.li>{inner}</_components.li>")
        }
        MdastNode::Blockquote(b) => jsx_wrap_children("blockquote", "", &b.children, ctx, fc),
        MdastNode::ThematicBreak(_) => "<_components.hr />".to_string(),
        MdastNode::Break(_) => "<_components.br />".to_string(),
        MdastNode::Math(m) => format!(
            "<_components.pre><_components.code class=\"language-math math-display\">{}</_components.code></_components.pre>",
            jsx_text_escape(&m.value),
        ),
        MdastNode::InlineMath(m) => format!(
            "<_components.code class=\"language-math math-inline\">{}</_components.code>",
            jsx_text_escape(&m.value),
        ),
        MdastNode::Root(r) => r
            .children
            .iter()
            .map(|c| jsx_render_child(c, ctx, fc))
            .collect(),
        // GFM pipe-table inside MDX JSX body — routes table tags through
        // `_components.<tag>`, matching the non-pipeline `emit_table_jsx`.
        MdastNode::Table(t) => jsx_render_table(t, ctx, fc),
        // GFM footnotes (issue #2023/#2025/#2027) nested inside an MDX
        // JSX element body. This is a SEPARATE recursive descent from
        // `pipeline::mdast_to_hast_inner`'s (this one is only reached
        // for nodes inside an `MdxJsxFlowElement`/`MdxJsxTextElement`
        // body), but `FootnoteModel::collect` already walked the WHOLE
        // document tree — including JSX element bodies — when building
        // the shared model, so an identifier referenced only in here
        // still has an entry. `fc` is the SAME `FootnoteRenderCtx` (and
        // therefore the same cursor) `mdast_to_hast_with`'s main walk
        // is using, threaded down through `JsxEmitStrategy::JsxPath`'s
        // closure — see that type's doc comment — so occurrences claim
        // in the correct combined document order rather than a second,
        // independently-advancing count that could drift from the main
        // walk's.
        //
        // A `FootnoteDefinition` never renders in place here either,
        // matching `pipeline::mdast_to_hast_inner`'s arm — its body only
        // appears once, in the collected section `mdast_to_hast_with`
        // appends at the end of the document.
        MdastNode::FootnoteDefinition(_) => String::new(),
        // A `FootnoteReference` claims its next occurrence from the
        // shared cursor, mirroring `pipeline::footnote_reference_marker`'s
        // shape but as JSX text routed through `_components.<tag>`, and
        // reusing the SAME `FootnoteEntry`/`FootnoteRef` data the model
        // already computed (ids, numbering, escaping policy) rather than
        // re-deriving any of it here. `next_reference` returning `None`
        // is not reachable through the public parse API (see the
        // pipeline.rs arm's comment for why), so this degrades to
        // nothing rather than panicking.
        MdastNode::FootnoteReference(r) => fc
            .next_reference(&r.identifier)
            .map(|(entry, footnote_ref)| jsx_footnote_reference_marker(entry, footnote_ref))
            .unwrap_or_default(),
        // ESM, frontmatter, reference-style link/image definitions,
        // etc. drop silently here — better than leaking a `Debug` repr.
        _ => String::new(),
    }
}

/// `<sup><a href="#{definition id}" id="{occurrence id}" data-footnote-ref=""
/// aria-describedby="footnote-label">{number}</a></sup>`, routed through
/// `_components.<tag>` like every other synthesized HTML tag in this
/// file — the JSX-child-renderer's counterpart to
/// `pipeline::footnote_reference_marker`.
///
/// `data-footnote-ref=""` — an empty-valued attribute, NOT the bare
/// boolean-attribute shorthand this file uses elsewhere. Bare means `true`
/// in JSX, which serializes as `data-footnote-ref="true"`, whereas the
/// hast bridge emits `data-footnote-ref=""` for the identical marker at
/// the document's top level. Two footnotes on one `.mdx` page would then
/// differ purely by whether one happened to sit inside a JSX component's
/// children — exactly the divergence epic #2021 exists to remove. The
/// number renders via
/// `js_string_literal_in_braces` (not plain text) so a document-level
/// reference and one nested inside an MDX JSX element body produce the
/// SAME `{"N"}` shape `HastJsxBridge::emit_node`'s `HastNode::Text` arm
/// already produces for the top-level path — the acceptance criterion
/// this file's tests pin is that `.md` and `.mdx` (and, here, JSX-nested
/// vs. top-level within one `.mdx`) never visibly diverge.
fn jsx_footnote_reference_marker(entry: &FootnoteEntry, footnote_ref: &FootnoteRef) -> String {
    format!(
        "<_components.sup><_components.a href=\"{}\" id=\"{}\" data-footnote-ref=\"\" aria-describedby=\"{}\">{}</_components.a></_components.sup>",
        jsx_attr_escape(&entry.href()),
        jsx_attr_escape(&footnote_ref.id),
        jsx_attr_escape(FOOTNOTE_LABEL_ID),
        js_string_literal_in_braces(&footnote_ref.number.to_string()),
    )
}

/// Render a GFM pipe-table as `_components.<tag>`-routed JSX.
///
/// Used by [`jsx_render_child`] for tables that appear inside MDX JSX
/// element bodies. Table tags route through `_components.<tag>` so
/// callers can override them, matching the non-pipeline `emit_table_jsx`.
/// The resulting string is embedded as a [`HastNode::JsxRaw`] payload;
/// the bridge's `collect_components_tag_names` scan registers each tag.
fn jsx_render_table(
    t: &markdown::mdast::Table,
    ctx: &NestedRenderCtx,
    fc: &FootnoteRenderCtx<'_>,
) -> String {
    let style_attr = |col: usize| -> String {
        // Object-valued, matching the hast bridge's own `style` handling —
        // a string `style` prop makes React throw. See `jsx_style_attr`.
        t.align
            .get(col)
            .and_then(align_style)
            .map(|v| jsx_style_attr(&format!("text-align: {v}")))
            .unwrap_or_default()
    };

    let emit_row = |row: &MdastNode, cell_tag: &str| -> String {
        let MdastNode::TableRow(tr) = row else {
            return String::new();
        };
        let mut out = String::from("<_components.tr>");
        for (col, cell) in tr.children.iter().enumerate() {
            let MdastNode::TableCell(tc) = cell else {
                continue;
            };
            let style = style_attr(col);
            let inner: String = tc
                .children
                .iter()
                .map(|c| jsx_render_child(c, ctx, fc))
                .collect();
            out.push_str(&format!(
                "<_components.{cell_tag}{style}>{inner}</_components.{cell_tag}>"
            ));
        }
        out.push_str("</_components.tr>");
        out
    };

    let mut out = String::from("<_components.table>");
    if let Some(head_row) = t.children.first() {
        out.push_str("<_components.thead>");
        out.push_str(&emit_row(head_row, "th"));
        out.push_str("</_components.thead>");
    }
    let body_rows = if t.children.len() > 1 {
        &t.children[1..]
    } else {
        &[]
    };
    if !body_rows.is_empty() {
        out.push_str("<_components.tbody>");
        for row in body_rows {
            out.push_str(&emit_row(row, "td"));
        }
        out.push_str("</_components.tbody>");
    }
    out.push_str("</_components.table>");
    out
}

/// Render the (disabled) task-list checkbox JSX literal that opens a
/// `ListItem`'s content when `ListItem.checked` is `Some(_)` (issue
/// #2024, epic #2021), followed by a single `{" "}` separating it from
/// the label. GFM task-list checkboxes are static, server-rendered
/// markup with no client-side toggle handler — always `disabled`;
/// `checked` is present only when the item itself is checked. Routes
/// through `_components.input`, like every other synthesized HTML tag in
/// this file. Shared by both JSX-emit `ListItem` arms
/// (`JsxEmitter::emit_node` and `jsx_render_child`).
///
/// Attribute spelling, ORDER (`type`, `disabled`, `checked`) and the
/// trailing spacer all match what `pipeline::task_list_checkbox_hast` +
/// `prepend_task_list_checkbox` produce for the same item, so a task list
/// serializes identically whether it sits at a document's top level or
/// inside a JSX component's children. `disabled`/`checked` stay BARE
/// (JSX `true`) rather than `=""`: they are real HTML boolean attributes,
/// and an empty string is falsy as a React/Preact prop — a `checked=""`
/// checkbox hydrates unchecked and enabled. `render_hast_attrs` applies
/// the same rule when bridging the hast path's empty-valued
/// `disabled`/`checked` into JSX.
fn task_list_checkbox_jsx(checked: bool) -> String {
    let checked_attr = if checked { " checked" } else { "" };
    format!("<_components.input type=\"checkbox\" disabled{checked_attr} />{{\" \"}}")
}

/// Wrap a task-list item's rendered content so the checkbox reads as a
/// checkbox BESIDE its label rather than on its own line above it.
///
/// The JSX-emit counterpart of `pipeline::prepend_task_list_checkbox` —
/// see that function for the full rationale. Neither emit path unwraps
/// tight-list paragraphs, so a task item's children start with a
/// `Paragraph`; the checkbox goes INSIDE that paragraph. An item that
/// starts with something else (a nested list, a code block) keeps the
/// plain prefix placement, since there is no inline context to join.
///
/// `render` turns a slice of mdast nodes into JSX text. It is a closure
/// because `JsxEmitter`'s call site needs `&mut self` while
/// `jsx_render_child`'s is a free function.
fn task_list_item_jsx<R>(children: &[MdastNode], checked: bool, mut render: R) -> String
where
    R: FnMut(&[MdastNode]) -> String,
{
    let checkbox = task_list_checkbox_jsx(checked);
    match children.split_first() {
        Some((MdastNode::Paragraph(p), rest)) => format!(
            "<_components.p>{checkbox}{}</_components.p>{}",
            render(&p.children),
            render(rest),
        ),
        _ => format!("{checkbox}{}", render(children)),
    }
}

fn jsx_wrap_children(
    tag: &str,
    attrs: &str,
    children: &[MdastNode],
    ctx: &NestedRenderCtx,
    fc: &FootnoteRenderCtx<'_>,
) -> String {
    format!(
        "<_components.{tag}{attrs}>{}</_components.{tag}>",
        children
            .iter()
            .map(|c| jsx_render_child(c, ctx, fc))
            .collect::<String>(),
    )
}

/// Escape characters that would terminate / re-open JSX syntax inside
/// element bodies (`<` / `>` / `{` / `}`). `&` is left alone — JSX
/// accepts HTML entities verbatim and authors may use them on
/// purpose.
fn jsx_text_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '{' => out.push_str("&#123;"),
            '}' => out.push_str("&#125;"),
            other => out.push(other),
        }
    }
    out
}

/// Escape characters that must not appear inside a JSX `"…"` attribute
/// value (`&` / `<` / `>` / `"`). Mirrors `escape_attr_literal` above
/// but lives here so the recursive renderer is self-contained.
fn jsx_attr_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
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
                        out.push_str(&emit_mdx_expression_braced(&e.value));
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

/// Emit an MDX expression node as a JSX `{…}` fragment, recovering the
/// one shape that silently degrades a whole page (zfb#1729).
///
/// MDX expression nodes (`MdxFlowExpression` / `MdxTextExpression`) and
/// expression-valued attributes are emitted verbatim so that valid MDX
/// expressions — `{1 + 1}`, `count={1 + 2}`, spreads `{...rest}`, JSX
/// comments `{/* … */}` — reach the downstream JSX/TSX compiler intact.
/// But when an expression's value begins with a backslash-escape (`\n`,
/// `\d`, …) it renders a bare `{\letter}` fragment. That is never a
/// valid *bare* JS expression, and it is the exact shape the bundler's
/// `jsx_likely_breaks_downstream_parser` gate rejects — a single such
/// fragment makes esbuild reject the module and the bundler degrade the
/// ENTIRE page to the `<pre data-zfb-content-fallback>` shape.
///
/// So: if emitting `{value}` verbatim would trip that gate, recover the
/// value as a JS string literal (`{"…escaped…"}`) instead. The bytes
/// stay visibly present and the module parses. Only the breaking shape
/// is recovered — [`expression_fragment_breaks_downstream_parser`] is
/// string/comment-aware, so every valid expression, spread, comment,
/// and numeric attribute stays `false` and is emitted verbatim.
fn emit_mdx_expression_braced(value: &str) -> String {
    let verbatim = format!("{{{value}}}");
    // The scanner is only a byte-pattern heuristic: it visits `{\letter}`
    // bytes even inside a regex literal (e.g. `{/[{\d}]/.test(x)}`), which
    // are perfectly valid JS. Stringifying such an expression would
    // silently change runtime behavior, so only recover when the value is
    // genuinely NOT valid JS (the real `\d`-leak class). A valid-but-
    // gate-tripping expression stays verbatim — it may still trip the real
    // bundler gate downstream, reproducing the conservative pre-epic
    // whole-page fallback for that page, which is the correct outcome (the
    // gate is out of scope; never silently change valid-JS semantics).
    if expression_fragment_breaks_downstream_parser(&verbatim) && !mdx_expression_is_valid_js(value)
    {
        js_string_literal_in_braces(value)
    } else {
        verbatim
    }
}

/// True iff `value` (the inner content of an MDX `{…}` expression) parses
/// as a single valid JS/TSX expression.
///
/// MDX expression bodies occupy a single expression position, so the value
/// is wrapped in parentheses (`(value);`) and parsed as a module: a valid
/// body yields exactly one expression statement whose expression is the
/// wrapping parenthesis. Anything else — a parse failure, recovered parser
/// errors, trailing tokens, or injected extra statements — is treated as
/// invalid. Used by [`emit_mdx_expression_braced`] to distinguish a
/// genuinely-broken `{\d}`-style leak (invalid → recover as a string) from
/// a valid expression the byte scanner falsely flagged (e.g. a regex
/// literal containing `{\d}` bytes → leave verbatim).
fn mdx_expression_is_valid_js(value: &str) -> bool {
    use swc_core::common::sync::Lrc;
    use swc_core::common::{FileName, SourceMap};
    use swc_core::ecma::ast::{EsVersion, Expr, ModuleItem, Stmt};
    use swc_core::ecma::parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};

    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(FileName::Anon.into(), format!("({value});"));
    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax {
            tsx: true,
            decorators: false,
            dts: false,
            no_early_errors: false,
            disallow_ambiguous_jsx_like: false,
        }),
        EsVersion::Es2022,
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    let Ok(module) = parser.parse_module() else {
        return false;
    };
    // SWC's parser is error-recovering: it can return `Ok` on invalid input
    // while stashing recoverable diagnostics. Reject those too.
    if !parser.take_errors().is_empty() {
        return false;
    }
    matches!(
        module.body.as_slice(),
        [ModuleItem::Stmt(Stmt::Expr(stmt))] if matches!(*stmt.expr, Expr::Paren(_))
    )
}

/// Local mirror of `zfb_build::bundler::jsx_likely_breaks_downstream_parser`
/// applied to a single emitted JSX fragment.
///
/// `zfb-build` depends on `zfb-content` (not the reverse), so the gate
/// cannot be imported here; this is the same string/line-comment/
/// block-comment-aware scan for a `{` (optionally `-`) directly followed
/// by `\` + an ASCII letter — the byte pattern a leaked string-escape
/// produces outside any JS string. **Keep in sync with the gate in
/// `crates/zfb-build/src/bundler.rs`.** A visible divergence is caught
/// by the `heuristic_says_jsx_breaks` mirror in
/// `crates/zfb-content/tests/large_mdx_fallback_regression.rs` and the
/// gate's own unit tests in zfb-build.
fn expression_fragment_breaks_downstream_parser(jsx: &str) -> bool {
    let bytes = jsx.as_bytes();
    let mut in_string: Option<u8> = None;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];

        if in_line_comment {
            if c == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if in_block_comment {
            if c == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                in_block_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }

        if let Some(q) = in_string {
            if c == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if c == q {
                in_string = None;
            }
            i += 1;
            continue;
        }

        if c == b'/' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'/' => {
                    in_line_comment = true;
                    i += 2;
                    continue;
                }
                b'*' => {
                    in_block_comment = true;
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }

        if c == b'"' || c == b'\'' || c == b'`' {
            in_string = Some(c);
            i += 1;
            continue;
        }

        if c == b'{' {
            let mut j = i + 1;
            if j < bytes.len() && bytes[j] == b'-' {
                j += 1;
            }
            if j + 1 < bytes.len() && bytes[j] == b'\\' && bytes[j + 1].is_ascii_alphabetic() {
                return true;
            }
        }

        i += 1;
    }
    false
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

/// Maximum number of entries the [`MdxModuleCache`] retains before
/// evicting the entire map.
///
/// **Policy: clear-all on overflow.** When an insertion would push the
/// entry count past this threshold the whole map is cleared first, then
/// the new entry is inserted.  This is the mechanically-safest bounded
/// policy: it is O(1) to decide, requires no ordering structure, and is
/// trivially correct — after a clear the invariant `len ≤ CAP` holds
/// again.  The trade-off is a one-time cold-compile burst when the cap
/// is hit, which is preferable to unbounded memory growth in a long
/// `zfb dev` session.
///
/// 4 096 entries: a compiled `CompiledMdx` is roughly 2–8 KiB of JSX
/// source.  At the generous end that is ~32 MiB for the whole map —
/// well within a normal dev-server budget — while still bounding
/// worst-case accumulation from edit churn across a large content tree.
const MDX_MODULE_CACHE_CAP: usize = 4_096;

/// One cache slot: the compiled output plus the broken-link
/// diagnostics that compile produced (zfb#939) plus the markdown
/// diagnostics its context-aware feature plugins emitted (zfb#944) plus
/// the dependency manifest of external reads its feature plugins
/// reported (zfb#942).
///
/// Diagnostics are a side channel — call sites compile, then drain
/// [`Pipeline::take_broken_links`] / `take_markdown_diagnostics` — so a
/// cache hit must replay the stored vecs back into the pipeline;
/// without that, every hit would silently swallow the file's
/// broken-link reports and cross-file findings. Pipelines without the
/// corresponding plugin/context always store empty vecs.
///
/// The manifest gates every hit: lookup re-probes each recorded
/// dependency against the current filesystem
/// ([`DependencyManifest::still_valid`]) and falls back to a full
/// recompile (which re-records) on any change — edited, deleted, or
/// newly-created-where-missing files, and any probe failure. Pipelines
/// without a read-recorder always store an empty manifest, which
/// validates trivially — zero behaviour or cost change for them.
#[derive(Debug, Clone)]
struct CachedMdxModule {
    compiled: CompiledMdx,
    broken_links: Vec<BrokenLinkDiagnostic>,
    markdown_diagnostics: Vec<MarkdownDiagnostic>,
    /// Cross-file fragment-link candidates this compile recorded
    /// (#960 / #977) — store/replay symmetric with
    /// `markdown_diagnostics`. Empty for unarmed pipelines.
    cross_file_links: Vec<CrossFileLinkCandidate>,
    /// Per-file heading records this compile surfaced (#960 / #977) —
    /// at most one for a single compile. Empty for unarmed pipelines.
    file_headings: Vec<FileHeadings>,
    dependencies: DependencyManifest,
}

/// In-memory cache of compiled MDX modules, keyed by the SHA-256 of the
/// raw input source plus the supplied pipeline's config fingerprint
/// (plus, for resolve-links pipelines, a per-call `source_dir` context
/// segment — see [`Pipeline::cache_key_context`]).
///
/// **Opt-in.** Callers that don't pass a `&MdxModuleCache` to
/// [`compile_mdx_to_jsx_module_cached`] get a fresh compilation every
/// call. This keeps unit tests hermetic — they never share state with
/// each other or with a long-running renderer process — and makes the
/// caching cost (memory + a hash lookup) a visible decision at the call
/// site.
///
/// The cache key is `sha256(input)` (full hex, NOT the 8-char content
/// hash on the output — two different MDX bodies whose *output* shares
/// an 8-hex prefix are still distinct entries) joined with the
/// [`Pipeline::config_fingerprint`] of the supplied pipeline (or a
/// fixed `no-pipeline` token), so identical inputs compiled under
/// different pipeline configs never alias one entry. Pipelines without
/// a fingerprint (manually mutated / carrying filesystem-reading
/// feature plugins — see [`Pipeline::config_fingerprint`]) bypass the
/// cache entirely.
#[derive(Debug, Default)]
pub struct MdxModuleCache {
    inner: Mutex<HashMap<String, CachedMdxModule>>,
}

/// Process-wide cache instance backing [`MdxModuleCache::process_global`].
static PROCESS_GLOBAL_MDX_MODULE_CACHE: LazyLock<MdxModuleCache> =
    LazyLock::new(MdxModuleCache::default);

impl MdxModuleCache {
    /// Construct an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The process-wide shared cache.
    ///
    /// The bundler's three MDX compile sites and the content-snapshot
    /// walker all reuse this one instance, so a dev process that runs
    /// snapshot + bundle every tick compiles each unchanged
    /// `(input, pipeline-config)` pair exactly once and serves every
    /// later tick from memory. The map is bounded at
    /// [`MDX_MODULE_CACHE_CAP`] entries: when an insertion would exceed
    /// the cap the entire map is cleared first (clear-on-overflow
    /// policy), so memory is always bounded even across a long edit
    /// session with high churn.
    ///
    /// Safe to share between configs because every entry is keyed by
    /// the pipeline config fingerprint (see the type-level docs);
    /// per-path output (the `mdx://` specifier) is re-derived on every
    /// hit by [`compile_mdx_to_jsx_module_cached`].
    #[must_use]
    pub fn process_global() -> &'static MdxModuleCache {
        &PROCESS_GLOBAL_MDX_MODULE_CACHE
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
    fn lock(&self) -> MutexGuard<'_, HashMap<String, CachedMdxModule>> {
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

/// Compile MDX with optional in-memory caching keyed by
/// `sha256(input)` + the pipeline's config fingerprint, optionally
/// running a [`Pipeline`]'s mdast + hast visitors before JSX emission.
///
/// When `cache` is `Some(_)` and the same `(input, pipeline config)`
/// pair has been compiled before, the cached JSX is returned without
/// invoking the emitter. When `cache` is `None`, every call compiles
/// fresh.
///
/// # Cache keying (zfb#905, context segment zfb#939)
///
/// The key is `"{sha256(input)};{fingerprint}"` — extended to
/// `"{sha256(input)};{fingerprint};{context}"` when the pipeline
/// carries per-call state ([`Pipeline::cache_key_context`], today
/// exactly the resolve-links per-file `source_dir`) — where the
/// fingerprint component is:
///
/// - the fixed token `no-pipeline` when `pipeline` is `None` (the
///   no-pipeline emit path produces different JSX than any pipeline'd
///   path, so the two must never alias);
/// - [`Pipeline::config_fingerprint`] (64 hex chars — cannot collide
///   with the token above) when the pipeline was built from config and
///   not manually mutated.
///
/// Pipelines whose fingerprint is `None` — manually mutated via
/// [`Pipeline::add_mdast_visitor`] / [`Pipeline::add_hast_visitor`], or
/// carrying a filesystem-reading feature plugin (see
/// [`Pipeline::config_fingerprint`]) — **bypass the cache entirely**:
/// every call compiles fresh, exactly as if `cache` were `None`.
///
/// The `mdx://<collection>/<slug>#<hash>` specifier is re-derived from
/// THIS call's `file_path` even on a cache hit: two files with
/// byte-identical bodies share the (expensive) compiled JSX but each
/// receives a specifier matching its own path, so a hit can never leak
/// another file's collection/slug.
///
/// # Dependency-manifest validation (zfb#942)
///
/// A pipeline carrying a read-recorder ([`Pipeline::set_read_recorder`])
/// has its plugins' external reads drained into a
/// [`DependencyManifest`] stored with the entry. Every later lookup
/// re-probes each recorded dependency (full re-hash) before honouring
/// the hit; an edited, deleted, or newly-created-where-missing dep —
/// or any probe failure — recompiles and re-records. A manifest that
/// recorded a read *error* is never stored at all (it could never
/// validate). Pipelines without a recorder store an empty manifest —
/// validation is then a no-op and behaviour is unchanged.
///
/// Recorder-armed pipelines additionally key the source file's parent
/// directory (`;recorder_source_dir=…`, normalised like every other
/// path segment): plugins may resolve reads relative to the file being
/// compiled, so identical bodies in different directories must not
/// alias one entry — the first file's manifest would still validate
/// for the second file and hand it JSX built from the wrong reads.
/// Identical bodies in the same directory keep sharing one entry.
///
/// Context-armed pipelines ([`Pipeline::set_build_context_roots`],
/// zfb#944) go one step further and key the full normalised source PATH
/// (`;context_source_path=…`): the context-aware plugins observe the
/// source path itself, not just its directory — transclude seeds cycle
/// detection with it (an identical body that includes a sibling can
/// expand for one file and cycle-error for the other), and
/// linkValidation stamps it into stored diagnostic locations — so even
/// same-directory identical bodies must not share an entry there.
///
/// # Markdown-diagnostics replay (zfb#944)
///
/// A context-armed pipeline's feature plugins emit
/// `MarkdownDiagnostic`s (e.g. linkValidation broken-link findings)
/// through the per-compile context sink, buffered on the pipeline and
/// drained by call sites via `Pipeline::take_markdown_diagnostics`.
/// Exactly like the broken-link channel above, a miss stores the slice
/// this compile appended and a hit replays it, so cross-file findings
/// survive cache hits.
///
/// # Broken-link diagnostics replay (zfb#939)
///
/// A pipeline wired with `ResolveLinksPlugin` accumulates
/// [`BrokenLinkDiagnostic`]s as a compile side channel, drained by call
/// sites via [`Pipeline::take_broken_links`] AFTER each compile. The
/// cache preserves that contract across hits: a miss stores the
/// diagnostics the compile appended alongside the JSX; a hit replays
/// the stored vec back into the pipeline's plugin before returning, so
/// the caller's drain observes exactly what a fresh compile would have
/// produced.
///
/// The on-miss insert is best-effort: if two threads race on the same
/// key they may both compile (and the second insert simply overwrites
/// the first identical value). We don't hold the cache lock across
/// compilation to avoid serialising CPU work.
///
/// # Errors
/// Forwards [`PipelineError::Parse`] from the underlying emitter. A parse
/// failure is never cached — the next call will retry.
///
/// # Precondition
/// When a `pipeline` carries per-document state (accumulated diagnostics,
/// heading registries, …), the caller must reset it (`reset_per_entry()`
/// or equivalent) before each call — the cache key covers input + config
/// fingerprint + per-call context only, not visitor state. All
/// production call sites do this today.
pub fn compile_mdx_to_jsx_module_cached(
    input: &str,
    file_path: &Path,
    cache: Option<&MdxModuleCache>,
    pipeline: Option<&mut Pipeline>,
) -> Result<CompiledMdx, PipelineError> {
    compile_mdx_to_jsx_module_cached_with_deps(input, file_path, cache, pipeline).map(|(c, _)| c)
}

/// Like [`compile_mdx_to_jsx_module_cached`], but also returns the set of
/// **external file paths** the served compile recorded reads of — its
/// [`DependencyManifest::recorded_paths`] (every recorded path,
/// regardless of `Content` / `Missing` outcome).
///
/// This is the incremental-materialise signal the dev bundler's
/// `ShadowSession` content-file skip cache needs (zfb#1148). The bundler
/// stats each returned dep path at record time and re-stats it on every
/// later tick: a content `.mdx` may be skipped (its previous compile /
/// import reused without re-reading/re-compiling/re-writing) only while
/// the file's own `(mtime, size)` AND every recorded dep's on-disk state
/// are unchanged. A file with zero deps is the trivial all-deps-unchanged
/// case → still skippable. A file that transcludes / links another (a
/// recorded read) is skipped only while that dep is byte-stable, and
/// re-materialised the moment the dep's mtime/size changes (or a
/// previously-missing dep appears).
///
/// The paths are read from the **served** compile's manifest, NOT the
/// live recorder: on a cache hit the feature plugins never run, so the
/// recorder would look empty even for a file that genuinely has deps —
/// only the stored, validated manifest is authoritative. On a miss it is
/// the freshly-drained manifest, captured before it is moved into the
/// cache entry.
///
/// A pipeline without a read-recorder (no filesystem-dependent feature
/// enabled) always returns an empty `Vec` — its manifest is empty by
/// construction, which is correct: with no such feature wired, a file's
/// output is a pure function of its own bytes.
pub fn compile_mdx_to_jsx_module_cached_with_deps(
    input: &str,
    file_path: &Path,
    cache: Option<&MdxModuleCache>,
    mut pipeline: Option<&mut Pipeline>,
) -> Result<(CompiledMdx, Vec<PathBuf>), PipelineError> {
    let (collection, slug) = collection_and_slug(file_path);

    // Fingerprint component of the cache key. `None` here means "no key
    // needed" (no cache supplied — skip the hashing work entirely) or
    // "this pipeline cannot be keyed" → bypass the cache.
    let fingerprint: Option<String> = match (cache, pipeline.as_deref()) {
        (None, _) => None,
        (Some(_), None) => Some("no-pipeline".to_string()),
        (Some(_), Some(p)) => p.config_fingerprint(),
    };
    let cache_for_lookup = if fingerprint.is_some() { cache } else { None };

    // `sha256(input)` (full hex) + fingerprint (+ per-call context) is
    // the key. The full input digest, not the 8-char prefix, so distinct
    // sources that happen to share an 8-hex prefix on their *output*
    // don't clobber each other in the cache.
    let cache_key = match (cache_for_lookup, fingerprint.as_deref()) {
        (Some(_), Some(fp)) => {
            let mut h = Sha256::new();
            h.update(input.as_bytes());
            let input_hash = hex::encode(h.finalize());
            // Per-call context segment (zfb#939): per-FILE pipeline
            // state that shapes the output but is invisible to the
            // construction-time config fingerprint — today the
            // resolve-links `source_dir`. Absent context keeps the
            // pre-#939 two-part key shape byte-for-byte.
            let mut key = match pipeline.as_deref().and_then(Pipeline::cache_key_context) {
                Some(ctx) => format!("{input_hash};{fp};{ctx}"),
                None => format!("{input_hash};{fp}"),
            };
            // Recorder source-dir segment (zfb#942): a recorder-armed
            // pipeline may carry plugins that resolve reads RELATIVE
            // to the file being compiled (transclude's `./include.md`),
            // so identical bodies in different directories can read
            // different files and emit different JSX. Manifest
            // validation alone cannot catch that aliasing — the first
            // file's deps still validate when the second file looks
            // up — so the resolution basis (the source file's parent
            // dir) joins the key whenever a recorder is attached.
            // Identical bodies in the SAME directory keep sharing one
            // entry (their relative reads resolve identically); the
            // pre-#942 key shape is untouched for every
            // recorder-less pipeline.
            if pipeline
                .as_deref()
                .is_some_and(|p| p.read_recorder().is_some())
            {
                let source_dir = file_path
                    .parent()
                    .map(crate::path_norm::normalize_path_lexically)
                    .unwrap_or_default();
                key.push_str(";recorder_source_dir=");
                key.push_str(&source_dir);
            }
            // Context source-path segment (zfb#944): a context-armed
            // pipeline threads a per-file BuildContext whose plugins
            // observe the source PATH itself — transclude's cycle
            // detection and linkValidation's diagnostic locations are
            // path-dependent, so even identical bodies in the SAME
            // directory must not share an entry. See the key-shape docs
            // above.
            if pipeline
                .as_deref()
                .is_some_and(|p| p.build_context_roots().is_some())
            {
                key.push_str(";context_source_path=");
                key.push_str(&crate::path_norm::normalize_path_lexically(file_path));
            }
            Some(key)
        }
        _ => None,
    };

    if let (Some(c), Some(key)) = (cache_for_lookup, cache_key.as_ref()) {
        // Clone out of the lock before validating: dependency
        // validation does filesystem reads (one re-hash per recorded
        // dep) and must not serialise other cache users behind it.
        let hit = c.lock().get(key).cloned();
        if let Some(hit) = hit {
            // Dependency-manifest validation (zfb#942): a hit is
            // honoured only while every external file the original
            // compile read is byte-identical (and every file it found
            // missing is still missing). Edited/deleted/created deps —
            // and any probe failure — fall through to a fresh compile,
            // which re-records the manifest and overwrites this entry
            // under the same key. Per-entry rehash only; there is no
            // reverse dep→entries graph to update. The empty manifest
            // of a plain pipeline validates trivially.
            if hit.dependencies.still_valid() {
                // Diagnostics replay (zfb#939/#944) + side-channel replay
                // (#977): re-inject the stored broken-link + markdown
                // diagnostics and the cross-file link/heading channels so
                // the caller's post-compile drains (`take_broken_links()`
                // / `take_markdown_diagnostics()` /
                // `take_cross_file_link_candidates()` /
                // `take_file_headings()`) see them despite the plugins
                // never running on this hit.
                if let Some(p) = pipeline.as_deref_mut() {
                    p.replay_broken_links(hit.broken_links);
                    p.replay_markdown_diagnostics(hit.markdown_diagnostics);
                    p.replay_cross_file_link_candidates(hit.cross_file_links);
                    p.replay_file_headings(hit.file_headings);
                }
                // Incremental-materialise signal (zfb#1148): the served
                // entry's validated manifest is authoritative for the
                // recorded dep paths — the plugins did not run on this
                // hit, so the live recorder is empty. `still_valid()`
                // above already re-probed every dep, so these paths are
                // a sound dep set for the bundler's mtime/size re-check.
                let recorded_deps = hit.dependencies.recorded_paths();
                return Ok((
                    CompiledMdx {
                        jsx_source: hit.compiled.jsx_source,
                        content_hash: hit.compiled.content_hash.clone(),
                        // Path-derived, so NOT taken from the cached entry: an
                        // identical body first compiled at another path must
                        // not hand this file the other file's collection/slug.
                        specifier: format!(
                            "mdx://{collection}/{slug}#{}",
                            hit.compiled.content_hash
                        ),
                    },
                    recorded_deps,
                ));
            }
        }
    }

    // Snapshot the diagnostic counts BEFORE compiling: the buffers may
    // still hold earlier files' not-yet-drained diagnostics (the
    // snapshot walker never drains), and only the suffix THIS compile
    // appends belongs in the cached entry.
    let broken_links_before = pipeline.as_deref().map_or(0, Pipeline::broken_links_len);
    let markdown_diagnostics_before = pipeline
        .as_deref()
        .map_or(0, Pipeline::markdown_diagnostics_len);
    let cross_file_links_before = pipeline
        .as_deref()
        .map_or(0, Pipeline::cross_file_link_candidates_len);
    let file_headings_before = pipeline.as_deref().map_or(0, Pipeline::file_headings_len);

    // Scope read recording (zfb#942) to THIS compile: reads left by an
    // earlier compile on the same pipeline (e.g. one that aborted on a
    // parse error and was never drained) must not leak into this
    // entry's manifest. No-op without a recorder.
    if let Some(p) = pipeline.as_deref() {
        p.clear_recorded_reads();
    }

    let opts = MdxJsxOptions::default()
        .with_filename(file_path.display().to_string())
        // Per-file BuildContext basis (zfb#944): consumed only by
        // pipelines that armed context roots; inert otherwise.
        .with_source_path(file_path);
    let jsx_source = mdx_to_jsx_module_inner(input, opts, pipeline.as_deref_mut())?;
    let content_hash = hash_8(&jsx_source);
    let specifier = format!("mdx://{collection}/{slug}#{content_hash}");

    let compiled = CompiledMdx {
        jsx_source,
        content_hash,
        specifier,
    };

    // Drain the reads this compile's plugins reported into a
    // [`DependencyManifest`] (zfb#942). Hoisted OUT of the cache-insert
    // block (it was previously nested there) so the
    // incremental-materialise dep-path signal (zfb#1148) is computed on
    // every miss — including the no-cache path — and so the recorder is
    // always drained (leaving it for the next compile to find would leak
    // reads into another entry's manifest). Empty without a recorder.
    // The recorded paths are captured before the manifest is moved into
    // the entry.
    let dependencies = pipeline
        .as_deref()
        .map(|p| p.take_dependency_manifest())
        .unwrap_or_default();
    let recorded_deps = dependencies.recorded_paths();

    if let (Some(c), Some(key)) = (cache_for_lookup, cache_key) {
        let broken_links = pipeline
            .as_deref()
            .map(|p| p.broken_links_since(broken_links_before))
            .unwrap_or_default();
        let markdown_diagnostics = pipeline
            .as_deref()
            .map(|p| p.markdown_diagnostics_since(markdown_diagnostics_before))
            .unwrap_or_default();
        // Side-channel slicing (#977): same snapshot-before/slice-after
        // discipline as the diagnostics above — the buffers may still
        // hold earlier files' undrained entries (the snapshot walker
        // never drains), and only THIS compile's suffix belongs in the
        // entry; anything else would leak one file's candidates/headings
        // into another file's cache-hit replay.
        let cross_file_links = pipeline
            .as_deref()
            .map(|p| p.cross_file_link_candidates_since(cross_file_links_before))
            .unwrap_or_default();
        let file_headings = pipeline
            .as_deref()
            .map(|p| p.file_headings_since(file_headings_before))
            .unwrap_or_default();
        // A manifest carrying a read *error* is unstorable: error states
        // cannot be re-validated, so the entry could never be served —
        // skip the insert and let every later call recompile until the
        // read stops erroring.
        if dependencies.is_storable() {
            let mut guard = c.lock();
            // Clear-on-overflow: if inserting this entry would push the map
            // past the cap, evict everything first.  Simple and correct —
            // the next hit on any previously-cached key will re-compile once,
            // then re-populate.
            if guard.len() >= MDX_MODULE_CACHE_CAP {
                guard.clear();
            }
            guard.insert(
                key,
                CachedMdxModule {
                    compiled: compiled.clone(),
                    broken_links,
                    markdown_diagnostics,
                    cross_file_links,
                    file_headings,
                    dependencies,
                },
            );
        }
    }

    Ok((compiled, recorded_deps))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emit(src: &str) -> String {
        mdx_to_jsx_module(src, MdxJsxOptions::default()).expect("emit ok")
    }

    // #1392: is_module_level_esm used to match only `export const`,
    // `export default`, `export {`, and `import ` — a top-level JsxRaw node
    // carrying `export let`/`export var`/`export function`/`export async
    // function`/`export class`/`export * from` would NOT be hoisted to
    // column 0, so MDX/esbuild would reject the emitted module (or silently
    // treat the declaration as indented content). Pin every newly-matched
    // form, plus the pre-existing forms so this test doubles as a
    // regression guard for those too.
    #[test]
    fn is_module_level_esm_matches_every_export_and_import_form() {
        let matching = [
            r#"export const toc = [];"#,
            r#"export let counter = 0;"#,
            r#"export var legacy = 0;"#,
            r#"export function helper() {}"#,
            r#"export async function fetchIt() {}"#,
            r#"export class Thing {}"#,
            r#"export default function Page() {}"#,
            r#"export default class Page {}"#,
            r#"export default 42;"#,
            r#"export { a, b };"#,
            r#"export * from "./mod";"#,
            r#"export * as ns from "./mod";"#,
            r#"import foo from "./mod";"#,
        ];
        for s in matching {
            assert!(is_module_level_esm(s), "expected a match for: {s}");
        }
    }

    #[test]
    fn is_module_level_esm_rejects_non_module_level_text() {
        let non_matching = [
            "just a paragraph",
            "// export const fake = 1 (a comment mentioning export)",
            "exported",
            "importantThing",
            "const x = 1;",
        ];
        for s in non_matching {
            assert!(!is_module_level_esm(s), "expected NO match for: {s}");
        }
    }

    // #1404 review (doc-accuracy): `is_module_level_esm` INTENTIONALLY does
    // not classify generator exports or TS type-only exports as module-level
    // ESM — no shipped/planned hast plugin injects them (see the fn doc's
    // "INTENTIONALLY NOT covered" section). This pins that documented
    // boundary: if a future change starts matching one of these, it must
    // reconcile the fn doc (and decide whether hoisting is actually correct)
    // rather than let the doc silently drift back into over-claiming.
    #[test]
    fn is_module_level_esm_excludes_generator_and_ts_type_exports() {
        let excluded = [
            // Generator exports — no space after `function`, so the
            // `export function `/`export async function ` prefixes miss them.
            r#"export function* gen() {}"#,
            r#"export async function* gen() {}"#,
            // TS type-only exports — erased before runtime, never injected.
            r#"export type Foo = string;"#,
            r#"export interface Foo {}"#,
            r#"export enum Color { Red }"#,
        ];
        for s in excluded {
            assert!(
                !is_module_level_esm(s),
                "form is intentionally NOT classified as module-level ESM; if \
                 this changed, update the is_module_level_esm doc: {s}"
            );
        }
    }

    /// Default feature-aware production pipeline (the shape the bundler,
    /// snapshot walker, and dev loader build when `zfb.config.ts` carries
    /// no markdown options).
    fn full_config_pipeline(theme: Option<&str>) -> Pipeline {
        Pipeline::with_defaults_and_full_config(
            theme,
            ResolvedGfmConstructs::CONSERVATIVE,
            None,
            true,
            false,
            None,
        )
        .expect("no themes_dir — cannot fail")
    }

    // zfb#905: a config-built pipeline must actually USE the cache. The
    // sentinel poke makes a true hit observable — an accidental
    // recompile would return real JSX, not the sentinel.
    #[test]
    fn cache_hit_serves_cached_entry_for_equal_pipeline_config() {
        let cache = MdxModuleCache::new();
        let src = "# heading\n\n```rs\nfn x() {}\n```\n";
        let path = Path::new("/virtual/blog/post.mdx");

        let mut p1 = full_config_pipeline(None);
        let first = compile_mdx_to_jsx_module_cached(src, path, Some(&cache), Some(&mut p1))
            .expect("compile 1");
        assert_eq!(cache.len(), 1, "pipeline'd compile must populate the cache");

        for entry in cache.lock().values_mut() {
            entry.compiled.jsx_source = "__SENTINEL__".to_string();
        }

        let mut p2 = full_config_pipeline(None);
        let second = compile_mdx_to_jsx_module_cached(src, path, Some(&cache), Some(&mut p2))
            .expect("compile 2");
        assert_eq!(
            second.jsx_source, "__SENTINEL__",
            "same config + same input must be served from the cache"
        );
        assert_eq!(second.content_hash, first.content_hash);

        // A different config (highlight theme flipped) must MISS: fresh
        // compile, second entry, sentinel never surfaces.
        let mut p3 = full_config_pipeline(Some("InspiredGitHub"));
        let third = compile_mdx_to_jsx_module_cached(src, path, Some(&cache), Some(&mut p3))
            .expect("compile 3");
        assert_ne!(
            third.jsx_source, "__SENTINEL__",
            "a different pipeline config must not alias the cached entry"
        );
        assert_eq!(cache.len(), 2, "different config must add its own entry");
    }

    // zfb#939: a resolve-links pipeline must also actually USE the
    // cache (pre-#939 it invalidated the fingerprint and bypassed).
    // Same sentinel poke as above — a true hit returns the sentinel,
    // an accidental recompile would return real JSX.
    #[test]
    fn resolve_links_recompile_is_served_from_the_cache() {
        let cache = MdxModuleCache::new();
        let src = "[good](./other.mdx)\n";
        let path = Path::new("/content/docs/index.mdx");
        let mut map = HashMap::new();
        map.insert(
            std::path::PathBuf::from("/content/docs/other.mdx"),
            "/docs/other/".to_string(),
        );

        let mut p1 = full_config_pipeline(None);
        p1.add_resolve_links(map.clone());
        p1.set_resolve_links_source_dir(std::path::PathBuf::from("/content/docs"));
        let first = compile_mdx_to_jsx_module_cached(src, path, Some(&cache), Some(&mut p1))
            .expect("compile 1");
        assert!(first.jsx_source.contains("/docs/other/"));
        assert_eq!(
            cache.len(),
            1,
            "resolve-links pipeline must populate the cache"
        );

        for entry in cache.lock().values_mut() {
            entry.compiled.jsx_source = "__SENTINEL__".to_string();
        }

        let mut p2 = full_config_pipeline(None);
        p2.add_resolve_links(map);
        p2.set_resolve_links_source_dir(std::path::PathBuf::from("/content/docs"));
        let second = compile_mdx_to_jsx_module_cached(src, path, Some(&cache), Some(&mut p2))
            .expect("compile 2");
        assert_eq!(
            second.jsx_source, "__SENTINEL__",
            "unchanged file + same map + same source_dir must be a true cache hit"
        );
    }

    // ── zfb#942: read-recorder + dependency-manifest cache validation ──
    //
    // Synthetic stand-in for a filesystem-reading feature plugin (the
    // real ones — transclude, imageDimensions, linkValidation — start
    // recording in zfb#944): on every visit it reports the configured
    // reads through the recorder, exactly as a production plugin will.
    struct SyntheticReadsVisitor {
        recorder: std::sync::Arc<zfb_md_ast::ReadRecorder>,
        files: Vec<std::path::PathBuf>,
        explicit: Vec<(std::path::PathBuf, zfb_md_ast::ReadOutcome)>,
    }

    impl crate::pipeline::MdastVisitor for SyntheticReadsVisitor {
        fn visit(&mut self, _node: &mut MdastNode) {
            for f in &self.files {
                let _ = self.recorder.record_file(f);
            }
            for (p, o) in &self.explicit {
                self.recorder.record_outcome(p, o.clone());
            }
        }
    }

    /// Cacheable default pipeline + synthetic recording visitor. The
    /// visitor is pushed through the fingerprint-preserving test seam
    /// so every pipeline built here shares ONE fingerprint — the same
    /// situation zfb#944 creates for config-derived recording plugins.
    fn recording_pipeline(
        files: &[std::path::PathBuf],
        explicit: &[(std::path::PathBuf, zfb_md_ast::ReadOutcome)],
    ) -> (Pipeline, std::sync::Arc<zfb_md_ast::ReadRecorder>) {
        let recorder = std::sync::Arc::new(zfb_md_ast::ReadRecorder::new());
        let mut p = full_config_pipeline(None);
        p.push_mdast_visitor_preserving_fingerprint_for_tests(Box::new(SyntheticReadsVisitor {
            recorder: std::sync::Arc::clone(&recorder),
            files: files.to_vec(),
            explicit: explicit.to_vec(),
        }));
        p.set_read_recorder(std::sync::Arc::clone(&recorder));
        (p, recorder)
    }

    fn poke_sentinel(cache: &MdxModuleCache) {
        for entry in cache.lock().values_mut() {
            entry.compiled.jsx_source = "__SENTINEL__".to_string();
        }
    }

    // zfb#942: an unchanged recorded dep keeps the entry a true hit; an
    // edited dep is a miss; the recompile re-records the manifest so
    // the edited state caches again.
    #[test]
    fn dep_edit_invalidates_then_recompile_recaches() {
        let cache = MdxModuleCache::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let dep = dir.path().join("include.md");
        std::fs::write(&dep, "v1").expect("write dep");
        let src = "# page\n";
        let path = Path::new("/content/docs/page.mdx");
        let deps = vec![dep.clone()];

        let (mut p1, _) = recording_pipeline(&deps, &[]);
        compile_mdx_to_jsx_module_cached(src, path, Some(&cache), Some(&mut p1))
            .expect("compile 1");
        assert_eq!(cache.len(), 1, "recording pipeline must populate the cache");

        poke_sentinel(&cache);
        let (mut p2, _) = recording_pipeline(&deps, &[]);
        let second = compile_mdx_to_jsx_module_cached(src, path, Some(&cache), Some(&mut p2))
            .expect("compile 2");
        assert_eq!(
            second.jsx_source, "__SENTINEL__",
            "unchanged dep must validate and serve the cached entry"
        );

        std::fs::write(&dep, "v2").expect("edit dep");
        let (mut p3, _) = recording_pipeline(&deps, &[]);
        let third = compile_mdx_to_jsx_module_cached(src, path, Some(&cache), Some(&mut p3))
            .expect("compile 3");
        assert_ne!(
            third.jsx_source, "__SENTINEL__",
            "edited dep must be a cache miss (recompile)"
        );
        assert_eq!(
            cache.len(),
            1,
            "recompile overwrites the stale entry in place"
        );

        // The recompile re-recorded the manifest against v2: with the
        // dep untouched since, the entry must hit again.
        poke_sentinel(&cache);
        let (mut p4, _) = recording_pipeline(&deps, &[]);
        let fourth = compile_mdx_to_jsx_module_cached(src, path, Some(&cache), Some(&mut p4))
            .expect("compile 4");
        assert_eq!(
            fourth.jsx_source, "__SENTINEL__",
            "re-recorded manifest must validate against the edited dep"
        );
    }

    // zfb#942: a read that found the file MISSING is recorded too — the
    // entry stays valid while the file stays absent and invalidates the
    // moment the file is created.
    #[test]
    fn missing_dep_created_later_invalidates_entry() {
        let cache = MdxModuleCache::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let dep = dir.path().join("not-yet.md");
        let src = "missing include\n";
        let path = Path::new("/content/docs/page.mdx");
        let deps = vec![dep.clone()];

        let (mut p1, _) = recording_pipeline(&deps, &[]);
        compile_mdx_to_jsx_module_cached(src, path, Some(&cache), Some(&mut p1))
            .expect("compile 1");
        assert_eq!(cache.len(), 1, "a Missing outcome is storable");

        poke_sentinel(&cache);
        let (mut p2, _) = recording_pipeline(&deps, &[]);
        let second = compile_mdx_to_jsx_module_cached(src, path, Some(&cache), Some(&mut p2))
            .expect("compile 2");
        assert_eq!(
            second.jsx_source, "__SENTINEL__",
            "still-missing dep must keep the entry a hit"
        );

        std::fs::write(&dep, "created now").expect("create dep");
        let (mut p3, _) = recording_pipeline(&deps, &[]);
        let third = compile_mdx_to_jsx_module_cached(src, path, Some(&cache), Some(&mut p3))
            .expect("compile 3");
        assert_ne!(
            third.jsx_source, "__SENTINEL__",
            "creating a previously-missing dep must invalidate the entry"
        );
    }

    // zfb#942: deleting a recorded dep is a miss; the recompile records
    // the now-missing state, which then validates while it stays gone.
    #[test]
    fn deleted_dep_invalidates_entry() {
        let cache = MdxModuleCache::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let dep = dir.path().join("include.md");
        std::fs::write(&dep, "v1").expect("write dep");
        let src = "# page\n";
        let path = Path::new("/content/docs/page.mdx");
        let deps = vec![dep.clone()];

        let (mut p1, _) = recording_pipeline(&deps, &[]);
        compile_mdx_to_jsx_module_cached(src, path, Some(&cache), Some(&mut p1))
            .expect("compile 1");
        poke_sentinel(&cache);

        std::fs::remove_file(&dep).expect("delete dep");
        let (mut p2, _) = recording_pipeline(&deps, &[]);
        let second = compile_mdx_to_jsx_module_cached(src, path, Some(&cache), Some(&mut p2))
            .expect("compile 2");
        assert_ne!(
            second.jsx_source, "__SENTINEL__",
            "deleted dep must be a cache miss"
        );

        // The recompile recorded Missing — valid while the file stays gone.
        poke_sentinel(&cache);
        let (mut p3, _) = recording_pipeline(&deps, &[]);
        let third = compile_mdx_to_jsx_module_cached(src, path, Some(&cache), Some(&mut p3))
            .expect("compile 3");
        assert_eq!(
            third.jsx_source, "__SENTINEL__",
            "re-recorded Missing must validate while the dep stays deleted"
        );
    }

    // zfb#942: a read that errored (non-NotFound — permissions, I/O, …)
    // can never be re-validated, so the entry is not stored at all and
    // every call recompiles.
    #[test]
    fn read_error_prevents_caching() {
        let cache = MdxModuleCache::new();
        let src = "# page\n";
        let path = Path::new("/content/docs/page.mdx");
        let explicit = vec![(
            std::path::PathBuf::from("/somewhere/locked.md"),
            zfb_md_ast::ReadOutcome::Error,
        )];

        let (mut p1, _) = recording_pipeline(&[], &explicit);
        let first = compile_mdx_to_jsx_module_cached(src, path, Some(&cache), Some(&mut p1))
            .expect("compile 1");
        assert_eq!(
            cache.len(),
            0,
            "an Error outcome must make the entry unstorable"
        );

        let (mut p2, _) = recording_pipeline(&[], &explicit);
        let second = compile_mdx_to_jsx_module_cached(src, path, Some(&cache), Some(&mut p2))
            .expect("compile 2");
        assert_eq!(cache.len(), 0, "still nothing cached on the second call");
        assert_eq!(
            first.jsx_source, second.jsx_source,
            "sanity: both fresh compiles emit identical JSX"
        );
    }

    // zfb#942 (review fix): recorder-armed pipelines must NOT share an
    // entry across source directories — a path-relative reader
    // (transclude's `./include.md`) reads different files for identical
    // bodies in different dirs, and manifest validation alone cannot
    // catch that aliasing. Same-directory bodies keep deduping.
    #[test]
    fn recorder_pipelines_key_entries_per_source_dir() {
        let cache = MdxModuleCache::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let dep = dir.path().join("include.md");
        std::fs::write(&dep, "v1").expect("write dep");
        let src = "shared body\n";
        let deps = vec![dep.clone()];

        let (mut p1, _) = recording_pipeline(&deps, &[]);
        compile_mdx_to_jsx_module_cached(
            src,
            Path::new("/content/docs/a.mdx"),
            Some(&cache),
            Some(&mut p1),
        )
        .expect("compile a");
        let (mut p2, _) = recording_pipeline(&deps, &[]);
        compile_mdx_to_jsx_module_cached(
            src,
            Path::new("/content/blog/b.mdx"),
            Some(&cache),
            Some(&mut p2),
        )
        .expect("compile b");
        assert_eq!(
            cache.len(),
            2,
            "identical bodies in DIFFERENT dirs must occupy distinct entries"
        );

        // Same dir as a.mdx → same resolution basis → true hit, no new entry.
        poke_sentinel(&cache);
        let (mut p3, _) = recording_pipeline(&deps, &[]);
        let third = compile_mdx_to_jsx_module_cached(
            src,
            Path::new("/content/docs/c.mdx"),
            Some(&cache),
            Some(&mut p3),
        )
        .expect("compile c");
        assert_eq!(
            cache.len(),
            2,
            "same-dir identical body must not add an entry"
        );
        assert_eq!(
            third.jsx_source, "__SENTINEL__",
            "same-dir identical body must be served from the shared entry"
        );
    }

    // zfb#942: reads sitting in the recorder from BEFORE the compile
    // (e.g. left by an aborted earlier compile on the same pipeline)
    // are cleared at compile start — they must not poison this entry's
    // manifest.
    #[test]
    fn stale_recorder_reads_do_not_leak_into_manifest() {
        let cache = MdxModuleCache::new();
        let src = "# page\n";
        let path = Path::new("/content/docs/page.mdx");

        let (mut p, recorder) = recording_pipeline(&[], &[]);
        recorder.record_outcome(
            Path::new("/stale/leftover.md"),
            zfb_md_ast::ReadOutcome::Error,
        );
        compile_mdx_to_jsx_module_cached(src, path, Some(&cache), Some(&mut p)).expect("compile");
        assert_eq!(
            cache.len(),
            1,
            "the pre-compile Error read must be discarded, leaving a storable entry"
        );
    }

    // ── zfb#944: REAL filesystem-reading feature plugins through the cache ──
    //
    // These drive the actual transclude / imageDimensions / linkValidation
    // plugins (wired by `with_defaults_and_full_config`, recorder attached
    // by the constructor, context armed via `set_build_context_roots`)
    // through `compile_mdx_to_jsx_module_cached` — the production choke
    // point — and pin the issue's acceptance scenarios end-to-end.

    /// Feature-config pipeline with context roots armed at `project_root`
    /// (public dir at `<root>/public`). Fresh per compile, mirroring the
    /// dev-tick shape (same config ⇒ same fingerprint).
    fn fs_features_pipeline(features_json: serde_json::Value, project_root: &Path) -> Pipeline {
        let feats: zfb_md_extras::MarkdownFeaturesConfig =
            serde_json::from_value(features_json).expect("valid features config");
        let mut p = Pipeline::with_defaults_and_full_config(
            None,
            ResolvedGfmConstructs::CONSERVATIVE,
            None,
            true,
            false,
            Some(&feats),
        )
        .expect("no themes_dir — cannot fail");
        p.set_build_context_roots(project_root.to_path_buf(), project_root.join("public"));
        p
    }

    /// Minimal PNG: signature + IHDR carrying `width`×`height`. Header
    /// sniffers (imagesize) read the dimensions straight from the IHDR
    /// without validating the CRC or needing pixel data.
    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        b.extend_from_slice(&13u32.to_be_bytes());
        b.extend_from_slice(b"IHDR");
        b.extend_from_slice(&width.to_be_bytes());
        b.extend_from_slice(&height.to_be_bytes());
        // bit depth 8, color type 6 (RGBA), compression/filter/interlace 0
        b.extend_from_slice(&[8, 6, 0, 0, 0]);
        b.extend_from_slice(&[0, 0, 0, 0]); // CRC (not validated by sniffers)
        b
    }

    // Acceptance: two files where one transcludes a shared snippet —
    // editing the snippet recompiles the transcluder and NOT the
    // unrelated file.
    #[test]
    fn transclude_snippet_edit_recompiles_dependent_not_unrelated_file() {
        let cache = MdxModuleCache::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("snippet.md"), "Shared snippet v1.\n").expect("write snippet");
        let body_a = ":::include{file=\"./snippet.md\"}\n";
        let body_b = "# unrelated\n\nplain body\n";
        let path_a = root.join("page-a.mdx");
        let path_b = root.join("page-b.mdx");
        std::fs::write(&path_a, body_a).expect("write a");
        std::fs::write(&path_b, body_b).expect("write b");
        let feats = serde_json::json!({ "transclude": {} });

        let mut p = fs_features_pipeline(feats.clone(), root);
        let first_a = compile_mdx_to_jsx_module_cached(body_a, &path_a, Some(&cache), Some(&mut p))
            .expect("compile a");
        assert!(
            first_a.jsx_source.contains("Shared snippet v1."),
            "the transclude plugin must fire on the cached compile path; got: {}",
            first_a.jsx_source
        );
        let mut p = fs_features_pipeline(feats.clone(), root);
        compile_mdx_to_jsx_module_cached(body_b, &path_b, Some(&cache), Some(&mut p))
            .expect("compile b");
        assert_eq!(cache.len(), 2);

        // Unchanged snippet → both entries are true hits.
        poke_sentinel(&cache);
        let mut p = fs_features_pipeline(feats.clone(), root);
        let hit_a = compile_mdx_to_jsx_module_cached(body_a, &path_a, Some(&cache), Some(&mut p))
            .expect("compile a again");
        assert_eq!(
            hit_a.jsx_source, "__SENTINEL__",
            "unchanged snippet must keep the transcluder a cache hit"
        );

        // Edit the snippet: the transcluder recompiles (fresh JSX with the
        // new content); the unrelated file stays a hit (sentinel).
        std::fs::write(root.join("snippet.md"), "Shared snippet v2.\n").expect("edit snippet");
        let mut p = fs_features_pipeline(feats.clone(), root);
        let fresh_a = compile_mdx_to_jsx_module_cached(body_a, &path_a, Some(&cache), Some(&mut p))
            .expect("recompile a");
        assert!(
            fresh_a.jsx_source.contains("Shared snippet v2."),
            "editing the snippet must recompile its dependent; got: {}",
            fresh_a.jsx_source
        );
        let mut p = fs_features_pipeline(feats, root);
        let hit_b = compile_mdx_to_jsx_module_cached(body_b, &path_b, Some(&cache), Some(&mut p))
            .expect("compile b again");
        assert_eq!(
            hit_b.jsx_source, "__SENTINEL__",
            "the unrelated file must NOT be recompiled by the snippet edit"
        );
    }

    // Acceptance: a missing include that is later created invalidates the
    // cached entry (and its cannot-read diagnostic replays on hits while
    // the include stays absent).
    #[test]
    fn missing_include_created_later_invalidates_real_transclude_entry() {
        let cache = MdxModuleCache::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let body = ":::include{file=\"./not-yet.md\"}\n";
        let path = root.join("page.mdx");
        std::fs::write(&path, body).expect("write page");
        let feats = serde_json::json!({ "transclude": {} });

        let mut p1 = fs_features_pipeline(feats.clone(), root);
        compile_mdx_to_jsx_module_cached(body, &path, Some(&cache), Some(&mut p1))
            .expect("compile 1");
        assert_eq!(cache.len(), 1, "a Missing include is storable");
        let fresh_diags = p1.take_markdown_diagnostics();
        assert!(
            fresh_diags
                .iter()
                .any(|d| matches!(d, MarkdownDiagnostic::Generic { message, .. } if message.contains("cannot read"))),
            "fresh compile must report the unreadable include: {fresh_diags:?}"
        );

        poke_sentinel(&cache);
        let mut p2 = fs_features_pipeline(feats.clone(), root);
        let second = compile_mdx_to_jsx_module_cached(body, &path, Some(&cache), Some(&mut p2))
            .expect("compile 2");
        assert_eq!(
            second.jsx_source, "__SENTINEL__",
            "still-missing include must keep the entry a hit"
        );
        assert_eq!(
            p2.take_markdown_diagnostics(),
            fresh_diags,
            "the cannot-read diagnostic must replay on the hit"
        );

        std::fs::write(root.join("not-yet.md"), "Now it exists.\n").expect("create include");
        let mut p3 = fs_features_pipeline(feats, root);
        let third = compile_mdx_to_jsx_module_cached(body, &path, Some(&cache), Some(&mut p3))
            .expect("compile 3");
        assert_ne!(
            third.jsx_source, "__SENTINEL__",
            "creating the include must invalidate the entry"
        );
        assert!(
            third.jsx_source.contains("Now it exists."),
            "the recompile must splice the new include; got: {}",
            third.jsx_source
        );
        assert!(
            p3.take_markdown_diagnostics().is_empty(),
            "the resolved include must not report diagnostics any more"
        );
    }

    // Acceptance: an image dimension change invalidates its dependents.
    #[test]
    fn image_dimension_change_invalidates_real_image_dimensions_entry() {
        let cache = MdxModuleCache::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("img.png"), png_bytes(100, 50)).expect("write png");
        let body = "![alt](./img.png)\n";
        let path = root.join("page.mdx");
        std::fs::write(&path, body).expect("write page");
        let feats = serde_json::json!({ "imageDimensions": {} });

        let mut p = fs_features_pipeline(feats.clone(), root);
        let first = compile_mdx_to_jsx_module_cached(body, &path, Some(&cache), Some(&mut p))
            .expect("compile 1");
        assert!(
            first.jsx_source.contains("width=\"100\"")
                && first.jsx_source.contains("height=\"50\""),
            "imageDimensions must fire on the cached compile path; got: {}",
            first.jsx_source
        );

        poke_sentinel(&cache);
        let mut p = fs_features_pipeline(feats.clone(), root);
        let second = compile_mdx_to_jsx_module_cached(body, &path, Some(&cache), Some(&mut p))
            .expect("compile 2");
        assert_eq!(
            second.jsx_source, "__SENTINEL__",
            "unchanged image must keep the entry a hit"
        );

        std::fs::write(root.join("img.png"), png_bytes(200, 150)).expect("replace png");
        let mut p = fs_features_pipeline(feats, root);
        let third = compile_mdx_to_jsx_module_cached(body, &path, Some(&cache), Some(&mut p))
            .expect("compile 3");
        assert_ne!(
            third.jsx_source, "__SENTINEL__",
            "a changed image must invalidate its dependent"
        );
        assert!(
            third.jsx_source.contains("width=\"200\"")
                && third.jsx_source.contains("height=\"150\""),
            "the recompile must inject the NEW dimensions; got: {}",
            third.jsx_source
        );
    }

    // Acceptance: a broken-link finding replays on a cache hit.
    #[test]
    fn broken_link_finding_replays_on_real_link_validation_hit() {
        let cache = MdxModuleCache::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let body = "[missing](./missing.md)\n";
        let path = root.join("page.mdx");
        std::fs::write(&path, body).expect("write page");
        let feats = serde_json::json!({ "linkValidation": {} });

        let mut p1 = fs_features_pipeline(feats.clone(), root);
        compile_mdx_to_jsx_module_cached(body, &path, Some(&cache), Some(&mut p1))
            .expect("compile 1");
        let fresh = p1.take_markdown_diagnostics();
        assert!(
            fresh.iter().any(
                |d| matches!(d, MarkdownDiagnostic::BrokenLink { url, .. } if url == "./missing.md")
            ),
            "fresh compile must report the broken link: {fresh:?}"
        );
        assert_eq!(cache.len(), 1, "a Missing link target is storable");

        poke_sentinel(&cache);
        let mut p2 = fs_features_pipeline(feats, root);
        let second = compile_mdx_to_jsx_module_cached(body, &path, Some(&cache), Some(&mut p2))
            .expect("compile 2");
        assert_eq!(
            second.jsx_source, "__SENTINEL__",
            "the still-broken link must be a true cache hit"
        );
        assert_eq!(
            p2.take_markdown_diagnostics(),
            fresh,
            "the broken-link finding must replay on the hit"
        );
        assert!(
            p2.take_markdown_diagnostics().is_empty(),
            "drain semantics: a second drain after the hit is empty"
        );
    }

    // zfb#944: a valid link target going away must invalidate — the
    // existence probe is recorded as a full-content dependency.
    #[test]
    fn deleted_link_target_invalidates_real_link_validation_entry() {
        let cache = MdxModuleCache::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("other.md"), "# Other\n").expect("write target");
        let body = "[ok](./other.md)\n";
        let path = root.join("page.mdx");
        std::fs::write(&path, body).expect("write page");
        let feats = serde_json::json!({ "linkValidation": {} });

        let mut p1 = fs_features_pipeline(feats.clone(), root);
        compile_mdx_to_jsx_module_cached(body, &path, Some(&cache), Some(&mut p1))
            .expect("compile 1");
        assert!(
            p1.take_markdown_diagnostics().is_empty(),
            "an existing target must not report a broken link"
        );

        poke_sentinel(&cache);
        std::fs::remove_file(root.join("other.md")).expect("delete target");
        let mut p2 = fs_features_pipeline(feats, root);
        let second = compile_mdx_to_jsx_module_cached(body, &path, Some(&cache), Some(&mut p2))
            .expect("compile 2");
        assert_ne!(
            second.jsx_source, "__SENTINEL__",
            "deleting the link target must invalidate the entry"
        );
        let diags = p2.take_markdown_diagnostics();
        assert!(
            diags.iter().any(
                |d| matches!(d, MarkdownDiagnostic::BrokenLink { url, .. } if url == "./other.md")
            ),
            "the recompile must report the now-broken link: {diags:?}"
        );
    }

    // zfb#944: context-armed pipelines key per source PATH — identical
    // bodies in ONE directory must not share an entry (transclude's
    // cycle detection and linkValidation's diagnostic locations observe
    // the source path itself, so per-file output/diagnostics can differ).
    #[test]
    fn context_armed_pipelines_key_entries_per_source_path() {
        let cache = MdxModuleCache::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("snippet.md"), "shared\n").expect("write snippet");
        let body = ":::include{file=\"./snippet.md\"}\n";
        let path_a = root.join("a.mdx");
        let path_b = root.join("b.mdx");
        std::fs::write(&path_a, body).expect("write a");
        std::fs::write(&path_b, body).expect("write b");
        let feats = serde_json::json!({ "transclude": {} });

        let mut p = fs_features_pipeline(feats.clone(), root);
        compile_mdx_to_jsx_module_cached(body, &path_a, Some(&cache), Some(&mut p))
            .expect("compile a");
        let mut p = fs_features_pipeline(feats, root);
        compile_mdx_to_jsx_module_cached(body, &path_b, Some(&cache), Some(&mut p))
            .expect("compile b");
        assert_eq!(
            cache.len(),
            2,
            "identical same-dir bodies must occupy distinct entries when \
             context threading is armed"
        );
    }

    // zfb#905: identical bodies at different paths share the compiled
    // JSX but each call gets a specifier derived from ITS OWN path — a
    // hit must never leak another file's collection/slug.
    #[test]
    fn cache_hit_rederives_specifier_from_current_path() {
        let cache = MdxModuleCache::new();
        let src = "shared body\n";

        let mut p1 = full_config_pipeline(None);
        let a = compile_mdx_to_jsx_module_cached(
            src,
            Path::new("/virtual/blog/a.mdx"),
            Some(&cache),
            Some(&mut p1),
        )
        .expect("compile a");
        let mut p2 = full_config_pipeline(None);
        let b = compile_mdx_to_jsx_module_cached(
            src,
            Path::new("/virtual/docs/b.mdx"),
            Some(&cache),
            Some(&mut p2),
        )
        .expect("compile b");

        assert_eq!(
            cache.len(),
            1,
            "identical bodies under one config share one cache entry"
        );
        assert_eq!(a.content_hash, b.content_hash);
        assert_eq!(a.jsx_source, b.jsx_source);
        assert_eq!(a.specifier, format!("mdx://blog/a#{}", a.content_hash));
        assert_eq!(b.specifier, format!("mdx://docs/b#{}", b.content_hash));
    }

    // zfb#905: the no-pipeline path and a pipeline'd path emit different
    // JSX for the same input (heading-links anchors etc.), so the two
    // must occupy distinct cache entries.
    #[test]
    fn no_pipeline_and_pipeline_entries_do_not_alias() {
        let cache = MdxModuleCache::new();
        let src = "## Intro\n\nhi\n";
        let path = Path::new("/virtual/blog/intro.mdx");

        let plain =
            compile_mdx_to_jsx_module_cached(src, path, Some(&cache), None).expect("compile plain");
        let mut p = full_config_pipeline(None);
        let piped = compile_mdx_to_jsx_module_cached(src, path, Some(&cache), Some(&mut p))
            .expect("compile piped");

        assert_eq!(
            cache.len(),
            2,
            "no-pipeline and pipeline'd keys must differ"
        );
        assert_ne!(
            plain.jsx_source, piped.jsx_source,
            "sanity: the two paths emit different JSX for a heading"
        );
    }

    // zfb#905: GFM construct flags are part of the fingerprint — the
    // canonical stale-JSX hazard is `~~strike~~` compiled under ALL_ON
    // being served to an ALL_OFF pipeline (or vice versa).
    #[test]
    fn gfm_config_split_compiles_distinct_entries() {
        let cache = MdxModuleCache::new();
        let src = "plain ~~gone~~ here\n";
        let path = Path::new("/virtual/blog/strike.mdx");

        let mut all_on = Pipeline::with_defaults_and_gfm(ResolvedGfmConstructs::ALL_ON);
        let on = compile_mdx_to_jsx_module_cached(src, path, Some(&cache), Some(&mut all_on))
            .expect("compile gfm on");
        let mut all_off = Pipeline::with_defaults_and_gfm(ResolvedGfmConstructs::ALL_OFF);
        let off = compile_mdx_to_jsx_module_cached(src, path, Some(&cache), Some(&mut all_off))
            .expect("compile gfm off");

        assert_eq!(cache.len(), 2, "ALL_ON and ALL_OFF must not share an entry");
        assert_ne!(on.jsx_source, off.jsx_source);
        assert!(
            !on.jsx_source.contains("~~gone~~"),
            "ALL_ON must consume the strikethrough tildes"
        );
        assert!(
            off.jsx_source.contains("~~gone~~"),
            "ALL_OFF must keep the literal tildes"
        );
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

    /// GFM pipe-table renders as `<table><thead>…</thead><tbody>…</tbody></table>`,
    /// matching the canonical shape from zfb#136 / issue #193.
    ///
    /// Input:
    /// ```text
    /// | Key | URL |
    /// | --- | --- |
    /// | docs | https://example.com/some/path.html |
    /// | api  | https://api.example.com/v1/endpoint.json |
    /// ```
    ///
    /// Expected structure mirrors what Docusaurus/Astro MDX would emit.
    #[test]
    fn pipe_table_emits_thead_tbody_structure() {
        let src = "| Key | URL |\n| --- | --- |\n| docs | https://example.com/some/path.html |\n| api  | https://api.example.com/v1/endpoint.json |\n";
        let out = emit(src);

        // All table-related tags registered in _components map.
        assert!(
            out.contains("table: \"table\","),
            "table tag missing: {out}"
        );
        assert!(
            out.contains("thead: \"thead\","),
            "thead tag missing: {out}"
        );
        assert!(
            out.contains("tbody: \"tbody\","),
            "tbody tag missing: {out}"
        );
        assert!(out.contains("tr: \"tr\","), "tr tag missing: {out}");
        assert!(out.contains("th: \"th\","), "th tag missing: {out}");
        assert!(out.contains("td: \"td\","), "td tag missing: {out}");

        // Outer table element.
        assert!(
            out.contains("<_components.table>"),
            "missing <table>: {out}"
        );

        // Header row with <th> cells.
        assert!(
            out.contains("<_components.thead>"),
            "missing <thead>: {out}"
        );
        assert!(out.contains("<_components.th>"), "missing <th>: {out}");
        assert!(
            out.contains("{\"Key\"}"),
            "header cell 'Key' missing: {out}"
        );
        assert!(
            out.contains("{\"URL\"}"),
            "header cell 'URL' missing: {out}"
        );

        // Body rows with <td> cells.
        assert!(
            out.contains("<_components.tbody>"),
            "missing <tbody>: {out}"
        );
        assert!(out.contains("<_components.td>"), "missing <td>: {out}");
        assert!(
            out.contains("{\"docs\"}"),
            "body cell 'docs' missing: {out}"
        );
        assert!(
            out.contains("example.com/some/path.html"),
            "body cell URL missing: {out}"
        );
    }

    /// Per-column alignment is emitted as a `style` OBJECT prop
    /// (`style={{"textAlign": "left"}}`) on `<th>` and `<td>` elements.
    /// `:---` = left, `---:` = right, `:---:` = center.
    ///
    /// The spelling changed from the string form `style="text-align: left"`
    /// during epic #2021's review pass: React throws on a string-valued
    /// `style` prop, so every JSX emit site now goes through
    /// `jsx_style_attr`. The alignment semantics asserted here are
    /// unchanged — only how the declaration is spelled in JSX.
    #[test]
    fn pipe_table_alignment_emits_style_attr() {
        // Columns: left | right | center | none
        let src = "| A | B | C | D |\n| :--- | ---: | :---: | --- |\n| a | b | c | d |\n";
        let out = emit(src);

        // Header cells carry the alignment.
        assert!(
            out.contains("style={{\"textAlign\": \"left\"}}"),
            "left alignment missing: {out}"
        );
        assert!(
            out.contains("style={{\"textAlign\": \"right\"}}"),
            "right alignment missing: {out}"
        );
        assert!(
            out.contains("style={{\"textAlign\": \"center\"}}"),
            "center alignment missing: {out}"
        );
        // The fourth column has no alignment → no style attr on that cell.
        // A loose check: the output must not have a 4th `style=` that
        // would indicate the None column sprouted one.
        let style_count = out.matches("style={{\"textAlign\":").count();
        // 4 columns × 2 rows (head + body) = 8 cells, but only 3 columns
        // have alignment → 3 × 2 = 6 `style=` occurrences.
        assert_eq!(
            style_count, 6,
            "expected 6 style attrs (3 cols × 2 rows): {out}"
        );
        assert!(
            !out.contains("style=\""),
            "no JSX emit site may produce a STRING style prop — React \
             throws on one: {out}"
        );
    }

    // ─── HastNode::Raw block-aware wrapper tests (#1490) ────────────────────

    /// syntect's output starts with `<pre` — the bridge must wrap it in
    /// `<div dangerouslySetInnerHTML …>`, not `<span>`, so that
    /// preact-render-to-string does not emit the invalid `<span><pre>` nesting.
    #[test]
    fn raw_pre_emits_div_wrapper() {
        let mut bridge = HastJsxBridge::new();
        let node = HastNode::Raw(r#"<pre class="syntect-x"><code>1</code></pre>"#.to_string());
        let out = bridge.emit_node(&node);
        assert!(
            out.starts_with("<div dangerouslySetInnerHTML"),
            "syntect <pre> raw node must use <div> wrapper, got: {out}"
        );
        assert!(
            !out.starts_with("<span"),
            "<span> must not be used for block-level raw HTML, got: {out}"
        );
    }

    /// Inline raw HTML (e.g. `<code>`, `<em>`) must continue to use the
    /// `<span>` wrapper — same shape as before this fix.
    #[test]
    fn raw_inline_emits_span_wrapper() {
        let mut bridge = HastJsxBridge::new();
        let node = HastNode::Raw("<code>x</code>".to_string());
        let out = bridge.emit_node(&node);
        assert!(
            out.starts_with("<span dangerouslySetInnerHTML"),
            "inline raw node must still use <span> wrapper, got: {out}"
        );
    }

    /// `<table>` is a block-level element — its raw HTML must be wrapped
    /// in `<div>`, not `<span>`.
    #[test]
    fn raw_table_emits_div_wrapper() {
        let mut bridge = HastJsxBridge::new();
        let node = HastNode::Raw("<table><tr><td>x</td></tr></table>".to_string());
        let out = bridge.emit_node(&node);
        assert!(
            out.starts_with("<div dangerouslySetInnerHTML"),
            "<table> raw node must use <div> wrapper, got: {out}"
        );
    }

    /// Leading whitespace before `<pre>` must not fool the dispatcher —
    /// `trim_start` must fire before the tag check.
    #[test]
    fn raw_leading_whitespace_emits_div_wrapper() {
        let mut bridge = HastJsxBridge::new();
        let node = HastNode::Raw("  \n<pre>hello</pre>".to_string());
        let out = bridge.emit_node(&node);
        assert!(
            out.starts_with("<div dangerouslySetInnerHTML"),
            "leading whitespace before <pre> must still produce <div> wrapper, got: {out}"
        );
    }

    /// HTML comments do not start with a block-level tag — they must
    /// fall through to the `<span>` path (no regression for the inline path).
    #[test]
    fn raw_html_comment_emits_span_wrapper() {
        let mut bridge = HastJsxBridge::new();
        let node = HastNode::Raw("<!-- comment -->".to_string());
        let out = bridge.emit_node(&node);
        assert!(
            out.starts_with("<span dangerouslySetInnerHTML"),
            "HTML comment raw node must use <span> wrapper, got: {out}"
        );
    }

    // zfb#911: inserting more than MDX_MODULE_CACHE_CAP entries must
    // trigger the clear-on-overflow policy so the map stays bounded.
    //
    // Uses a fresh `MdxModuleCache::new()` (not the process-global) so
    // this test is hermetic and not affected by other tests' entries.
    // Keys are distinct bodies — one per iteration — so no hits occur
    // and each `compile_mdx_to_jsx_module_cached` call adds one entry.
    #[test]
    fn cache_clears_on_overflow_past_cap() {
        let cache = MdxModuleCache::new();
        let path = Path::new("/virtual/blog/overflow.mdx");

        // Fill exactly up to the cap with distinct single-word bodies.
        // We rely only on the returned len(), not on map iteration order.
        for i in 0..MDX_MODULE_CACHE_CAP {
            let src = format!("word{i}\n");
            compile_mdx_to_jsx_module_cached(&src, path, Some(&cache), None).expect("compile fill");
        }
        assert_eq!(
            cache.len(),
            MDX_MODULE_CACHE_CAP,
            "filling exactly to the cap must not trigger a clear"
        );

        // One more entry: this should trigger a clear then re-insert → len 1.
        compile_mdx_to_jsx_module_cached("overflow_trigger\n", path, Some(&cache), None)
            .expect("compile overflow");
        assert_eq!(
            cache.len(),
            1,
            "overflow past CAP must clear the map; only the triggering entry remains"
        );
    }

    // ── zfb#954: same-file anchor validation ─────────────────────────────────

    // zfb#954: a valid same-file anchor passes; an invalid one is BrokenLink.
    #[test]
    fn armed_same_file_anchor_validates_against_local_registry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let body = "## Setup\n\n[ok](#setup)\n[bad](#nope)\n";
        let path = root.join("page.mdx");
        std::fs::write(&path, body).expect("write page");
        let feats = serde_json::json!({ "linkValidation": {} });

        let mut p = fs_features_pipeline(feats, root);
        compile_mdx_to_jsx_module_cached(body, &path, Some(&MdxModuleCache::new()), Some(&mut p))
            .expect("compile");
        let diags = p.take_markdown_diagnostics();
        assert_eq!(
            diags
                .iter()
                .filter(|d| matches!(d, MarkdownDiagnostic::BrokenLink { .. }))
                .count(),
            1,
            "exactly one BrokenLink expected: {diags:?}"
        );
        assert!(
            diags
                .iter()
                .any(|d| matches!(d, MarkdownDiagnostic::BrokenLink { url, .. } if url == "#nope")),
            "#nope must be reported as BrokenLink: {diags:?}"
        );
    }

    // zfb#954: a heading inside a JSX body has its id rendered and must
    // NOT produce a false-positive when linked from the same file.
    #[test]
    fn armed_same_file_anchor_inside_jsx_body_no_false_positive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        // The `<Note>` component is PascalCase — not mapped through
        // `_components`; it comes from the caller's `components` prop and
        // triggers a runtime throw if missing, but compile succeeds.
        let body = "<Note>\n\n## Inside\n\n</Note>\n\n[ok](#inside)\n";
        let path = root.join("page.mdx");
        std::fs::write(&path, body).expect("write page");
        let feats = serde_json::json!({ "linkValidation": {} });

        let mut p = fs_features_pipeline(feats, root);
        compile_mdx_to_jsx_module_cached(body, &path, Some(&MdxModuleCache::new()), Some(&mut p))
            .expect("compile");
        let diags = p.take_markdown_diagnostics();
        assert!(
            diags
                .iter()
                .all(|d| !matches!(d, MarkdownDiagnostic::BrokenLink { .. })),
            "JSX-nested heading anchor must not produce BrokenLink: {diags:?}"
        );
    }

    // zfb#954: a top-level h1 anchor passes silently even though no id
    // renders (h1 is intentionally left alone by HeadingLinksPlugin).
    #[test]
    fn armed_top_level_h1_anchor_passes_silently() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let body = "# Title\n\n[top](#title)\n";
        let path = root.join("page.mdx");
        std::fs::write(&path, body).expect("write page");
        let feats = serde_json::json!({ "linkValidation": {} });

        let mut p = fs_features_pipeline(feats, root);
        compile_mdx_to_jsx_module_cached(body, &path, Some(&MdxModuleCache::new()), Some(&mut p))
            .expect("compile");
        let diags = p.take_markdown_diagnostics();
        assert!(
            diags
                .iter()
                .all(|d| !matches!(d, MarkdownDiagnostic::BrokenLink { .. })),
            "h1-slug anchor must pass silently: {diags:?}"
        );
    }

    // zfb#954: with resolveMarkdownLinks ON, rewritten hrefs become
    // URL-space and are skipped — zero BrokenLink diagnostics for valid links.
    #[test]
    fn armed_url_space_href_no_diagnostic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        // Write the target file WITH a heading that can be linked.
        std::fs::write(root.join("other.md"), "## Existing Heading\n").expect("write other");
        let body = "[x](./other.md)\n[y](./other.md#existing-heading)\n";
        let path = root.join("page.mdx");
        std::fs::write(&path, body).expect("write page");
        let feats = serde_json::json!({ "linkValidation": {} });

        let mut p = fs_features_pipeline(feats, root);
        // Add ResolveLinksPlugin mapping other.md → a URL-space href.
        let mut map = HashMap::new();
        map.insert(root.join("other.md"), "/docs/other/".to_string());
        p.add_resolve_links(map);
        p.set_resolve_links_source_dir(root.to_path_buf());

        compile_mdx_to_jsx_module_cached(body, &path, Some(&MdxModuleCache::new()), Some(&mut p))
            .expect("compile");
        let diags = p.take_markdown_diagnostics();
        assert!(
            diags
                .iter()
                .all(|d| !matches!(d, MarkdownDiagnostic::BrokenLink { .. })),
            "URL-space hrefs after resolve must not report BrokenLink: {diags:?}"
        );
    }

    // zfb#954: same-file anchor verdict replays correctly on a cache hit.
    #[test]
    fn armed_same_file_anchor_verdict_replays_on_cache_hit() {
        let cache = MdxModuleCache::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let body = "## Setup\n\n[bad](#nope)\n";
        let path = root.join("page.mdx");
        std::fs::write(&path, body).expect("write page");
        let feats = serde_json::json!({ "linkValidation": {} });

        let mut p1 = fs_features_pipeline(feats.clone(), root);
        compile_mdx_to_jsx_module_cached(body, &path, Some(&cache), Some(&mut p1))
            .expect("compile 1");
        let fresh = p1.take_markdown_diagnostics();
        assert!(
            fresh
                .iter()
                .any(|d| matches!(d, MarkdownDiagnostic::BrokenLink { url, .. } if url == "#nope")),
            "fresh compile must report the broken anchor: {fresh:?}"
        );

        poke_sentinel(&cache);
        let mut p2 = fs_features_pipeline(feats, root);
        let second = compile_mdx_to_jsx_module_cached(body, &path, Some(&cache), Some(&mut p2))
            .expect("compile 2");
        assert_eq!(
            second.jsx_source, "__SENTINEL__",
            "must be a true cache hit"
        );
        assert_eq!(
            p2.take_markdown_diagnostics(),
            fresh,
            "BrokenLink must replay identically on the cache hit"
        );
    }

    // ── #977: cross-file anchor side channels ─────────────────────────────

    // #977 acceptance: a cache-hit compile replays IDENTICAL candidates +
    // headings as a fresh compile of the same input.
    #[test]
    fn cross_file_channels_replay_identically_on_cache_hit() {
        let cache = MdxModuleCache::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("other.md"), "## Target\n").expect("write target");
        let body = "## Local\n\n[x](./other.md#target)\n";
        let path = root.join("page.mdx");
        std::fs::write(&path, body).expect("write page");
        let feats = serde_json::json!({ "linkValidation": {} });

        let mut p1 = fs_features_pipeline(feats.clone(), root);
        compile_mdx_to_jsx_module_cached(body, &path, Some(&cache), Some(&mut p1))
            .expect("compile 1");
        let fresh_candidates = p1.take_cross_file_link_candidates();
        let fresh_headings = p1.take_file_headings();
        assert_eq!(
            fresh_candidates.len(),
            1,
            "the degrade branch must record the cross-file link: {fresh_candidates:?}"
        );
        let c = &fresh_candidates[0];
        assert_eq!(c.target_path, root.join("other.md"));
        assert_eq!(c.fragment, "target");
        assert_eq!(c.raw_href, "./other.md#target");
        assert_eq!(c.source_path, path);
        assert_eq!(
            fresh_headings.len(),
            1,
            "one record per compiled file: {fresh_headings:?}"
        );
        assert_eq!(
            fresh_headings[0].source_path,
            zfb_types::normalize_path_lexical(&path),
            "headings record is keyed by the shared-helper normal form"
        );
        assert!(
            fresh_headings[0].headings.iter().any(|h| h.id == "local"),
            "the compiled file's own headings must be surfaced: {fresh_headings:?}"
        );

        poke_sentinel(&cache);
        let mut p2 = fs_features_pipeline(feats, root);
        let second = compile_mdx_to_jsx_module_cached(body, &path, Some(&cache), Some(&mut p2))
            .expect("compile 2");
        assert_eq!(
            second.jsx_source, "__SENTINEL__",
            "must be a true cache hit"
        );
        assert_eq!(
            p2.take_cross_file_link_candidates(),
            fresh_candidates,
            "candidates must replay IDENTICALLY on the hit"
        );
        assert_eq!(
            p2.take_file_headings(),
            fresh_headings,
            "headings must replay IDENTICALLY on the hit"
        );
        assert!(
            p2.take_cross_file_link_candidates().is_empty() && p2.take_file_headings().is_empty(),
            "drain semantics: a second drain after the hit is empty"
        );
    }

    // #977: per-compile buffer slicing — two undrained compiles on ONE
    // pipeline (the snapshot-walker pattern: it never drains) must store
    // disjoint per-entry slices; a later hit on either entry replays only
    // that file's channel data, never the other's.
    #[test]
    fn channels_slice_per_compile_without_cross_file_leak() {
        let cache = MdxModuleCache::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("other.md"), "plain target\n").expect("write target");
        let body_a = "## Alpha Heading\n\n[a](./other.md#alpha)\n";
        let body_b = "## Beta Heading\n\n[b](./other.md#beta)\n";
        let path_a = root.join("a.mdx");
        let path_b = root.join("b.mdx");
        std::fs::write(&path_a, body_a).expect("write a");
        std::fs::write(&path_b, body_b).expect("write b");
        let feats = serde_json::json!({ "linkValidation": {} });

        // ONE pipeline, NO draining between compiles.
        let mut p = fs_features_pipeline(feats.clone(), root);
        compile_mdx_to_jsx_module_cached(body_a, &path_a, Some(&cache), Some(&mut p))
            .expect("compile a");
        p.reset_per_entry();
        compile_mdx_to_jsx_module_cached(body_b, &path_b, Some(&cache), Some(&mut p))
            .expect("compile b");
        assert_eq!(
            p.cross_file_link_candidates_len(),
            2,
            "undrained buffer holds both compiles' candidates"
        );
        assert_eq!(
            p.file_headings_len(),
            2,
            "undrained buffer holds both compiles' headings records"
        );

        poke_sentinel(&cache);
        // Hit on A replays ONLY A's slice.
        let mut p2 = fs_features_pipeline(feats.clone(), root);
        let hit_a = compile_mdx_to_jsx_module_cached(body_a, &path_a, Some(&cache), Some(&mut p2))
            .expect("hit a");
        assert_eq!(hit_a.jsx_source, "__SENTINEL__", "a must be a true hit");
        let a_candidates = p2.take_cross_file_link_candidates();
        let a_headings = p2.take_file_headings();
        assert_eq!(a_candidates.len(), 1, "{a_candidates:?}");
        assert_eq!(a_candidates[0].fragment, "alpha");
        assert_eq!(a_headings.len(), 1, "{a_headings:?}");
        assert_eq!(
            a_headings[0].source_path,
            zfb_types::normalize_path_lexical(&path_a)
        );
        assert!(a_headings[0]
            .headings
            .iter()
            .all(|h| h.id != "beta-heading"));

        // Hit on B replays ONLY B's slice.
        let mut p3 = fs_features_pipeline(feats, root);
        let hit_b = compile_mdx_to_jsx_module_cached(body_b, &path_b, Some(&cache), Some(&mut p3))
            .expect("hit b");
        assert_eq!(hit_b.jsx_source, "__SENTINEL__", "b must be a true hit");
        let b_candidates = p3.take_cross_file_link_candidates();
        let b_headings = p3.take_file_headings();
        assert_eq!(b_candidates.len(), 1, "{b_candidates:?}");
        assert_eq!(b_candidates[0].fragment, "beta");
        assert_eq!(b_headings.len(), 1, "{b_headings:?}");
        assert_eq!(
            b_headings[0].source_path,
            zfb_types::normalize_path_lexical(&path_b)
        );
    }

    // #977 acceptance: unarmed configs (no linkValidation) record NOTHING
    // in either channel — for both the truly-unarmed pipeline (no context
    // roots) and a context-armed pipeline without linkValidation.
    #[test]
    fn unarmed_configs_record_no_channel_data() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("other.md"), "## Target\n").expect("write target");
        let body = "## Local\n\n[x](./other.md#target)\n";
        let path = root.join("page.mdx");
        std::fs::write(&path, body).expect("write page");

        // (a) No features at all → context-free emit path.
        let mut p = full_config_pipeline(None);
        compile_mdx_to_jsx_module_cached(body, &path, Some(&MdxModuleCache::new()), Some(&mut p))
            .expect("compile unarmed");
        assert!(
            p.take_cross_file_link_candidates().is_empty(),
            "unarmed: no candidates"
        );
        assert!(p.take_file_headings().is_empty(), "unarmed: no headings");

        // (b) Context-armed but transclude-only (no linkValidation) —
        // the channels exist solely for the cross-file anchor check.
        let feats = serde_json::json!({ "transclude": {} });
        let mut p = fs_features_pipeline(feats, root);
        compile_mdx_to_jsx_module_cached(body, &path, Some(&MdxModuleCache::new()), Some(&mut p))
            .expect("compile transclude-only");
        assert!(
            p.take_cross_file_link_candidates().is_empty(),
            "no linkValidation: no candidates"
        );
        assert!(
            p.take_file_headings().is_empty(),
            "no linkValidation: no headings records"
        );
    }

    // #977: a heading-less file still records ONE (empty) headings record
    // — "compiled with zero headings" is a meaningful verdict for the
    // post-compile check, distinct from "never compiled".
    #[test]
    fn heading_less_file_records_empty_headings_record() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let body = "plain paragraph, no headings\n";
        let path = root.join("page.mdx");
        std::fs::write(&path, body).expect("write page");
        let feats = serde_json::json!({ "linkValidation": {} });

        let mut p = fs_features_pipeline(feats, root);
        compile_mdx_to_jsx_module_cached(body, &path, Some(&MdxModuleCache::new()), Some(&mut p))
            .expect("compile");
        let headings = p.take_file_headings();
        assert_eq!(headings.len(), 1, "{headings:?}");
        assert_eq!(
            headings[0].source_path,
            zfb_types::normalize_path_lexical(&path)
        );
        assert!(
            headings[0].headings.is_empty(),
            "zero headings, but the record itself must exist: {headings:?}"
        );
    }

    // ───────────────────────────────────────────────────────────────
    // RED characterization tests for GFM task-list checkbox emission,
    // Site 2 (#2022, Wave 1 of epic #2021 — superseding #1950):
    // `JsxEmitter::emit_node`'s `ListItem` arm (~line 892,
    // `self.emit_html("li", &[], &li.children)`) never reads
    // `ListItem.checked`.
    //
    // This site is unit-tested directly (constructing the mdast node
    // and calling `JsxEmitter::emit_node` by hand) rather than through
    // the crate's public API, because it is structurally unreachable
    // with `checked: Some(_)` through any public entry point today:
    // `JsxEmitter::emit_node` is only reached by `mdx_to_jsx_module_inner`
    // when NO pipeline is supplied (`take_hast_detour = pipeline_mut.is_some()`
    // — see that function), and the no-pipeline branch always resolves
    // GFM constructs to `ResolvedGfmConstructs::CONSERVATIVE`
    // (`task_list_item: false`, hard-coded a few lines above). Supplying
    // ANY pipeline — the only way to turn `task_list_item` on — forces
    // the hast detour instead, which routes a top-level list through
    // `pipeline::mdast_to_hast_inner`'s `ListItem` arm (Site 1), never
    // through this one. See `tests/gfm_task_list_red.rs` for Sites 1 and 3.
    //
    // Desired post-fix contract mirrors the other two sites: a checked
    // item's checkbox carries a `checked` marker, an unchecked item's
    // does not, both are `disabled` (static, server-rendered output),
    // and item text survives alongside the checkbox.
    fn task_list_item_node(checked: Option<bool>, text: &str) -> MdastNode {
        MdastNode::ListItem(markdown::mdast::ListItem {
            children: vec![MdastNode::Paragraph(markdown::mdast::Paragraph {
                children: vec![MdastNode::Text(markdown::mdast::Text {
                    value: text.to_string(),
                    position: None,
                })],
                position: None,
            })],
            position: None,
            spread: false,
            checked,
        })
    }

    #[test]
    fn emit_node_checked_task_list_item_emits_checked_checkbox() {
        let li = task_list_item_node(Some(true), "Buy milk");
        let mut emitter = JsxEmitter::new();
        let out = emitter.emit_node(&li);
        assert!(
            out.contains("type=\"checkbox\""),
            "checked task-list item must emit a checkbox input; got:\n{out}"
        );
        assert!(
            out.contains("checked"),
            "checked task-list item's checkbox must carry a checked marker; got:\n{out}"
        );
        assert!(
            out.contains("disabled"),
            "the emitted checkbox is static (server-rendered) and must be disabled; got:\n{out}"
        );
        assert!(
            out.contains("Buy milk"),
            "item text must still be present alongside the checkbox; got:\n{out}"
        );
    }

    #[test]
    fn emit_node_unchecked_task_list_item_emits_unchecked_checkbox() {
        let li = task_list_item_node(Some(false), "Buy milk");
        let mut emitter = JsxEmitter::new();
        let out = emitter.emit_node(&li);
        assert!(
            out.contains("type=\"checkbox\""),
            "unchecked task-list item must still emit a checkbox input; got:\n{out}"
        );
        assert!(
            !out.contains("checked"),
            "unchecked task-list item's checkbox must NOT carry a checked marker; got:\n{out}"
        );
        assert!(
            out.contains("disabled"),
            "the emitted checkbox is static (server-rendered) and must be disabled; got:\n{out}"
        );
        assert!(
            out.contains("Buy milk"),
            "item text must still be present alongside the checkbox; got:\n{out}"
        );
    }

    /// Control (NOT ignored — must pass today and after the fix): an
    /// ordinary list item (`checked: None`, i.e. not part of a task
    /// list at all) must never grow a checkbox. Pins that the fix is
    /// scoped to `checked.is_some()` and does not regress plain lists.
    #[test]
    fn emit_node_plain_list_item_never_emits_a_checkbox() {
        let li = task_list_item_node(None, "Buy milk");
        let mut emitter = JsxEmitter::new();
        let out = emitter.emit_node(&li);
        assert!(
            !out.contains("type=\"checkbox\""),
            "a plain (non-task-list) list item must never emit a checkbox; got:\n{out}"
        );
        assert!(
            out.contains("Buy milk"),
            "item text must still be present; got:\n{out}"
        );
    }

    // -----------------------------------------------------------------
    // Footnote RED tests (issue #2023, epic #2021: GFM Footnotes And
    // Task Lists) — the JSX converter path.
    //
    // The real production path for `.mdx` content always supplies a
    // `Pipeline` (`compile_mdx_to_jsx_module_cached` / the bundler / dev
    // loader), which takes the hast detour: `mdast_to_hast_with` (shared
    // with `pipeline.rs`'s `Pipeline::run`, see the mirrored tests
    // there) then this file's `emit_node(&HastNode)`. The no-pipeline
    // legacy entry point (`mdx_to_jsx_module` with no pipeline) always
    // forces `ResolvedGfmConstructs::CONSERVATIVE`
    // (`footnote_definition: false`) with no way to override it, so
    // footnote syntax never parses into `FootnoteReference`/
    // `FootnoteDefinition` nodes on that path — its `emit_node(&MdastNode)`
    // catch-all (~line 953) is unreachable for footnotes through the
    // public API and is deliberately NOT exercised here.
    //
    // What IS reachable and broken in THIS file specifically:
    // `jsx_render_child`'s own catch-all (~line 1795), fired when a
    // footnote reference/definition is nested INSIDE an MDX JSX
    // element's body (`<Note>…[^a]…</Note>`) — see the `jsx_nested_*`
    // tests below, tagged for #2027. The ordinary (non-nested) cases
    // mirror `pipeline.rs`'s test names/fixtures exactly and are tagged
    // for #2026, since they exercise the SAME shared catch-all in
    // `mdast_to_hast_inner`, just observed through the JSX string
    // output instead of a `HastNode` tree.

    /// Pipeline with every GFM construct on, including
    /// `footnote_definition`.
    fn footnote_pipeline() -> Pipeline {
        Pipeline::with_resolved_gfm_constructs(ResolvedGfmConstructs::ALL_ON)
    }

    fn emit_with_footnotes(src: &str) -> String {
        let mut p = footnote_pipeline();
        mdx_to_jsx_module_with_pipeline(src, MdxJsxOptions::default(), &mut p).expect("emit ok")
    }

    /// Extract every `key="VALUE"` occurrence's VALUE from a JSX source
    /// string, in left-to-right (document) order. A tiny hand-rolled
    /// scanner — this crate has no regex dependency, and the exact
    /// id/href STRING scheme is #2025's policy call, not something
    /// these tests should hardcode.
    fn extract_attr_values(jsx: &str, key: &str) -> Vec<String> {
        let needle = format!("{key}=\"");
        let mut out = Vec::new();
        let mut rest = jsx;
        while let Some(idx) = rest.find(needle.as_str()) {
            let after = &rest[idx + needle.len()..];
            match after.find('"') {
                Some(end) => {
                    out.push(after[..end].to_string());
                    rest = &after[end + 1..];
                }
                None => break,
            }
        }
        out
    }

    /// Extract a `key="VALUE"` attribute value from the innermost JSX
    /// opening tag that ENCLOSES byte offset `pos` in `jsx` — i.e. the
    /// nearest `<tag …>` immediately before `pos` with no intervening
    /// `>`. Used to tie a specific visible marker occurrence (e.g. the
    /// SECOND `{"1"}`) to the `id`/`href` on the exact element that
    /// renders it, rather than to "some id/href anywhere in the
    /// document" (codex review flagged the weaker document-wide
    /// version as unable to distinguish a correct per-occurrence
    /// backreference wiring from a broken one that merely produces the
    /// right COUNT of ids/hrefs).
    fn enclosing_tag_attr(jsx: &str, pos: usize, key: &str) -> Option<String> {
        let before = &jsx[..pos];
        let tag_start = before.rfind('<')?;
        let tag_end = jsx[tag_start..].find('>').map(|i| tag_start + i)?;
        extract_attr_values(&jsx[tag_start..tag_end], key)
            .into_iter()
            .next()
    }

    // 1. A single reference and its definition are ASSOCIATED — mirrors
    // `pipeline::tests::footnote_reference_and_definition_are_associated`.
    // Strengthened per codex review: the ORIGINAL version only checked
    // that "every fragment href resolves to SOME id anywhere in the
    // document", which an implementation emitting only a
    // marker-to-definition link (and no backreference at all) would
    // satisfy. This version ties the marker's OWN id to a backreference
    // href specifically targeting it, mirroring the pipeline.rs test's
    // structure.
    #[test]
    fn jsx_footnote_reference_and_definition_are_associated() {
        let jsx = emit_with_footnotes("Ref one[^a] end.\n\n[^a]: Definition body.\n");

        let marker_pos = jsx
            .find("{\"1\"}")
            .unwrap_or_else(|| panic!("expected the visible footnote marker \"1\": {jsx}"));
        assert!(
            jsx.contains("{\"Definition body.\"}"),
            "footnote definition body missing from emitted JSX: {jsx}"
        );

        let marker_id = enclosing_tag_attr(&jsx, marker_pos, "id").unwrap_or_else(|| {
            panic!("reference marker's own enclosing tag must carry an id=: {jsx}")
        });
        let marker_href = enclosing_tag_attr(&jsx, marker_pos, "href").unwrap_or_else(|| {
            panic!("reference marker's own enclosing tag must carry an href=: {jsx}")
        });
        assert!(
            marker_href.starts_with('#'),
            "reference marker href must be a fragment link, got {marker_href:?}: {jsx}"
        );

        // The marker's href must resolve to SOME rendered id= (the
        // definition's target)…
        let ids = extract_attr_values(&jsx, "id");
        let def_target = marker_href.trim_start_matches('#');
        assert!(
            ids.iter().any(|id| id == def_target),
            "the reference marker's href ({marker_href}) does not resolve to \
             any rendered id= — it must point at the rendered definition: \
             ids={ids:?} jsx={jsx}"
        );

        // …AND a backreference href must exist SPECIFICALLY targeting
        // the marker's OWN id (not just any id in the document) —
        // proves the definition can link back to THIS occurrence.
        let expected_backref = format!("#{marker_id}");
        let hrefs = extract_attr_values(&jsx, "href");
        assert!(
            hrefs.iter().any(|h| h == &expected_backref),
            "no backreference href={expected_backref} found pointing back at \
             the marker's own id={marker_id} — reference and definition must \
             link to EACH OTHER, not just one direction: hrefs={hrefs:?} jsx={jsx}"
        );
    }

    // 2. Repeated references to ONE definition need distinct
    // backreference targets — mirrors
    // `pipeline::tests::repeated_references_get_distinct_backreference_targets`.
    // Strengthened per codex review: the ORIGINAL version only counted
    // ids/hrefs document-wide, which an implementation giving only the
    // FIRST occurrence a working backreference (the second rendering
    // unusable) could still satisfy by coincidence (definition id +
    // first-marker id = 2 unique ids; def-to-first-marker link +
    // marker-to-definition link = 2 unique hrefs). This version ties
    // EACH of the two visible marker occurrences to its OWN id and its
    // OWN backreference specifically.
    #[test]
    fn jsx_repeated_references_get_distinct_backreference_targets() {
        let jsx = emit_with_footnotes("Ref one[^a] and ref again[^a] end.\n\n[^a]: Shared def.\n");

        let marker_count = jsx.matches("{\"1\"}").count();
        assert_eq!(
            marker_count, 2,
            "expected exactly two visible footnote-1 markers (same number, \
             two occurrences), got {marker_count}: {jsx}"
        );

        let mut marker_ids = Vec::new();
        let mut rest = jsx.as_str();
        let mut offset = 0usize;
        while let Some(rel) = rest.find("{\"1\"}") {
            let pos = offset + rel;
            let id = enclosing_tag_attr(&jsx, pos, "id").unwrap_or_else(|| {
                panic!("marker occurrence at byte {pos} must carry its own id=: {jsx}")
            });
            marker_ids.push(id);
            offset = pos + "{\"1\"}".len();
            rest = &jsx[offset..];
        }
        assert_eq!(
            marker_ids.len(),
            2,
            "expected two marker ids, got {marker_ids:?}"
        );
        assert_ne!(
            marker_ids[0], marker_ids[1],
            "each repeated reference occurrence must get its own distinct \
             backreference id, got the same id twice: {marker_ids:?}"
        );

        let hrefs = extract_attr_values(&jsx, "href");
        for id in &marker_ids {
            let expected_backref = format!("#{id}");
            assert!(
                hrefs.iter().any(|h| h == &expected_backref),
                "no backreference href={expected_backref} found for marker \
                 id={id} — EACH occurrence needs its own working \
                 backreference, not just a document-wide id/href count \
                 match: hrefs={hrefs:?} jsx={jsx}"
            );
        }
    }

    // 3. Multiple definitions: numbering and order follow REFERENCE
    // order — mirrors
    // `pipeline::tests::multiple_definitions_are_numbered_and_ordered_by_first_reference`.
    #[test]
    fn jsx_multiple_definitions_are_numbered_and_ordered_by_first_reference() {
        let jsx = emit_with_footnotes(
            "First[^a] then second[^b] end.\n\n[^b]: Second body.\n\n[^a]: First body.\n",
        );

        let one_pos = jsx
            .find("{\"1\"}")
            .unwrap_or_else(|| panic!("marker \"1\" missing from emitted JSX: {jsx}"));
        let two_pos = jsx
            .find("{\"2\"}")
            .unwrap_or_else(|| panic!("marker \"2\" missing from emitted JSX: {jsx}"));
        assert!(
            one_pos < two_pos,
            "the FIRST-referenced footnote (`a`) must be numbered 1 and its \
             marker must appear before the SECOND-referenced footnote's \
             marker \"2\": jsx={jsx}"
        );

        let a_pos = jsx
            .find("First body.")
            .unwrap_or_else(|| panic!("First body. missing from emitted JSX: {jsx}"));
        let b_pos = jsx
            .find("Second body.")
            .unwrap_or_else(|| panic!("Second body. missing from emitted JSX: {jsx}"));
        assert!(
            a_pos < b_pos,
            "footnote definitions must render in REFERENCE order (a before \
             b): jsx={jsx}"
        );
    }

    // 4. Duplicate `[^a]` definitions collapse to exactly one entry —
    // mirrors
    // `pipeline::tests::duplicate_definitions_collapse_to_exactly_one_entry`.
    // WHICH body wins is #2025's policy call; this test only pins the
    // "exactly one, not two" structural fact.
    #[test]
    fn jsx_duplicate_definitions_collapse_to_exactly_one_entry() {
        let jsx =
            emit_with_footnotes("Dup label[^a] end.\n\n[^a]: First.\n\n[^a]: Second (dup).\n");

        let has_first = jsx.contains("First.");
        let has_second = jsx.contains("Second (dup).");
        assert!(
            has_first ^ has_second,
            "exactly ONE of the duplicate definition bodies must survive \
             (the tie-break is #2025's policy call) — got first={has_first} \
             second={has_second}: {jsx}"
        );

        let marker_count = jsx.matches("{\"1\"}").count();
        assert_eq!(
            marker_count, 1,
            "duplicate definitions must still yield exactly one reference \
             marker \"1\", got {marker_count}: {jsx}"
        );
    }

    // 5. A reference with NO matching definition stays literal text —
    // parser-level fact (see the mirrored, non-ignored pin in
    // `pipeline::tests::unmatched_reference_stays_literal_text` for the
    // full rationale). Passing today; not a RED test.
    #[test]
    fn jsx_unmatched_reference_stays_literal_text() {
        let jsx = emit_with_footnotes("Dangling ref[^missing] end.\n");
        assert!(
            jsx.contains("[^missing]"),
            "unmatched footnote reference must stay literal text in the \
             emitted JSX: {jsx}"
        );
    }

    // 6. A footnote reference AND its definition nested inside an MDX
    // JSX element's body (`<Note>…</Note>`) go through this file's OWN
    // `jsx_render_child` catch-all (~line 1795), not the shared
    // `mdast_to_hast_inner` one — confirmed via a throwaway
    // `zfb-content` example during this issue's investigation: both
    // `FootnoteReference` and `FootnoteDefinition` parse as direct
    // children of the `MdxJsxFlowElement` node when nested this way.
    // #2027's acceptance criteria calls this out by name ("a footnote
    // reference inside JSX children").
    #[test]
    fn jsx_nested_footnote_reference_and_definition_inside_jsx_element_are_not_dropped() {
        let jsx = emit_with_footnotes("<Note>\n\nRef[^a] end.\n\n[^a]: Body text.\n\n</Note>\n");

        assert!(
            jsx.contains("{\"1\"}"),
            "a footnote reference nested inside an MDX JSX element body must \
             still render its visible marker \"1\" — jsx_render_child's \
             catch-all drops it today: {jsx}"
        );
        assert!(
            jsx.contains("Body text."),
            "a footnote definition nested inside an MDX JSX element body \
             must still render its content: {jsx}"
        );
    }

    // 7. A footnote DEFINITION whose body contains JSX (`<Bold>…</Bold>`)
    // — the reverse nesting #2027's acceptance criteria also calls out
    // by name. The `FootnoteDefinition` itself is top-level here (so
    // the emission fix mechanically lands via #2026's shared catch-all),
    // but confirming the nested JSX content survives is explicitly
    // #2027's stated scope, so it stays tagged for #2027's confirm pass
    // rather than #2026's.
    #[test]
    fn jsx_footnote_definition_body_containing_jsx_is_not_dropped() {
        let jsx =
            emit_with_footnotes("Ref[^a] end.\n\n[^a]: Body with <Bold>emphasis</Bold> inside.\n");

        assert!(
            jsx.contains("Body with"),
            "footnote definition body must survive: {jsx}"
        );
        assert!(
            jsx.contains("emphasis"),
            "JSX content nested inside a footnote definition body must \
             survive: {jsx}"
        );
        assert!(
            jsx.contains("Bold"),
            "the JSX component name nested inside a footnote definition \
             body must survive (not just its text content): {jsx}"
        );
    }

    // 8. `jsx_render_child`'s catch-all still swallows what it legitimately
    // should — mirrors `pipeline::tests::catch_all_still_swallows_reference_style_link_definitions`,
    // but exercised through THIS file's own catch-all by nesting the
    // reference-style link definition inside an MDX JSX element body
    // (`<Note>…</Note>`), the one shape that routes through
    // `jsx_render_child` rather than the shared `mdast_to_hast_inner`
    // catch-all. Carving footnotes out of the catch-all (this wave)
    // must not weaken or remove it for everything else it drops.
    #[test]
    fn jsx_catch_all_still_swallows_reference_style_link_definitions_nested_in_jsx() {
        let jsx = emit_with_footnotes(
            "<Note>\n\nPara text.\n\n[label]: /elsewhere \"Title\"\n\n</Note>\n",
        );

        assert!(
            jsx.contains("Para text."),
            "the surrounding paragraph must still render: {jsx}"
        );
        assert!(
            !jsx.contains("/elsewhere") && !jsx.contains("Title"),
            "a reference-style link definition nested inside an MDX JSX \
             element body must still be silently dropped by \
             jsx_render_child's catch-all, got: {jsx}"
        );
    }

    // 9. `gfm: false` (footnote_definition off) leaves footnote syntax
    // nested inside an MDX JSX element body as literal text, with no
    // footnote-shaped markup anywhere — mirrors
    // `pipeline::tests::gfm_false_leaves_footnote_syntax_as_literal_text_with_no_section_appended`,
    // but through THIS file's `jsx_render_child` path (a `Pipeline`
    // whose GFM constructs default to `CONSERVATIVE`, i.e.
    // `Pipeline::new()`, never turns on `footnote_definition`, so
    // markdown-rs never produces a `FootnoteReference`/
    // `FootnoteDefinition` node at all — the same structural guarantee
    // the task-list wave relied on for its own `gfm: false` byte-parity
    // proof, not something this emitter has to re-derive).
    #[test]
    fn jsx_gfm_false_leaves_nested_footnote_syntax_as_literal_text() {
        let mut p = Pipeline::new();
        let jsx = mdx_to_jsx_module_with_pipeline(
            "<Note>\n\nRef[^a] end.\n\n[^a]: Body text.\n\n</Note>\n",
            MdxJsxOptions::default(),
            &mut p,
        )
        .expect("emit ok");

        assert!(
            jsx.contains("[^a]") && jsx.contains("[^a]: Body text."),
            "footnote syntax must survive as literal text nested inside an \
             MDX JSX element body when footnote_definition is off, got: {jsx}"
        );
        assert!(
            !jsx.contains("data-footnote-ref")
                && !jsx.contains("data-footnote-backref")
                && !jsx.contains("data-footnotes"),
            "no footnote-shaped attribute may appear anywhere when the \
             construct is off: {jsx}"
        );
    }

    // ---- the two JSX emit sites must not diverge (epic #2021 review) ----
    //
    // The epic's stated purpose is that a user switching a page between
    // `.md` and `.mdx` sees no behavioural difference. The same rule binds
    // the two JSX-emit sites WITHIN one `.mdx`: a footnote or task list
    // that happens to sit inside a JSX component's children goes through
    // `jsx_render_child`, while one at the document's top level goes
    // through the hast bridge. The three tests below pin the spellings
    // that used to differ.

    /// The exact checkbox literal both JSX sites must produce. Bare
    /// boolean attributes (`disabled`/`checked` are real HTML booleans and
    /// must not be falsy props), `type`-`disabled`-`checked` order, and one
    /// `{" "}` separating the checkbox from its label.
    const CHECKED_CHECKBOX_JSX: &str =
        "<_components.input type=\"checkbox\" disabled checked />{\" \"}";
    const UNCHECKED_CHECKBOX_JSX: &str = "<_components.input type=\"checkbox\" disabled />{\" \"}";

    const TASK_AND_FOOTNOTE_SRC: &str =
        "- [ ] Todo\n- [x] Done\n\nRef[^a] end.\n\n[^a]: Body text.\n";

    #[test]
    fn both_jsx_emit_sites_spell_the_footnote_reference_marker_identically() {
        let top_level = emit_with_footnotes(TASK_AND_FOOTNOTE_SRC);
        let nested = emit_with_footnotes(&format!("<Note>\n\n{TASK_AND_FOOTNOTE_SRC}\n</Note>\n"));

        for (label, jsx) in [("top-level", &top_level), ("nested", &nested)] {
            assert!(
                jsx.contains("data-footnote-ref=\"\""),
                "the {label} path must spell the marker attribute as \
                 data-footnote-ref=\"\": {jsx}"
            );
            // The bare JSX shorthand means `true`, which serializes as the
            // visibly different `data-footnote-ref="true"` — the exact
            // divergence this pins against.
            assert!(
                !jsx.contains("data-footnote-ref "),
                "the {label} path must not emit the bare boolean-attribute \
                 shorthand for data-footnote-ref (JSX `true`, which \
                 serializes as data-footnote-ref=\"true\"): {jsx}"
            );
        }
    }

    #[test]
    fn both_jsx_emit_sites_spell_the_task_list_checkbox_identically() {
        let top_level = emit_with_footnotes(TASK_AND_FOOTNOTE_SRC);
        let nested = emit_with_footnotes(&format!("<Note>\n\n{TASK_AND_FOOTNOTE_SRC}\n</Note>\n"));

        for (label, jsx) in [("top-level", &top_level), ("nested", &nested)] {
            assert!(
                jsx.contains(UNCHECKED_CHECKBOX_JSX),
                "the {label} path must emit {UNCHECKED_CHECKBOX_JSX:?}: {jsx}"
            );
            assert!(
                jsx.contains(CHECKED_CHECKBOX_JSX),
                "the {label} path must emit {CHECKED_CHECKBOX_JSX:?}: {jsx}"
            );
        }
    }

    #[test]
    fn both_jsx_emit_sites_put_the_checkbox_inside_the_items_own_paragraph() {
        // A checkbox emitted as a SIBLING before the item's `<p>` renders on
        // its own line ABOVE the label, because neither emit path unwraps
        // tight-list paragraphs. It must open the paragraph instead, so the
        // checkbox reads as a checkbox beside its text (as GitHub renders
        // one).
        let top_level = emit_with_footnotes(TASK_AND_FOOTNOTE_SRC);
        let nested = emit_with_footnotes(&format!("<Note>\n\n{TASK_AND_FOOTNOTE_SRC}\n</Note>\n"));

        for (label, jsx) in [("top-level", &top_level), ("nested", &nested)] {
            assert!(
                jsx.contains(&format!(
                    "<_components.li><_components.p>{UNCHECKED_CHECKBOX_JSX}"
                )),
                "the {label} path must open the item's own paragraph with the \
                 checkbox, never place it as a sibling before it: {jsx}"
            );
            assert!(
                !jsx.contains(&format!("<_components.li>{UNCHECKED_CHECKBOX_JSX}")),
                "the {label} path must not emit the checkbox as a sibling \
                 before the item's paragraph: {jsx}"
            );
        }
    }

    #[test]
    fn jsx_style_attr_emits_a_react_compatible_object_prop() {
        assert_eq!(
            jsx_style_attr("text-align: center"),
            " style={{\"textAlign\": \"center\"}}"
        );
        // Custom properties keep their exact name — React and Preact both
        // read `--x` verbatim, and camelCasing would break them.
        assert_eq!(
            jsx_style_attr("--shiki-dark-bg:#111;color:#eee"),
            " style={{\"--shiki-dark-bg\": \"#111\", \"color\": \"#eee\"}}"
        );
        // A value may contain `:` and `,` (`clip: rect(0,0,0,0)`); only the
        // FIRST `:` separates name from value.
        assert_eq!(
            jsx_style_attr("clip:rect(0,0,0,0)"),
            " style={{\"clip\": \"rect(0,0,0,0)\"}}"
        );
        // Nothing parseable → the attribute is omitted entirely rather
        // than emitted empty.
        assert_eq!(jsx_style_attr(""), "");
        assert_eq!(jsx_style_attr(";  ;"), "");
        assert_eq!(jsx_style_attr("novalue"), "");
    }

    #[test]
    fn the_footnote_label_is_hidden_by_an_inline_style_not_a_project_css_class() {
        // `sr-only` is a Tailwind utility, and this class is emitted from
        // Rust — Tailwind's content scan never sees the string, so the
        // utility is never generated; zfb ships no stylesheet defining it
        // either. The inline style is what actually makes the documented
        // "visually hidden" landmark true.
        let jsx = emit_with_footnotes(TASK_AND_FOOTNOTE_SRC);
        // Emitted as a style OBJECT, never a string — React throws on a
        // string `style` prop. `jsx_style_attr` camelCases the hyphenated
        // property names on the way through.
        assert!(
            jsx.contains(&jsx_style_attr(FOOTNOTE_LABEL_STYLE)),
            "the footnote label must carry the visually-hidden inline style: {jsx}"
        );
        assert!(
            jsx.contains("\"whiteSpace\": \"nowrap\"") && !jsx.contains("white-space"),
            "hyphenated CSS property names must be camelCased for React: {jsx}"
        );
        assert!(
            jsx.contains("class=\"sr-only\""),
            "the sr-only class stays as a styling hook alongside the inline \
             style: {jsx}"
        );
    }
}
