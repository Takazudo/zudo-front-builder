//! Tests for `mdx_to_jsx_module_with_pipeline` (#121).
//!
//! These tests exercise the hast-detour wired in for issue #121: when
//! the JSX emit path is given a [`Pipeline`], it now runs both mdast
//! AND hast visitors against the parsed tree. Today the HTML serializer
//! path (`Pipeline::run`) and the JSX emit path share the same
//! plugin chain end-to-end.
//!
//! Coverage matrix (one test per default plugin shipped by
//! `Pipeline::with_defaults`, plus one for the opt-in
//! `add_strip_md_ext()`, plus one MDX-passthrough sanity case):
//!
//! 1. heading-links: `<h2>`–`<h6>` get an `id` attribute and an empty
//!    permalink anchor child.
//! 2. code-title: `<pre data-meta='title="…"'>` is wrapped in a
//!    `code-block-container` div.
//! 3. mermaid: a fenced ```mermaid block becomes a
//!    `<div class="mermaid" data-mermaid>…</div>`.
//! 4. syntect: a non-mermaid fenced code block routes through
//!    syntect (output reaches the JSX module wrapped in
//!    `dangerouslySetInnerHTML`).
//! 5. strip-md-ext (opt-in): internal `[x](./guide.md)` links lose
//!    the `.md` and gain a trailing slash on the JSX path.
//! 6. MDX component passthrough: a `<Note>` reference still triggers
//!    the PascalCase preamble and survives in the JSX body.

use zfb_content::pipeline::Pipeline;
use zfb_content::{mdx_to_jsx_module_with_pipeline, MdxJsxOptions};
use zfb_md_ast::TocExportConfig;
use zfb_render::{CompileOptions, SwcPipeline};

fn emit_with_defaults(src: &str) -> String {
    let mut p = Pipeline::with_defaults();
    mdx_to_jsx_module_with_pipeline(src, MdxJsxOptions::default(), &mut p)
        .expect("pipeline emit ok")
}

fn emit_with_defaults_strip_md(src: &str) -> String {
    let mut p = Pipeline::with_defaults();
    p.add_strip_md_ext();
    mdx_to_jsx_module_with_pipeline(src, MdxJsxOptions::default(), &mut p)
        .expect("pipeline emit ok")
}

#[test]
fn heading_links_plugin_fires_on_jsx_path() {
    // h2 must get an id="…" attribute AND an empty permalink anchor
    // child with class="hash-link".
    let out = emit_with_defaults("## Hello World\n");
    assert!(
        out.contains("id=\"hello-world\""),
        "heading should get id slug from heading-links plugin:\n{out}",
    );
    assert!(
        out.contains("class=\"hash-link\""),
        "heading should get hash-link anchor child:\n{out}",
    );
    assert!(
        out.contains("aria-label=\"Direct link to Hello World\""),
        "heading anchor should carry aria-label:\n{out}",
    );
    // h1 stays untouched (heading-links never sees it).
    let out = emit_with_defaults("# Page Title\n");
    assert!(
        !out.contains("id=\"page-title\""),
        "h1 must NOT receive an id:\n{out}",
    );
}

#[test]
fn code_title_plugin_fires_on_jsx_path() {
    let out = emit_with_defaults("```rust title=\"main.rs\"\nfn main() {}\n```\n");
    assert!(
        out.contains("class=\"code-block-container\""),
        "code-title plugin should wrap titled <pre> in a container:\n{out}",
    );
    assert!(
        out.contains("class=\"code-block-title\""),
        "code-title plugin should emit a title bar:\n{out}",
    );
    // Title text survives.
    assert!(
        out.contains("\"main.rs\""),
        "title text should survive into the JSX body:\n{out}",
    );
}

#[test]
fn mermaid_plugin_fires_on_jsx_path() {
    let out = emit_with_defaults("```mermaid\ngraph TD;\n  A-->B;\n```\n");
    assert!(
        out.contains("class=\"mermaid\""),
        "mermaid plugin should produce a mermaid div:\n{out}",
    );
    assert!(
        out.contains("data-mermaid"),
        "mermaid plugin should emit the data-mermaid attribute:\n{out}",
    );
    // The original code/pre shell must NOT survive — the mermaid
    // plugin replaces the entire <pre>.
    assert!(
        !out.contains("language-mermaid"),
        "language-mermaid class must not leak past mermaid plugin:\n{out}",
    );
    // Diagram source text survives via the JS string literal route.
    assert!(
        out.contains("graph TD;"),
        "mermaid source text should survive into the JSX body:\n{out}",
    );
}

#[test]
fn syntect_plugin_fires_on_jsx_path() {
    // syntect emits HTML, which the bridge wraps in
    // dangerouslySetInnerHTML — both markers must show up.
    let out = emit_with_defaults("```rust\nfn main() {}\n```\n");
    assert!(
        out.contains("dangerouslySetInnerHTML"),
        "syntect HTML output must be embedded via dangerouslySetInnerHTML:\n{out}",
    );
    assert!(
        out.contains("syntect-"),
        "syntect class hook must reach the JSX body:\n{out}",
    );
}

