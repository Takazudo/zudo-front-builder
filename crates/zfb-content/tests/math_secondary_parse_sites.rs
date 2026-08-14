//! Path-aware math constructs at the two secondary parse sites (zfb#2397).
//!
//! Companion to `gfm_secondary_parse_sites.rs`, which covers the GFM half
//! of the same two sites. Read that file's header for what "secondary
//! parse site" means; this one covers only what the GFM fix deliberately
//! left alone.
//!
//! Both secondary sites hardcoded [`constructs_for_pipeline`] — the HTML
//! serializer's construct set, where `math_flow` / `math_text` are OFF.
//! The top-level parse does not: the JSX-emit path uses
//! `constructs_for_jsx_emit`, which turns them on, because the emitter has
//! dedicated `Math` / `InlineMath` arms and the alternative is LaTeX
//! leaking out as bare `{…}` JSX expression containers that esbuild
//! rejects — which degrades the WHOLE page to
//! `<pre data-zfb-content-fallback>`. So a single `$$…$$` in a transcluded
//! file took the entire page down while the byte-identical markup written
//! inline was fine.
//!
//! The fix is a per-run [`SecondaryParseTarget`], pushed onto every mdast
//! visitor by each of `Pipeline`'s four mdast dispatch loops before it
//! visits. Structure here:
//!
//! 1. **Dispatch** — all four loops hand out their own hardcoded target,
//!    including the two context-FREE ones. Asserted with a recording
//!    visitor rather than end-to-end, because the context-free arms are
//!    exactly where an end-to-end transclude assertion cannot reach
//!    (`TranscludePlugin::visit` is a no-op without a `BuildContext`).
//! 2. **JSX path** — transcluded block and inline math emit the safe
//!    `language-math` shape, with no bare `{…}` leak.
//! 3. **HTML path** — the explicit non-goal: transcluded math stays
//!    literal text there, byte-identical to before. Enabling it would
//!    create the mirror image of the asymmetry this fix removes.
//! 4. **Directive re-parse** — the other secondary site, pinned on both
//!    paths.
//!
//! [`constructs_for_pipeline`]: zfb_content::pipeline::constructs_for_pipeline

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use markdown::mdast::Node as MdastNode;
use zfb_content::pipeline::{Pipeline, ResolvedGfmConstructs, SecondaryParseTarget};
use zfb_md_ast::{
    BuildContext, DirectiveSpec, MarkdownFeaturesConfig, MdastVisitor, TranscludeConfig,
};

// ── harness ────────────────────────────────────────────────────────────

/// A pipeline with both secondary parse sites wired: transclude, plus a
/// `:::note` container directive. Mirrors the harness in
/// `gfm_secondary_parse_sites.rs`.
fn pipeline(gfm: ResolvedGfmConstructs) -> Pipeline {
    let mut directives = HashMap::new();
    directives.insert("note".to_string(), DirectiveSpec::Short("Note".to_string()));
    let features = MarkdownFeaturesConfig {
        transclude: Some(TranscludeConfig::default()),
        directives: Some(directives),
        ..Default::default()
    };
    Pipeline::with_defaults_and_full_config(None, gfm, None, false, false, Some(&features))
        .expect("pipeline builds")
}

fn tmpdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("math_secondary_parse")
        .tempdir()
        .expect("tempdir")
}

/// Write `body` to `<dir>/snippet.md` and return the include directive
/// that pulls it in.
fn snippet(dir: &std::path::Path, body: &str) -> String {
    std::fs::write(dir.join("snippet.md"), body).expect("write snippet");
    r#":::include{file="./snippet.md"}"#.to_string()
}

/// Compile through the JSX-emit path with `source_path` armed, so
/// `TranscludePlugin` can resolve a relative include. Arming the context
/// roots also routes the compile through
/// `Pipeline::apply_mdast_visitors_with_context`.
fn compile_jsx_in(dir: &std::path::Path, gfm: ResolvedGfmConstructs, input: &str) -> String {
    let mut p = pipeline(gfm);
    p.set_build_context_roots(dir.to_path_buf(), dir.join("public"));
    let opts = zfb_content::MdxJsxOptions::default()
        .with_filename("page.mdx".to_string())
        .with_source_path(dir.join("page.mdx"));
    zfb_content::mdx_to_jsx_module_with_pipeline(input, opts, &mut p).expect("compile ok")
}

