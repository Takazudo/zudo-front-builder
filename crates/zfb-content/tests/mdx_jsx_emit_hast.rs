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
//! 3. image-enlarge: a `<p>` whose only child is `<img>` becomes a
//!    `<figure class="zd-enlargeable">` with an enlarge button.
//! 4. mermaid: a fenced ```mermaid block becomes a
//!    `<div class="mermaid" data-mermaid>…</div>`.
//! 5. syntect: a non-mermaid fenced code block routes through
//!    syntect (output reaches the JSX module wrapped in
//!    `dangerouslySetInnerHTML`).
//! 6. strip-md-ext (opt-in): internal `[x](./guide.md)` links lose
//!    the `.md` and gain a trailing slash on the JSX path.
//! 7. MDX component passthrough: a `<Note>` reference still triggers
//!    the PascalCase preamble and survives in the JSX body.

use zfb_content::pipeline::Pipeline;
use zfb_content::{mdx_to_jsx_module_with_pipeline, MdxJsxOptions};
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
    let out =
        emit_with_defaults("```rust title=\"main.rs\"\nfn main() {}\n```\n");
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
fn image_enlarge_plugin_fires_on_jsx_path() {
    // A standalone <p><img></p> becomes <figure class="zd-enlargeable">…</figure>.
    let out = emit_with_defaults("![alt](pic.png)\n");
    assert!(
        out.contains("class=\"zd-enlargeable\""),
        "image-enlarge should produce a zd-enlargeable figure:\n{out}",
    );
    assert!(
        out.contains("class=\"zd-enlarge-btn\""),
        "image-enlarge should attach the enlarge button:\n{out}",
    );
    assert!(
        out.contains("aria-label=\"Enlarge image\""),
        "enlarge button must carry aria-label:\n{out}",
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
    let out = mdx_to_jsx_module_with_pipeline(
        "## Hello\n",
        MdxJsxOptions::default(),
        &mut p,
    )
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
    let out = emit_with_defaults(
        "```rust title=\"main.rs\"\nfn main() {}\n```\n",
    );
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

    // Cover heading-links + syntect + code-title + image-enlarge +
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
    let opts =
        CompileOptions::default().with_filename("hast-detour.tsx".to_string());
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
fn admonitions_directive_survives_with_hast_phase() {
    // mdast phase: AdmonitionsPlugin folds `:::note … :::` into a
    // <Note> element. hast phase: the JsxRaw payload survives
    // verbatim and registers `Note` for the preamble.
    let out = emit_with_defaults(":::note\n\nbody text\n\n:::\n");
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
    let out = emit_with_defaults("<table className=\"x\"><tbody><tr><td>cell</td></tr></tbody></table>\n");
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