#[test]
fn strip_md_ext_plugin_fires_on_jsx_path() {
    // Opt-in via `add_strip_md_ext()` after `with_defaults()`. The
    // default trailing-slash mode should rewrite ./guide.md → ./guide/.
    let out = emit_with_defaults_strip_md("[x](./guide.md)\n");
    assert!(
        out.contains("href=\"./guide/\""),
        "strip-md-ext should rewrite internal .md to ./guide/:\n{out}",
    );
    assert!(
        !out.contains("./guide.md"),
        ".md must be stripped from internal hrefs:\n{out}",
    );
}

#[test]
fn mdx_component_passthrough_survives_hast_detour() {
    // A PascalCase MDX component reference must still trigger the
    // preamble and survive verbatim in the JSX body. This is the
    // sanity case for the hast detour: the JsxRaw flavour of hast
    // raw nodes preserves JSX semantics across the detour.
    let out = emit_with_defaults("<Note>hello</Note>\n");
    assert!(
        out.contains("const Note = _components.Note ?? components.Note;"),
        "PascalCase preamble must be emitted for <Note>:\n{out}",
    );
    assert!(
        out.contains("if (!Note) throw new Error("),
        "PascalCase guard must be emitted for <Note>:\n{out}",
    );
    assert!(
        out.contains("<Note>") && out.contains("</Note>"),
        "<Note> opening/closing tags must survive the detour:\n{out}",
    );
}

#[test]
fn bare_pipeline_runs_no_plugins_on_jsx_path() {
    // `Pipeline::with_mdx()` ships zero visitors. The hast detour
    // still runs (we built a tree, walked it, emitted JSX), but no
    // visitor mutated it — so heading-links / code-title / etc. must
    // NOT fire.
    let mut p = Pipeline::with_mdx();
    let out = mdx_to_jsx_module_with_pipeline("## Hello\n", MdxJsxOptions::default(), &mut p)
        .expect("emit ok");
    assert!(
        !out.contains("id=\"hello\""),
        "bare pipeline must NOT add heading-links id:\n{out}",
    );
    assert!(
        !out.contains("class=\"hash-link\""),
        "bare pipeline must NOT add hash-link anchor:\n{out}",
    );
}

#[test]
fn defaults_compose_for_titled_rust_block() {
    // code-title MUST run before syntect: the title wrapper survives
    // and the inner <pre> becomes syntect HTML inside dangerously-set
    // inner HTML.
    let out = emit_with_defaults("```rust title=\"main.rs\"\nfn main() {}\n```\n");
    assert!(
        out.contains("class=\"code-block-container\""),
        "container must survive composition:\n{out}",
    );
    assert!(
        out.contains("class=\"code-block-title\""),
        "title bar must survive composition:\n{out}",
    );
    assert!(
        out.contains("dangerouslySetInnerHTML"),
        "syntect output must be embedded via dangerouslySetInnerHTML:\n{out}",
    );
    assert!(
        out.contains("syntect-"),
        "syntect class hook must reach the JSX body:\n{out}",
    );
}

/// SWC-acceptance smoke: every plugin's output (including the
/// dangerouslySetInnerHTML wrap and the JsxRaw passthrough) must
/// produce a valid TSX module that survives SWC's JSX transform.
#[test]
fn jsx_with_hast_detour_compiles_via_swc() {
    let pipeline_compile = SwcPipeline::new();

    // Cover heading-links + syntect + code-title +
    // mermaid + an MDX component in one document so the smoke covers
    // the full default chain end-to-end.
    let src = "# Title\n\
        \n\
        ## Section\n\
        \n\
        Welcome to **bold** text.\n\
        \n\
        <Note>callout body</Note>\n\
        \n\
        ![alt](pic.png)\n\
        \n\
        ```rust title=\"main.rs\"\n\
        fn main() {}\n\
        ```\n\
        \n\
        ```mermaid\n\
        graph TD;\n\
          A-->B;\n\
        ```\n";
    let out = emit_with_defaults(src);
    let opts = CompileOptions::default().with_filename("hast-detour.tsx".to_string());
    let compiled = pipeline_compile
        .compile(&out, &opts)
        .unwrap_or_else(|e| panic!("SWC rejected hast-detour output: {e}\n--- src ---\n{out}"));
    assert!(
        compiled.code.contains("MDXContent"),
        "compiled output missing MDXContent default export:\n{}",
        compiled.code,
    );
    // JSX is fully desugared.
    assert!(
        !compiled.code.contains("<_Fragment>"),
        "JSX leaked through SWC:\n{}",
        compiled.code,
    );
}