/// Compile through the JSX-emit path with the context roots UNARMED, the
/// shape a directives-only project produces: `build_context_roots` is set
/// only when transclude / imageDimensions / linkValidation is enabled
/// (`zfb::commands::bundler_input`), so such a project reaches
/// `Pipeline::apply_mdast_visitors` — the context-free arm.
fn compile_jsx_context_free(gfm: ResolvedGfmConstructs, input: &str) -> String {
    let mut p = pipeline(gfm);
    let opts = zfb_content::MdxJsxOptions::default().with_filename("page.mdx".to_string());
    zfb_content::mdx_to_jsx_module_with_pipeline(input, opts, &mut p).expect("compile ok")
}

/// Render `input` as HTML with `source_path` armed, so `TranscludePlugin`
/// actually runs.
fn render_in(dir: &std::path::Path, gfm: ResolvedGfmConstructs, input: &str) -> String {
    let mut p = pipeline(gfm);
    let mut ctx = BuildContext {
        source_path: Some(dir.join("page.md")),
        project_root: dir.to_path_buf(),
        public_dir: dir.join("public"),
        heading_registry: None,
        diagnostics: None,
        cross_file_links: None,
    };
    let hast = p.run_with_context(input, &mut ctx).expect("render ok");
    zfb_content::serializer::serialize(&hast)
}

/// Block math plus inline math in one payload, so a regression in either
/// construct shows up rather than being masked by the other.
const MATH_BODY: &str = "\
Lead in.

$$
\\int_{-\\infty}^{\\infty} f(x)\\,dx
$$

When $x \\to \\infty$ the limit holds.
";

/// The exact leak the whole-page fallback comes from: LaTeX reaching the
/// emitter as text that JSX reads as an expression container. `{\infty}`
/// and friends are not valid JS, so esbuild rejects the module and the
/// bundler's defensive skip degrades the page.
fn assert_no_bare_latex_expression_container(jsx: &str, context: &str) {
    for leak in ["{\\int", "{-\\infty}", "{\\infty}", "{\\to", "{\\,dx"] {
        assert!(
            !jsx.contains(leak),
            "{context}: raw LaTeX leaked into a JSX expression container \
             (`{leak}`) — this is what esbuild rejects, taking the whole \
             page to <pre data-zfb-content-fallback>:\n{jsx}"
        );
    }
}

// ── 1. dispatch ────────────────────────────────────────────────────────

/// Records every [`SecondaryParseTarget`] the pipeline hands it.
struct TargetRecorder(Arc<Mutex<Vec<SecondaryParseTarget>>>);

impl MdastVisitor for TargetRecorder {
    fn visit(&mut self, _node: &mut MdastNode) {}

    fn set_secondary_parse_target(&mut self, target: SecondaryParseTarget) {
        self.0.lock().expect("recorder lock").push(target);
    }
}