/// Real-world `<Note>\n\nbody **bold** body\n\n</Note>` shape — the
/// markdown inside the MDX block must survive into the JSX output so
/// the rendered DOM still gets the bold formatting. This is the
/// regression Codex caught in the first round of #121 review: the old
/// `reconstruct_jsx` fallback `other.to_string()` produced a debug
/// string instead of recursing into Strong/Emphasis/etc.
#[test]
fn mdx_jsx_block_with_markdown_body_preserves_formatting() {
    let out = emit_with_defaults(
        "<Note>\n\nThis is **bold** and *italic* text with `code`.\n\n</Note>\n",
    );
    // The body must remain visible (not replaced by a debug stringification).
    assert!(
        out.contains("This is "),
        "body text must survive into JSX:\n{out}",
    );
    // Bold formatting must reach the output as a JSX element.
    assert!(
        out.contains("<strong>") || out.contains("<_components.strong>"),
        "bold must render as a strong element:\n{out}",
    );
    assert!(
        out.contains("bold"),
        "bold word must appear in the JSX body:\n{out}",
    );
    // The previous regression produced literal `Paragraph` /
    // `Strong` debug repr — make sure that does not leak.
    assert!(
        !out.contains("Paragraph {"),
        "mdast Debug repr leaked into JSX body:\n{out}",
    );
    assert!(
        !out.contains("Strong {"),
        "mdast Debug repr leaked into JSX body:\n{out}",
    );
}

/// Inline math survives the hast detour (regression for #121 round
/// one — Math/InlineMath were dropped by the `_ => Raw("")` fallback
/// in `mdast_to_hast`).
#[test]
fn inline_math_survives_hast_detour() {
    let out = emit_with_defaults("When $x \\to \\infty$ converges.\n");
    assert!(
        out.contains("language-math math-inline"),
        "inline math must reach the JSX output:\n{out}",
    );
    // The LaTeX body must be preserved (escaped as a JS string
    // literal — backslashes doubled).
    assert!(
        out.contains("\\\\to") || out.contains("\\\\infty"),
        "LaTeX backslashes must survive as escaped JS strings:\n{out}",
    );
}

/// Block math survives the hast detour.
#[test]
fn block_math_survives_hast_detour() {
    let out = emit_with_defaults("$$\n\\int_0^1 f(x)\\,dx\n$$\n");
    assert!(
        out.contains("language-math math-display"),
        "block math must reach the JSX output:\n{out}",
    );
    assert!(
        out.contains("\\\\int_0^1") || out.contains("\\\\int_"),
        "LaTeX body must survive as escaped JS strings:\n{out}",
    );
}

#[test]
fn directive_survives_with_hast_phase() {
    // mdast phase: the directives step (configured here with `note → Note`)
    // folds `:::note … :::` into a <Note> element. hast phase: the JsxRaw
    // payload survives verbatim and registers `Note` for the preamble.
    // Core seeds no directive names, so the mapping is supplied explicitly.
    use std::collections::HashMap;
    use zfb_md_extras::{DirectiveSpec, MarkdownFeaturesConfig};

    let mut directives = HashMap::new();
    directives.insert("note".to_string(), DirectiveSpec::Short("Note".to_string()));
    let features = MarkdownFeaturesConfig {
        directives: Some(directives),
        ..Default::default()
    };
    let mut p = Pipeline::with_defaults_and_features(&features);
    let out = mdx_to_jsx_module_with_pipeline(
        ":::note\n\nbody text\n\n:::\n",
        MdxJsxOptions::default(),
        &mut p,
    )
    .expect("pipeline emit ok");
    assert!(
        out.contains("<Note") && out.contains("</Note>"),
        "directive transform should yield <Note>:\n{out}",
    );
    assert!(
        out.contains("const Note = _components.Note ?? components.Note;"),
        "Note preamble must be emitted from JsxRaw scan:\n{out}",
    );
}

/// Regression for codex-review #125: lowercase MDX JSX tags must
/// route through `_components.<tag>` on the pipeline path so callers
/// can override `<span>`/`<table>`/etc. via the `components` prop.
/// Before the fix, `jsx_element_text` re-emitted lowercase tags
/// verbatim, silently breaking authors who relied on
/// `components={{span: ...}}` overrides for explicit MDX JSX.
#[test]
fn lowercase_mdx_jsx_tags_route_through_components() {
    let out = emit_with_defaults("<span>foo</span>\n");
    assert!(
        out.contains("<_components.span>") && out.contains("</_components.span>"),
        "lowercase MDX JSX must route through `_components.<tag>`:\n{out}",
    );
    // The preamble must register the tag so the default
    // `_components` map has a fallback string for it (so the route
    // works even without an explicit `components` prop override).
    assert!(
        out.contains("span: \"span\","),
        "default `_components.span = \"span\"` fallback must be emitted:\n{out}",
    );
    // Sanity: the bare `<span>` form must NOT leak through alongside
    // the `_components.span` form.
    assert!(
        !out.contains("<span>") && !out.contains("</span>"),
        "bare `<span>` must not appear alongside the `_components.span` route:\n{out}",
    );
}

/// Regression for codex-review #125: nested lowercase MDX JSX with
/// attributes must also route through `_components.<tag>` and keep
/// its attributes intact.
#[test]
fn lowercase_mdx_jsx_with_attrs_routes_through_components() {
    let out = emit_with_defaults(
        "<table className=\"x\"><tbody><tr><td>cell</td></tr></tbody></table>\n",
    );
    assert!(
        out.contains("<_components.table"),
        "<table> must route through `_components.table`:\n{out}",
    );
    // Attribute is preserved on the `_components.<tag>` form.
    assert!(
        out.contains("className=\"x\""),
        "className must survive on the `_components.table` route:\n{out}",
    );
    // Each lowercase tag in the tree gets registered for the
    // default `_components` map.
    for tag in ["table", "tbody", "tr", "td"] {
        let needle = format!("{tag}: \"{tag}\",");
        assert!(
            out.contains(&needle),
            "`_components` default must register `{needle}`:\n{out}",
        );
    }
}

/// Regression for codex-review #125: an empty MDX fragment shorthand
/// `<></>` must emit a valid JSX `_Fragment` rather than the
/// invalid `< />` the previous emitter produced. SWC must accept
/// the resulting module.
#[test]
fn empty_mdx_fragment_emits_valid_jsx() {
    let out = emit_with_defaults("<>\n\nfoo\n\n</>\n");
    // The fragment shorthand routes to `_Fragment` (already imported
    // at the module top).
    assert!(
        out.contains("<_Fragment>") || out.contains("<_Fragment "),
        "empty fragment shorthand must emit `_Fragment`:\n{out}",
    );
    // The bare invalid `< />` / `< >` forms must NOT appear.
    assert!(
        !out.contains("< />") && !out.contains("< >"),
        "invalid bare fragment markers must not leak through:\n{out}",
    );
    // SWC must accept the result — this is the headline guarantee
    // (the previous emitter produced JSX SWC rejected).
    let opts = CompileOptions::default().with_filename("empty-fragment.tsx".to_string());
    let pipeline_compile = SwcPipeline::new();
    pipeline_compile
        .compile(&out, &opts)
        .unwrap_or_else(|e| panic!("SWC rejected empty-fragment output: {e}\n--- src ---\n{out}"));
}

/// Regression for #286 (epic #285): markdown block nodes nested inside
/// an MDX JSX element body — a heading, a paragraph, AND a list — must
/// route through `_components.<tag>` so consumers can override `<h2>` /
/// `<p>` / `<ul>` / `<li>` via the `components` prop, matching
/// `JsxEmitter::emit_jsx`'s contract. Before the fix `jsx_render_child`
/// / `jsx_wrap_children` emitted bare `<h2>`/`<p>`/`<ul>`/`<li>` tags,
/// which missed the consumer `_components` map (no styled heading) and
/// were never registered in the preamble.
#[test]
fn nested_markdown_in_mdx_jsx_routes_through_components() {
    let out = emit_with_defaults(
        "<Outro>\n\n## Wrap up\n\nA closing paragraph.\n\n- first\n- second\n\n</Outro>\n",
    );
    // The PascalCase wrapper still routes correctly (unchanged behavior).
    assert!(
        out.contains("const Outro = _components.Outro ?? components.Outro;"),
        "PascalCase wrapper preamble must be emitted for <Outro>:\n{out}",
    );
    // Nested markdown tags route through `_components.<tag>`.
    for needle in [
        "<_components.h2",
        "<_components.p>",
        "<_components.ul>",
        "<_components.li>",
    ] {
        assert!(
            out.contains(needle),
            "nested markdown must route through `{needle}`:\n{out}",
        );
    }
    // Bare HTML tags must NOT leak through alongside the routed forms —
    // this negative assertion is what catches a future regression.
    for bare in [
        "<h2>", "<h2 ", "<p>", "<ul>", "<li>", "</h2>", "</p>", "</ul>", "</li>",
    ] {
        assert!(
            !out.contains(bare),
            "bare `{bare}` must not leak alongside the `_components` route:\n{out}",
        );
    }
    // The preamble must register each tag's default fallback so the
    // route works even without an explicit `components` override —
    // this proves `collect_components_tag_names` harvested the tags.
    for tag in ["h2", "p", "ul", "li"] {
        let needle = format!("{tag}: \"{tag}\",");
        assert!(
            out.contains(&needle),
            "`_components` default must register `{needle}`:\n{out}",
        );
    }
    // SWC must accept the emitted module.
    let opts = CompileOptions::default().with_filename("nested-md.tsx".to_string());
    SwcPipeline::new()
        .compile(&out, &opts)
        .unwrap_or_else(|e| panic!("SWC rejected nested-markdown output: {e}\n--- src ---\n{out}"));
}