/// Every one of `Pipeline`'s four mdast dispatch loops must announce its
/// own hardcoded target — including the two CONTEXT-FREE arms.
///
/// Asserted here rather than end-to-end because the context-free arms are
/// unreachable for transclude by construction: `TranscludePlugin::visit`
/// is a no-op without a `BuildContext`, so no transclude assertion can
/// prove `apply_mdast_visitors` dispatches at all. A ctx-carrying trait
/// method would pass every end-to-end test in this file while leaving
/// those two loops silent, which is precisely why the mechanism is a
/// plain setter.
///
/// The four calls arrive in dispatch order, so this also pins that no
/// loop leaks a target set by a previous run: each loop overwrites.
#[test]
fn all_four_mdast_dispatch_loops_announce_their_own_hardcoded_target() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut p = pipeline(ResolvedGfmConstructs::ALL_ON);
    p.add_mdast_visitor(Box::new(TargetRecorder(Arc::clone(&log))));

    let dir = tmpdir();
    let mut ctx = BuildContext {
        source_path: Some(dir.path().join("page.md")),
        project_root: dir.path().to_path_buf(),
        public_dir: dir.path().join("public"),
        heading_registry: None,
        diagnostics: None,
        cross_file_links: None,
    };

    let mut mdast = MdastNode::Root(markdown::mdast::Root {
        children: Vec::new(),
        position: None,
    });

    p.run("hi").expect("run ok");
    p.run_with_context("hi", &mut ctx).expect("run ctx ok");
    p.apply_mdast_visitors(&mut mdast);
    p.apply_mdast_visitors_with_context(&mut mdast, &mut ctx);

    let seen = log.lock().expect("recorder lock").clone();
    assert_eq!(
        seen,
        vec![
            SecondaryParseTarget::Html,
            SecondaryParseTarget::Html,
            SecondaryParseTarget::Jsx,
            SecondaryParseTarget::Jsx,
        ],
        "run / run_with_context must announce Html; apply_mdast_visitors / \
         apply_mdast_visitors_with_context must announce Jsx"
    );
}

// ── 2. JSX path ────────────────────────────────────────────────────────

/// The headline fix. A transcluded `$$…$$` / `$…$` must reach the emitter
/// as `Math` / `InlineMath` nodes, exactly as the byte-identical markup
/// written inline already did (`mdx_jsx_emit`'s
/// `block_math_emits_safe_jsx_with_display_class` and its inline twin pin
/// the top-level behaviour this brings transcluded content into line with).
///
/// The `class=` spelling (rather than the `className=` those top-level
/// unit tests assert) is not a discrepancy: those drive `JsxEmitter`
/// directly, while a pipeline-driven compile detours through
/// `mdast_to_hast`, whose Math arms build the `<pre><code class="…">`
/// element the HTML serializer also uses.
#[test]
fn jsx_emit_transcluded_math_emits_the_safe_math_shape() {
    let dir = tmpdir();
    let include = snippet(dir.path(), MATH_BODY);

    let jsx = compile_jsx_in(dir.path(), ResolvedGfmConstructs::ALL_ON, &include);

    assert!(
        jsx.contains("language-math math-display"),
        "transcluded block math did not parse: {jsx}"
    );
    assert!(
        jsx.contains("language-math math-inline"),
        "transcluded inline math did not parse: {jsx}"
    );
    assert!(
        jsx.contains("<_components.pre><_components.code"),
        "block math must route through _components.pre/code: {jsx}"
    );
    // The LaTeX body survives as a JS string literal, backslashes doubled.
    assert!(
        jsx.contains("\\\\int_"),
        "expected backslash-escaped LaTeX inside a JS string literal: {jsx}"
    );
    assert_no_bare_latex_expression_container(&jsx, "JSX emit, transcluded");
}

/// Math is orthogonal to the GFM flag set: `markdown.gfm: false` must not
/// switch it back off on the JSX path, because it never came from the GFM
/// resolve in the first place — `constructs_for_jsx_emit` layers it over
/// whatever `constructs_for_pipeline` produced.
///
/// Without this, threading the target could plausibly be implemented by
/// reusing a GFM flag and the whole-page fallback would come straight back
/// for every project that opted out of GFM.
#[test]
fn jsx_emit_transcluded_math_is_independent_of_the_gfm_flag_set() {
    let dir = tmpdir();
    let include = snippet(dir.path(), MATH_BODY);

    let jsx = compile_jsx_in(dir.path(), ResolvedGfmConstructs::ALL_OFF, &include);

    assert!(
        jsx.contains("language-math math-display") && jsx.contains("language-math math-inline"),
        "math must parse on the JSX path regardless of markdown.gfm: {jsx}"
    );
    assert_no_bare_latex_expression_container(&jsx, "JSX emit, transcluded, gfm off");
}