/// Regression for #477 (upstream zudolab/zzmod#292): a markdown
/// `## heading` nested inside an MDX JSX element body must get a
/// rehype-slug `id`, a hash-link anchor, AND a TOC (`headings`) entry —
/// matching what `HeadingLinksPlugin` gives a top-level heading.
///
/// Before this fix the nested heading routed through `_components.h2`
/// for styling (#286 / PR #439) but carried NO `id`, NO anchor, and was
/// dropped from `export const headings`, because it lives in an opaque
/// JsxRaw payload that hast visitors never traverse.
#[test]
fn nested_heading_in_mdx_jsx_gets_slug_id_anchor_and_toc_entry() {
    let out = emit_with_defaults("<Outro>\n\n## Wrap Up\n\n</Outro>\n");

    // (1) The nested `<_components.h2>` carries `id="wrap-up"` — the
    // exact `slugify("Wrap Up")` output.
    assert!(
        out.contains("<_components.h2 id=\"wrap-up\">"),
        "nested heading must get id from slugify:\n{out}",
    );

    // (2) The TOC export includes the nested heading (depth, slug, text).
    assert!(
        out.contains("{ depth: 2, slug: \"wrap-up\", text: \"Wrap Up\" }"),
        "headings export must include the nested heading:\n{out}",
    );

    // (4) The nested heading carries a hash-link anchor with the SAME
    // shape `HeadingLinksPlugin` stamps on a top-level heading:
    // `<_components.a href="#slug" class="hash-link" aria-label="…">`.
    assert!(
        out.contains(
            "<_components.a href=\"#wrap-up\" class=\"hash-link\" aria-label=\"Direct link to Wrap Up\"></_components.a>"
        ),
        "nested heading must carry a hash-link anchor identical to a top-level one:\n{out}",
    );

    // SWC must accept the emitted module.
    let opts = CompileOptions::default().with_filename("nested-slug.tsx".to_string());
    SwcPipeline::new()
        .compile(&out, &opts)
        .unwrap_or_else(|e| panic!("SWC rejected nested-slug output: {e}\n--- src ---\n{out}"));
}

/// (3) Slug dedup numbering stays consistent when a same-text heading
/// appears BOTH top-level and nested-in-JSX. Both passes draw from the
/// single document-order `seen` map in `collect_headings`, so in
/// document order the top-level `## Foo` wins `foo` and the later
/// nested `## Foo` gets `foo-1` — and the rendered ids match the TOC.
#[test]
fn nested_and_top_level_same_text_share_dedup_numbering() {
    // Top-level `## Foo` first (gets `foo`), then nested `## Foo` inside
    // `<Outro>` second (must get `foo-1`).
    let out = emit_with_defaults("## Foo\n\n<Outro>\n\n## Foo\n\n</Outro>\n");

    // Top-level heading keeps the un-numbered slug (HeadingLinksPlugin).
    assert!(
        out.contains("id=\"foo\""),
        "top-level heading should keep the base slug `foo`:\n{out}",
    );
    // Nested heading gets the deduped `foo-1`, NOT a colliding `foo`.
    assert!(
        out.contains("<_components.h2 id=\"foo-1\">"),
        "nested same-text heading must dedup to `foo-1`:\n{out}",
    );
    assert!(
        out.contains("href=\"#foo-1\""),
        "nested heading anchor must point at the deduped slug:\n{out}",
    );

    // TOC export carries BOTH in document order with consistent slugs.
    let foo_idx = out
        .find("slug: \"foo\"")
        .expect("TOC must list top-level `foo`");
    let foo1_idx = out
        .find("slug: \"foo-1\"")
        .expect("TOC must list nested `foo-1`");
    assert!(
        foo_idx < foo1_idx,
        "TOC must list `foo` before `foo-1` (document order):\n{out}",
    );

    let opts = CompileOptions::default().with_filename("dedup.tsx".to_string());
    SwcPipeline::new()
        .compile(&out, &opts)
        .unwrap_or_else(|e| panic!("SWC rejected dedup output: {e}\n--- src ---\n{out}"));
}

/// Multiple headings — including a deeply-nested one inside a list —
/// in a single MDX JSX body must each pop the right slug from the
/// document-order cursor (no off-by-one, no slug bleed between
/// headings). Also confirms the `in_jsx` flag propagates through a
/// list/list-item nested under the JSX element.
#[test]
fn multiple_and_deeply_nested_headings_in_one_jsx_pop_correct_slugs() {
    let out = emit_with_defaults(
        "<Outro>\n\n## Alpha\n\n### Beta\n\n- item\n\n  #### Gamma\n\n</Outro>\n",
    );

    // Each heading gets its own slug id at the correct depth.
    for (depth, slug) in [(2, "alpha"), (3, "beta"), (4, "gamma")] {
        assert!(
            out.contains(&format!("<_components.h{depth} id=\"{slug}\">")),
            "nested h{depth} must get id=\"{slug}\":\n{out}",
        );
        // …and a TOC entry.
        assert!(
            out.contains(&format!("depth: {depth}, slug: \"{slug}\"")),
            "TOC must include h{depth} `{slug}`:\n{out}",
        );
    }

    // Slugs appear in document order in the TOC (alpha < beta < gamma).
    let a = out.find("slug: \"alpha\"").unwrap();
    let b = out.find("slug: \"beta\"").unwrap();
    let g = out.find("slug: \"gamma\"").unwrap();
    assert!(
        a < b && b < g,
        "TOC headings must be in document order:\n{out}"
    );

    let opts = CompileOptions::default().with_filename("multi-nested.tsx".to_string());
    SwcPipeline::new()
        .compile(&out, &opts)
        .unwrap_or_else(|e| panic!("SWC rejected multi-nested output: {e}\n--- src ---\n{out}"));
}

/// Confirm test for #605 (Wave 2): guards regressions #599 + #600 together on
/// the JSX-emit / compile path.
///
/// When both `ruby` (caret syntax) and `tocExport` are enabled:
/// - (a) the bare `{これ}` expression must NOT appear in the output — the caret
///   ruby annotation `これは^{これ}` must be transformed into ruby markup
///   (#600 / Sub B fix). Pre-fix, JSX emitter left `{これ}` as a raw expression
///   referencing an undefined JS variable, which compiles fine but fails at
///   runtime as a ReferenceError.
/// - (b) `export const toc = [...]` must be at column 0 (module scope), NOT
///   indented inside the `<_Fragment>` body (#599 / Sub A fix). Pre-fix, the
///   6-space indented form caused esbuild/SWC parse errors.
/// - (c) The emitted JSX module must compile via SWC with no parse errors —
///   the compound guard for both regressions on the actual compile path.
#[test]
fn ruby_caret_and_toc_export_together_on_jsx_compile_path() {
    // Wire BOTH features manually: RubyPlugin (mdast phase) + TocExportPlugin
    // (hast phase, after HeadingLinksPlugin has assigned ids).
    let mut p = Pipeline::with_defaults();
    // RubyPlugin runs in the mdast phase — must be added before the pipeline
    // converts mdast → hast. `with_defaults()` ships no mdast visitors so
    // this registration is safe to do right after construction.
    p.add_mdast_visitor(Box::new(zfb_md_extras::ruby::RubyPlugin::new()));
    p.add_hast_visitor(Box::new(zfb_md_extras::toc_export::TocExportPlugin::new(
        TocExportConfig::default(),
    )));

    // The document contains both repro inputs:
    // - `これは^{これ}` — caret ruby syntax from #600 (Sub B repro, bare-base form)
    // - two headings so TocExportPlugin produces a non-empty toc array
    let src =
        "## Introduction\n\nこれは^{これ} is ruby annotation.\n\n## Conclusion\n\nMore text.\n";
    let out = mdx_to_jsx_module_with_pipeline(src, MdxJsxOptions::default(), &mut p)
        .expect("pipeline emit with ruby + toc_export ok");

    // (a) Sub B guard — two-part check for the caret ruby annotation.
    //
    //     Positive: the caret annotation must produce ruby markup. On the
    //     JSX-emit path, lowercase tags route through `_components.<tag>`, so
    //     the expected form is `<_components.ruby>` (not a bare `<ruby>` literal).
    //     The `rb`/`rt` children follow the same pattern.
    assert!(
        out.contains("<_components.ruby>"),
        "caret ruby annotation must emit `<_components.ruby>` markup (#600 regression):\n{out}",
    );
    assert!(
        out.contains("<_components.rb>これは</_components.rb>"),
        "ruby base (これは) must be wrapped in `<_components.rb>` (#600 regression):\n{out}",
    );
    assert!(
        out.contains("<_components.rt>これ</_components.rt>"),
        "ruby text (これ) must be wrapped in `<_components.rt>` (#600 regression):\n{out}",
    );

    //     Negative: the bare `{これ}` expression must NOT appear.
    //     Before Sub B's fix, the caret was not parsed as ruby, so the
    //     `{これ}` part landed as a raw MdxTextExpression in the JSX output,
    //     causing a runtime ReferenceError (`これ` is not defined). SWC
    //     compiles it fine, so compile-only checks do NOT catch this regression.
    assert!(
        !out.contains("{これ}"),
        "bare `{{これ}}` expression must NOT appear — caret ruby must be transformed (#600 regression):\n{out}",
    );

    // (b) Sub A guard: `export const toc` must be at column 0 (module scope).
    assert!(
        out.contains("\nexport const toc = ["),
        "export const toc must appear at column 0 (no leading spaces) (#599 regression):\n{out}",
    );
    assert!(
        !out.contains("      export const toc"),
        "export const toc must NOT be 6-space-indented (Fragment-body regression #599):\n{out}",
    );
    // Module-scope position: before `function _createMdxContent`.
    let toc_pos = out
        .find("export const toc")
        .expect("export const toc must be present in output");
    let fn_pos = out
        .find("function _createMdxContent")
        .expect("function _createMdxContent must be present");
    assert!(
        toc_pos < fn_pos,
        "export const toc must appear before function _createMdxContent (module scope):\n{out}",
    );
    // Toc payload must reference both headings (both qualify at depth 2 with default max_depth=3).
    assert!(
        out.contains("\"id\":\"introduction\"") && out.contains("\"id\":\"conclusion\""),
        "toc array must contain both heading ids:\n{out}",
    );

    // (c) SWC compile guard: the combined output must be valid TSX.
    //     Before Sub A's fix, SWC rejected with `Expected "}" but found ":"`.
    let opts = CompileOptions::default().with_filename("ruby-toc-confirm.tsx".to_string());
    SwcPipeline::new()
        .compile(&out, &opts)
        .unwrap_or_else(|e| {
            panic!(
                "SWC rejected ruby + toc_export combined output (#605 regression guard): {e}\n--- src ---\n{out}"
            )
        });
}