// ── 3. HTML path (the explicit non-goal) ───────────────────────────────

/// Pins the deliberate asymmetry-avoidance: math stays OFF at every
/// `Html`-target parse, so transcluded content keeps rendering exactly
/// what it rendered before this change.
///
/// Turning it on here would render `Math` / `InlineMath` into real
/// `<pre><code class="language-math …">` elements (see `mdast_to_hast`),
/// changing serializer output for every existing project — and it would
/// drag in markdown-rs's `math_text_single_dollar` behaviour, where a
/// literal `$` in prose becomes math. That is the mirror image of the bug
/// class PR 2391 fixed, not a fix.
#[test]
fn html_transcluded_math_stays_literal_text() {
    let dir = tmpdir();
    let include = snippet(dir.path(), MATH_BODY);

    let html = render_in(dir.path(), ResolvedGfmConstructs::ALL_ON, &include);

    assert!(
        !html.contains("language-math"),
        "the HTML path must not grow math elements: {html}"
    );
    assert!(
        html.contains("$$") && html.contains("$x \\to \\infty$"),
        "both math spellings must survive as literal text: {html}"
    );
    assert!(
        html.contains("Lead in."),
        "the include must still be spliced in: {html}"
    );
}

// ── 4. directive re-parse ──────────────────────────────────────────────

/// No-regression pin for the directive site on the context-FREE JSX arm —
/// NOT a test of the `reparse_block` threading, and deliberately named so.
///
/// Reverting either half of the fix leaves this green (measured), because
/// the body below never reaches `reparse_block` at all. `reparse_block` is
/// reachable only through `single_text_collapsed`, which requires the
/// paragraph to have exactly ONE multi-line `Text` child — and on the JSX
/// path the main parse already has math on, so it tokenises `$x \to
/// \infty$` into an `InlineMath` node, leaving the paragraph with several
/// inline children and routing it to `transform_block_container` instead.
/// That is general, not a quirk of this input: any math rich enough to
/// render differently is consumed by the main parse first, so threading
/// the target through `reparse_block` removes a latent inconsistency
/// between two parse sites rather than changing rendered output. The same
/// argument the #2390 changelog made for GFM at this site.
///
/// What this test is for: proving a directives-only project — the shape
/// that compiles through the context-free `apply_mdast_visitors` arm —
/// still emits structured, esbuild-safe math after the change.
///
/// Asserted at the JSX-string level, per `gfm_secondary_parse_sites.rs`'s
/// header note: this site's output lands inside an `MdxJsxFlowElement`,
/// which the HTML path stringifies lossily, so an HTML-level assertion
/// there would measure `reconstruct_jsx` rather than the parse site.
#[test]
fn jsx_emit_collapsed_directive_body_with_math_emits_the_safe_math_shape() {
    let jsx = compile_jsx_context_free(
        ResolvedGfmConstructs::ALL_ON,
        ":::note\nplain lead-in\nWhen $x \\to \\infty$ the limit holds.\n:::\n",
    );

    assert!(
        jsx.contains("<Note>"),
        "the directive must transform: {jsx}"
    );
    assert!(
        jsx.contains("language-math math-inline"),
        "math in a collapsed directive body must reach the emitted JSX as \
         math, not as text: {jsx}"
    );
    assert_no_bare_latex_expression_container(&jsx, "JSX emit, collapsed directive");
}

/// The HTML half of the same body: math off, literal text — the same
/// non-goal `html_transcluded_math_stays_literal_text` pins for the other
/// site.
#[test]
fn html_collapsed_directive_body_with_math_stays_literal_text() {
    let dir = tmpdir();

    let html = render_in(
        dir.path(),
        ResolvedGfmConstructs::ALL_ON,
        ":::note\nplain lead-in\nWhen $x \\to \\infty$ the limit holds.\n:::\n",
    );

    assert!(
        !html.contains("language-math"),
        "the HTML path must not grow math elements: {html}"
    );
    assert!(
        html.contains("$x \\to \\infty$"),
        "the math spelling must survive as literal text: {html}"
    );
}