/// Regression for #599 / fix in #602: `TocExportPlugin` injects
/// `export const toc = …` as a `HastNode::JsxRaw` at the front of the
/// hast Root. On the JSX-emit pipeline path, `HastJsxBridge::emit_root`
/// used to prepend 6 spaces of indent before every child, making the
/// export land inside the `<_Fragment>` body — in MDX an indented `export`
/// is parsed as content so esbuild rejected the JSON object literal with
/// `Expected "}" but found ":"`. The fix hoists root-level module-level
/// ESM JsxRaw nodes to column 0 at module scope.
#[test]
fn toc_export_plugin_emits_at_column_0_on_jsx_path() {
    // Build a pipeline with the default plugins PLUS TocExportPlugin.
    // We wire it generically via add_hast_visitor rather than adding a
    // dedicated `add_toc_export` method — generic wiring is all the fix
    // requires, and it avoids touching files outside the scope of #602.
    let mut p = Pipeline::with_defaults();
    p.add_hast_visitor(Box::new(zfb_md_extras::toc_export::TocExportPlugin::new(
        TocExportConfig::default(),
    )));
    let src = "## Introduction\n\nSome body text.\n\n## Conclusion\n\nMore text.\n";
    let out = mdx_to_jsx_module_with_pipeline(src, MdxJsxOptions::default(), &mut p)
        .expect("pipeline emit with toc_export ok");

    // (1) The export must start at column 0 — no leading whitespace.
    // A substring search for the newline-prefixed form catches both the
    // "indented inside Fragment body" regression and any accidental
    // whitespace introduced between the preceding line and the export.
    assert!(
        out.contains("\nexport const toc = ["),
        "export const toc must appear at column 0 (no leading spaces):\n{out}",
    );

    // (2) The buggy 6-space indented form must NOT be present.
    assert!(
        !out.contains("      export const toc"),
        "export const toc must NOT be indented (6 spaces = Fragment body regression):\n{out}",
    );

    // (3) The export must be at module scope, before _createMdxContent.
    let toc_pos = out
        .find("export const toc")
        .expect("export const toc must be present in output");
    let fn_pos = out
        .find("function _createMdxContent")
        .expect("function _createMdxContent must be present");
    assert!(
        toc_pos < fn_pos,
        "export const toc must appear before function _createMdxContent (module scope):\n{out}",
    );

    // (4) The toc payload must contain the two headings (h2 only, default
    // max_depth=3; both "Introduction" and "Conclusion" qualify).
    assert!(
        out.contains("\"id\":\"introduction\"") && out.contains("\"id\":\"conclusion\""),
        "toc array must contain both heading ids:\n{out}",
    );

    // (5) SWC must accept the emitted module — this is the headline
    // regression check. Before the fix, SWC rejected the module with the
    // same class of error esbuild reported in #599:
    // `Expected "}" but found ":"` on the first colon in the JSON literal.
    let opts = CompileOptions::default().with_filename("toc-export.tsx".to_string());
    SwcPipeline::new().compile(&out, &opts).unwrap_or_else(|e| {
        panic!("SWC rejected toc-export output (#599 regression): {e}\n--- src ---\n{out}")
    });
}

/// zfb#871 headline acceptance: `features.headingIds.strategy =
/// "hierarchical"` must produce ancestor-prefixed ids in BOTH render
/// paths — the rendered `<hN id="…">` + hash-link anchors (stamped by
/// `HeadingLinksPlugin` on the hast detour) AND the `headings` export
/// (computed by `collect_headings` via the shared `SlugAllocator`).
#[test]
fn hierarchical_strategy_ids_match_in_both_paths() {
    use zfb_md_extras::{HeadingIdStrategy, HeadingIdsConfig, MarkdownFeaturesConfig};

    let features = MarkdownFeaturesConfig {
        heading_ids: Some(HeadingIdsConfig {
            strategy: HeadingIdStrategy::Hierarchical,
        }),
        ..Default::default()
    };
    let mut p = Pipeline::with_defaults_and_features(&features);
    let out = mdx_to_jsx_module_with_pipeline(
        "## Foo\n\n### Moo\n\n#### Mew\n",
        MdxJsxOptions::default(),
        &mut p,
    )
    .expect("pipeline emit ok");

    // Rendered hast path: ids + matching anchor hrefs.
    for (id, href) in [
        ("foo", "#foo"),
        ("foo-moo", "#foo-moo"),
        ("foo-moo-mew", "#foo-moo-mew"),
    ] {
        assert!(
            out.contains(&format!("id=\"{id}\"")),
            "rendered heading must carry hierarchical id {id:?}:\n{out}",
        );
        assert!(
            out.contains(&format!("href=\"{href}\"")),
            "hash-link must carry hierarchical href {href:?}:\n{out}",
        );
    }

    // headings export path: same slugs, in document order.
    for slug in ["\"foo\"", "\"foo-moo\"", "\"foo-moo-mew\""] {
        assert!(
            out.contains(&format!("slug: {slug}")),
            "headings export must carry hierarchical slug {slug}:\n{out}",
        );
    }
    // The flat slugs must NOT leak through for the nested levels.
    assert!(
        !out.contains("slug: \"moo\"") && !out.contains("id=\"moo\""),
        "flat slug `moo` must not appear in hierarchical mode:\n{out}",
    );
}

/// zfb#871: a JSX-nested heading gets its hierarchical id stamped by
/// `jsx_render_child` (HeadingLinksPlugin never sees it), prefixed by
/// the preceding top-level ancestor.
#[test]
fn hierarchical_strategy_jsx_nested_heading_prefixed() {
    use zfb_md_extras::{HeadingIdStrategy, HeadingIdsConfig, MarkdownFeaturesConfig};

    let features = MarkdownFeaturesConfig {
        heading_ids: Some(HeadingIdsConfig {
            strategy: HeadingIdStrategy::Hierarchical,
        }),
        ..Default::default()
    };
    let mut p = Pipeline::with_defaults_and_features(&features);
    let out = mdx_to_jsx_module_with_pipeline(
        "## Foo\n\n<Note>\n\n### Moo\n\n</Note>\n",
        MdxJsxOptions::default(),
        &mut p,
    )
    .expect("pipeline emit ok");

    assert!(
        out.contains("id=\"foo-moo\""),
        "JSX-nested heading must get hierarchical id `foo-moo`:\n{out}",
    );
    assert!(
        out.contains("slug: \"foo-moo\""),
        "headings export must record the nested hierarchical slug:\n{out}",
    );
}

/// zfb#871 byte-stability guardrail: an absent (or default) `headingIds`
/// keeps the flat scheme — nested headings keep their unprefixed slugs
/// and the cross-level dedup counter still applies.
#[test]
fn default_features_keep_flat_heading_ids() {
    use zfb_md_extras::MarkdownFeaturesConfig;

    let features = MarkdownFeaturesConfig::default();
    let mut p = Pipeline::with_defaults_and_features(&features);
    let out = mdx_to_jsx_module_with_pipeline(
        "## Foo\n\n### Moo\n\n### Foo\n",
        MdxJsxOptions::default(),
        &mut p,
    )
    .expect("pipeline emit ok");

    for id in ["id=\"foo\"", "id=\"moo\"", "id=\"foo-1\""] {
        assert!(
            out.contains(id),
            "flat default must keep unprefixed + deduped ids ({id}):\n{out}",
        );
    }
    assert!(
        !out.contains("id=\"foo-moo\""),
        "hierarchical ids must NOT appear without opting in:\n{out}",
    );
}

/// Codex-review follow-up (zfb#871): a CUSTOM pipeline that hand-wires
/// `HeadingLinksPlugin::with_strategy(Hierarchical)` must call
/// `set_heading_id_strategy` so the `headings` export mirrors the
/// rendered ids — this test pins the setter keeping the two in sync.
#[test]
fn manual_wiring_with_setter_keeps_headings_export_in_sync() {
    use zfb_content::plugins::heading_links::{HeadingIdStrategy, HeadingLinksPlugin};

    let mut p = Pipeline::with_mdx();
    p.set_heading_id_strategy(HeadingIdStrategy::Hierarchical);
    p.add_hast_visitor(Box::new(HeadingLinksPlugin::with_strategy(
        HeadingIdStrategy::Hierarchical,
    )));
    let out =
        mdx_to_jsx_module_with_pipeline("## Foo\n\n### Moo\n", MdxJsxOptions::default(), &mut p)
            .expect("pipeline emit ok");

    assert!(
        out.contains("id=\"foo-moo\""),
        "manually wired plugin must render hierarchical ids:\n{out}",
    );
    assert!(
        out.contains("slug: \"foo-moo\""),
        "headings export must follow the declared strategy:\n{out}",
    );
}
