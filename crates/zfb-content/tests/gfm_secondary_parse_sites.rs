//! GFM parity at the two secondary markdown parse sites (zfb#2390).
//!
//! `Pipeline::run` is not the only place zfb calls `markdown::to_mdast`.
//! Two plugins re-enter the parser from *inside* the mdast visitor chain:
//!
//! - `zfb_md_extras::transclude::TranscludePlugin` parses each
//!   `:::include{file="…"}` target,
//! - `DirectiveRegistry::reparse_block` re-parses a collapsed
//!   (blank-line-less) directive body, and — via `flush_prose` — ordinary
//!   page prose sitting between two collapsed directive runs.
//!
//! Both hardcoded a bare `markdown::ParseOptions::mdx()`. `Constructs::mdx()`
//! inherits `Constructs::default()`, where every `gfm_*` flag is `false`, so
//! content reaching either path got **no GFM constructs at all** regardless
//! of `markdown.gfm`: a pipe table written inline rendered as a table, the
//! byte-identical table in an included file rendered as literal text.
//!
//! Before #2390 no test in the workspace put GFM syntax through either
//! path, so this file and its Level-1 companion are the entire regression
//! net for the fix. Each assertion was confirmed to fail against the
//! pre-fix code rather than merely documenting current behaviour.
//!
//! **Scope: transclude only.** Transcluded nodes splice in as top-level
//! siblings, so the rendered HTML shows them in full and an end-to-end
//! assertion is meaningful here. The `reparse_block` site is covered at
//! Level 1 instead, in `plugins::directives`'s own `mod tests` — its
//! output lands inside an `MdxJsxFlowElement`, which this HTML path hands
//! to `reconstruct_jsx`, whose documented lossy fallback stringifies
//! nested markdown (`[label](url)` → the text `label`). An HTML assertion
//! there would be measuring `reconstruct_jsx`, not the construct
//! threading. Asserting on the mdast the re-parse produces is both exact
//! and immune to that.
//!
//! Structure:
//!
//! 1. **Parity** — the same markdown renders the same whether written
//!    inline or reached through the secondary site, under `ALL_ON`.
//! 2. **Gating** — under `ALL_OFF` it stays literal text, pinning that a
//!    project which never opted into GFM is byte-identical to before.
//! 3. **Nested anchors** — turning on `autolink_literal` at a new parse
//!    site must not reintroduce #2388's `<a>`-inside-`<a>`. The pipeline
//!    normalises its *own* parse before the visitor chain starts, so a
//!    subtree parsed later is reachable only if the site normalises it
//!    itself.
//! 4. **CJK autolink boundary** — same argument for #1105, whose
//!    `CjkAutolinkBoundaryPlugin` is a visitor at chain index 0 and
//!    therefore never sees a subtree spliced in later.

use std::collections::HashMap;

use zfb_content::pipeline::{Pipeline, ResolvedGfmConstructs};
use zfb_md_ast::{BuildContext, DirectiveSpec, MarkdownFeaturesConfig, TranscludeConfig};

// ── harness ────────────────────────────────────────────────────────────

/// A pipeline with both secondary parse sites wired: transclude, plus
/// `:::note` and `:::tip` container directives.
///
/// `cjk_friendly` is a parameter because the CJK autolink boundary tests
/// need it on while the parity tests deliberately keep it off (so a
/// boundary rewrite can never be mistaken for the construct threading
/// under test). `hard_breaks` was added by zfb#2398 for the same reason.
///
/// `tip` exists so a NESTED collapsed run has a second registered name to
/// nest (zfb#2413); no test predating it writes `:::tip`.
fn pipeline_full(gfm: ResolvedGfmConstructs, cjk_friendly: bool, hard_breaks: bool) -> Pipeline {
    let mut directives = HashMap::new();
    directives.insert("note".to_string(), DirectiveSpec::Short("Note".to_string()));
    directives.insert("tip".to_string(), DirectiveSpec::Short("Tip".to_string()));
    let features = MarkdownFeaturesConfig {
        transclude: Some(TranscludeConfig::default()),
        directives: Some(directives),
        ..Default::default()
    };
    Pipeline::with_defaults_and_full_config(
        None,
        gfm,
        None,
        cjk_friendly,
        hard_breaks,
        Some(&features),
    )
    .expect("pipeline builds")
}

/// Render `input` as HTML with `source_path` armed, so `TranscludePlugin`
/// (which needs `BuildContext::source_path` to resolve a relative include)
/// actually runs.
fn render_in(dir: &std::path::Path, gfm: ResolvedGfmConstructs, input: &str) -> String {
    render_in_cjk(dir, gfm, false, input)
}

fn render_in_cjk(
    dir: &std::path::Path,
    gfm: ResolvedGfmConstructs,
    cjk_friendly: bool,
    input: &str,
) -> String {
    render_in_full(dir, gfm, cjk_friendly, false, input)
}

/// Like [`render_in_cjk`] but also threads `hardBreaks` (zfb#2398).
fn render_in_full(
    dir: &std::path::Path,
    gfm: ResolvedGfmConstructs,
    cjk_friendly: bool,
    hard_breaks: bool,
    input: &str,
) -> String {
    let mut p = pipeline_full(gfm, cjk_friendly, hard_breaks);
    let source_path = dir.join("page.md");
    let mut ctx = BuildContext {
        source_path: Some(source_path),
        project_root: dir.to_path_buf(),
        public_dir: dir.join("public"),
        heading_registry: None,
        diagnostics: None,
        cross_file_links: None,
    };
    let hast = p.run_with_context(input, &mut ctx).expect("render ok");
    zfb_content::serializer::serialize(&hast)
}

fn tmpdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("gfm_secondary_parse")
        .tempdir()
        .expect("tempdir")
}

/// Write `body` to `<dir>/snippet.md` and return the include directive that
/// pulls it in.
fn snippet(dir: &std::path::Path, body: &str) -> String {
    snippet_named(dir, "snippet.md", body)
}

/// Like [`snippet`] but under a caller-chosen filename, so a test can
/// write more than one distinct include target in the same `dir` (e.g. a
/// two-level nested `:::include` chain — zfb#2398).
fn snippet_named(dir: &std::path::Path, name: &str, body: &str) -> String {
    std::fs::write(dir.join(name), body).expect("write snippet");
    format!(r#":::include{{file="./{name}"}}"#)
}

/// True when one anchor opens before the previous one closed.
///
/// Byte-scanned (not `&str`-sliced) because the markup between the ASCII
/// tag boundaries is not ASCII in the CJK cases, and advancing a byte index
/// into a `&str` panics the moment the cursor lands mid-codepoint. Matches
/// exact tag boundaries so `<abbr>` is never miscounted as an anchor. Twin
/// of the helper in `gfm_autolink_nested_link.rs`.
fn has_nested_anchor(markup: &str) -> bool {
    let bytes = markup.as_bytes();
    let mut depth = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let rest = &bytes[i..];
        if rest.starts_with(b"</a>") {
            depth = depth.saturating_sub(1);
            i += 4;
            continue;
        }
        if rest.starts_with(b"<a") && matches!(rest.get(2), Some(b' ') | Some(b'>')) {
            depth += 1;
            if depth > 1 {
                return true;
            }
            i += 2;
            continue;
        }
        i += 1;
    }
    false
}

/// The GFM payload used by every parity test: one construct per flag in
/// `ResolvedGfmConstructs`, so a regression in any single flag's threading
/// shows up rather than being masked by the others.
const GFM_BODY: &str = "\
| a | b |
| - | - |
| 1 | 2 |

~~struck~~

- [x] done
- [ ] todo

See https://example.com/x for more.

Footnote here.[^n]

[^n]: the note body.
";

/// Every construct in `GFM_BODY`, as the rendered-HTML marker that proves
/// it parsed rather than surviving as literal text.
const GFM_MARKERS: &[(&str, &str)] = &[
    ("table", "<table>"),
    ("strikethrough", "<del>"),
    ("task_list_item", "type=\"checkbox\""),
    ("autolink_literal", "href=\"https://example.com/x\""),
    ("footnote_definition", "data-footnote-backref"),
];

fn assert_all_gfm_rendered(markup: &str, context: &str) {
    for (construct, marker) in GFM_MARKERS {
        assert!(
            markup.contains(marker),
            "{context}: GFM `{construct}` did not parse — expected `{marker}` in:\n{markup}"
        );
    }
}

fn assert_no_gfm_rendered(markup: &str, context: &str) {
    for (construct, marker) in GFM_MARKERS {
        assert!(
            !markup.contains(marker),
            "{context}: GFM `{construct}` parsed but every construct is OFF — \
             unexpected `{marker}` in:\n{markup}"
        );
    }
}

// ── transclude ─────────────────────────────────────────────────────────

#[test]
fn transcluded_file_parses_with_the_projects_gfm_config() {
    let dir = tmpdir();
    let include = snippet(dir.path(), GFM_BODY);

    let inline = render_in(dir.path(), ResolvedGfmConstructs::ALL_ON, GFM_BODY);
    let transcluded = render_in(dir.path(), ResolvedGfmConstructs::ALL_ON, &include);

    assert_all_gfm_rendered(&inline, "inline control");
    assert_all_gfm_rendered(&transcluded, "transcluded");
}

#[test]
fn transcluded_file_keeps_every_construct_off_when_gfm_is_off() {
    let dir = tmpdir();
    let include = snippet(dir.path(), GFM_BODY);

    let transcluded = render_in(dir.path(), ResolvedGfmConstructs::ALL_OFF, &include);

    assert_no_gfm_rendered(&transcluded, "transcluded, ALL_OFF");
    // Positive control: the file WAS included — otherwise "no GFM" would
    // pass trivially on an empty splice.
    assert!(
        transcluded.contains("struck"),
        "the include must still be spliced in: {transcluded}"
    );
}

/// zfb#2388 at the transclude parse site. `Pipeline::normalize_nested_links`
/// runs before the visitor chain, so it cannot reach a subtree transclude
/// parses later — the plugin has to unwrap its own output.
#[test]
fn transcluded_autolink_in_a_link_label_yields_no_nested_anchor() {
    let dir = tmpdir();
    let body = "[http://localhost:4321](http://localhost:4321)\n";
    let include = snippet(dir.path(), body);

    let transcluded = render_in(dir.path(), ResolvedGfmConstructs::ALL_ON, &include);

    assert!(
        transcluded.contains("<a href=\"http://localhost:4321\">"),
        "the author's link must survive: {transcluded}"
    );
    assert!(
        !has_nested_anchor(&transcluded),
        "nested <a> inside <a> in transcluded content: {transcluded}"
    );
}

/// zfb#1105 at the transclude parse site. `CjkAutolinkBoundaryPlugin` is a
/// visitor at mdast-chain index 0, so a transcluded subtree spliced in later
/// is never reached by it.
#[test]
fn transcluded_bare_url_does_not_swallow_trailing_cjk() {
    let dir = tmpdir();
    let body = "詳しくは https://example.com/xを参照。\n";
    let include = snippet(dir.path(), body);

    let transcluded = render_in_cjk(dir.path(), ResolvedGfmConstructs::ALL_ON, true, &include);

    assert!(
        transcluded.contains("href=\"https://example.com/x\""),
        "the bare URL must autolink at exactly the ASCII boundary: {transcluded}"
    );
    // The CJK run is flush against the URL — no whitespace to stop the
    // autolink — so without the boundary fix `を参照` is swallowed into the
    // href. A correctly-cut link puts `</a>` between the two, so this
    // substring can only appear if the swallow happened.
    assert!(
        !transcluded.contains("https://example.com/xを"),
        "trailing CJK was swallowed into the href: {transcluded}"
    );
}

// ── JSX-emit path ──────────────────────────────────────────────────────
//
// The HTML assertions above are not sufficient on their own. The JSX-emit
// path has its OWN `markdown::to_mdast` call (in `mdx_jsx_emit.rs`, not
// shared with `Pipeline::run`) and its own construct set, so it can
// regress independently — the same reason `gfm_autolink_nested_link.rs`
// covers both entry points for the sibling defect. It is also the path
// that matters in production for directives, whose bodies stay structured
// here instead of being flattened by `reconstruct_jsx`.

/// Compile through the JSX-emit path with `source_path` armed, so
/// `TranscludePlugin` can resolve a relative include.
fn compile_jsx_in(
    dir: &std::path::Path,
    gfm: ResolvedGfmConstructs,
    cjk_friendly: bool,
    input: &str,
) -> String {
    compile_jsx_in_full(dir, gfm, cjk_friendly, false, input)
}

/// Like [`compile_jsx_in`] but also threads `hardBreaks` (zfb#2398).
fn compile_jsx_in_full(
    dir: &std::path::Path,
    gfm: ResolvedGfmConstructs,
    cjk_friendly: bool,
    hard_breaks: bool,
    input: &str,
) -> String {
    let mut p = pipeline_full(gfm, cjk_friendly, hard_breaks);
    p.set_build_context_roots(dir.to_path_buf(), dir.join("public"));
    let opts = zfb_content::MdxJsxOptions::default()
        .with_filename("page.mdx".to_string())
        .with_source_path(dir.join("page.mdx"));
    zfb_content::mdx_to_jsx_module_with_pipeline(input, opts, &mut p).expect("compile ok")
}

#[test]
fn jsx_emit_transcluded_file_parses_with_the_projects_gfm_config() {
    let dir = tmpdir();
    let include = snippet(dir.path(), GFM_BODY);

    let jsx = compile_jsx_in(dir.path(), ResolvedGfmConstructs::ALL_ON, false, &include);

    // JSX spellings differ from the HTML ones (`_components.table` etc.),
    // so these markers are deliberately not the `GFM_MARKERS` set.
    for (construct, marker) in [
        ("table", "_components.table"),
        ("strikethrough", "_components.del"),
        ("task_list_item", "type=\"checkbox\""),
        ("autolink_literal", "href=\"https://example.com/x\""),
        ("footnote_definition", "data-footnote-backref"),
    ] {
        assert!(
            jsx.contains(marker),
            "JSX emit, transcluded: GFM `{construct}` did not parse — \
             expected `{marker}` in:\n{jsx}"
        );
    }
}

#[test]
fn jsx_emit_transcluded_file_keeps_every_construct_off_when_gfm_is_off() {
    let dir = tmpdir();
    let include = snippet(dir.path(), GFM_BODY);

    let jsx = compile_jsx_in(dir.path(), ResolvedGfmConstructs::ALL_OFF, false, &include);

    for (construct, marker) in [
        ("table", "_components.table"),
        ("strikethrough", "_components.del"),
        ("task_list_item", "type=\"checkbox\""),
        ("autolink_literal", "href=\"https://example.com/x\""),
        ("footnote_definition", "data-footnote-backref"),
    ] {
        assert!(
            !jsx.contains(marker),
            "JSX emit, transcluded, ALL_OFF: GFM `{construct}` parsed — \
             unexpected `{marker}` in:\n{jsx}"
        );
    }
    assert!(
        jsx.contains("struck"),
        "the include must still be spliced in: {jsx}"
    );
}

/// zfb#2388 on the JSX-emit path. `mdx_jsx_emit.rs` has its own
/// `unwrap_nested_links` call after its own parse, but — exactly as on the
/// HTML path — that runs before the visitor chain and so cannot reach what
/// transclude parses later.
#[test]
fn jsx_emit_transcluded_autolink_in_a_link_label_yields_no_nested_anchor() {
    let dir = tmpdir();
    let include = snippet(
        dir.path(),
        "[http://localhost:4321](http://localhost:4321)\n",
    );

    let jsx = compile_jsx_in(dir.path(), ResolvedGfmConstructs::ALL_ON, false, &include);

    assert!(
        jsx.contains("http://localhost:4321"),
        "the author's link must survive: {jsx}"
    );
    let normalized = jsx
        .replace("<_components.a", "<a")
        .replace("</_components.a>", "</a>");
    assert!(
        !has_nested_anchor(&normalized),
        "nested <a> inside <a> in transcluded content on the JSX path: {jsx}"
    );
}

/// zfb#1105 on the JSX-emit path.
#[test]
fn jsx_emit_transcluded_bare_url_does_not_swallow_trailing_cjk() {
    let dir = tmpdir();
    let include = snippet(dir.path(), "詳しくは https://example.com/xを参照。\n");

    let jsx = compile_jsx_in(dir.path(), ResolvedGfmConstructs::ALL_ON, true, &include);

    assert!(
        jsx.contains("href=\"https://example.com/x\""),
        "the bare URL must autolink at exactly the ASCII boundary: {jsx}"
    );
    assert!(
        !jsx.contains("https://example.com/xを"),
        "trailing CJK was swallowed into the href: {jsx}"
    );
}

/// No-regression pin for the directive site on the JSX path — NOT a test
/// of the `reparse_block` threading, and deliberately named so.
///
/// Reverting either half of the fix leaves this green, because the body
/// below never reaches `reparse_block` at all: with the constructs
/// threaded, the MAIN parse already tokenises `~~struck~~` into a
/// `Delete` node, so the paragraph has multiple inline children and
/// `single_text_collapsed` declines, routing to
/// `transform_block_container` instead.
///
/// That is the general case, not a quirk of this input. An independent
/// review of this change diffed real JSX-emit output across 12 collapsed
/// directive shapes × 4 GFM configurations and found it byte-identical:
/// `reparse_block` is reachable only through `single_text_collapsed`, and
/// now that the main parse shares the same constructs, content rich
/// enough to render differently is generally consumed by that main parse
/// first. So the threading there removes a latent inconsistency between
/// two parse sites rather than changing rendered output — which is why
/// the changelog scopes the user-visible behaviour change to transclude,
/// and why `reparse_block`'s own contract is pinned at Level 1 in
/// `plugins::directives`'s `mod tests` instead of here.
///
/// What this test is for: proving the directive path still emits
/// structured JSX children after the change, i.e. that threading the
/// constructs did not break the collapsed-directive machinery.
#[test]
fn jsx_emit_collapsed_directive_body_still_emits_structured_children() {
    let dir = tmpdir();
    let input = ":::note\nplain lead-in\n~~struck~~\n:::\n";

    let jsx = compile_jsx_in(dir.path(), ResolvedGfmConstructs::ALL_ON, false, input);

    assert!(
        jsx.contains("<Note>"),
        "the directive must transform: {jsx}"
    );
    assert!(
        jsx.contains("_components.del"),
        "the body's strikethrough must reach the emitted JSX structured, \
         not flattened to text: {jsx}"
    );
}

// ── CJK-friendly emphasis + hard breaks at both secondary parse sites (zfb#2398) ──
//
// `CjkFriendlyPlugin` and `HardBreaksPlugin` are visitors in the pipeline's
// own mdast chain, so — exactly like the GFM constructs (#2390) and math
// constructs (#2397) before them — they never see a subtree parsed later
// by `TranscludePlugin` or `DirectiveRegistry::reparse_block`. This file
// covers the transclude site end-to-end (HTML + JSX-emit, including a
// two-level nested `:::include` chain). `reparse_block`'s two call sites
// (the directive body and `flush_prose`) are covered at Level 1 instead, in
// `plugins::directives`'s own `mod tests` (see its "CJK-friendly emphasis +
// hard breaks at this parse site" section) — an HTML-level end-to-end test
// for either call site here would not be revert-sensitive to this
// sub-issue's change; see the explanatory comments below, where those tests
// would otherwise have gone.

/// A CJK-flanked strong marker `CjkFriendlyPlugin` corrects — see
/// `zfb_md_ast::cjk_friendly`'s module docs for why plain CommonMark leaves
/// this as literal `*` text (the run is never tokenised into `Strong` at
/// all without the fix, so "off" and "on" are trivially distinguishable).
/// Shared with that module's other test sites rather than re-spelled here
/// (zfb#2402).
const CJK_EMPHASIS_BODY: &str = zfb_md_ast::cjk_friendly::FLANKED_EMPHASIS_REPRO;

#[test]
fn transcluded_cjk_emphasis_flanking_is_corrected() {
    let dir = tmpdir();
    let include = snippet(dir.path(), CJK_EMPHASIS_BODY);

    let inline = render_in_cjk(
        dir.path(),
        ResolvedGfmConstructs::ALL_ON,
        true,
        CJK_EMPHASIS_BODY,
    );
    let transcluded = render_in_cjk(dir.path(), ResolvedGfmConstructs::ALL_ON, true, &include);

    assert!(
        inline.contains("<strong>重要。</strong>"),
        "inline control did not correct CJK emphasis flanking: {inline}"
    );
    assert!(
        transcluded.contains("<strong>重要。</strong>"),
        "CJK emphasis flanking was not corrected inside transcluded content: {transcluded}"
    );
}

#[test]
fn transcluded_cjk_emphasis_stays_literal_when_cjk_friendly_is_off() {
    let dir = tmpdir();
    let include = snippet(dir.path(), CJK_EMPHASIS_BODY);

    let transcluded = render_in_cjk(dir.path(), ResolvedGfmConstructs::ALL_ON, false, &include);

    assert!(
        !transcluded.contains("<strong>"),
        "CJK emphasis must stay literal text when cjkFriendly is off: {transcluded}"
    );
    // Positive control: the file WAS included — otherwise "no <strong>"
    // would pass trivially on an empty splice.
    assert!(
        transcluded.contains("テスト"),
        "the include must still be spliced in: {transcluded}"
    );
}

/// The decoupling assertion PR 2391's own test suite could not make:
/// `CjkFriendlyPlugin` is threaded raw, un-ANDed with `gfm.autolink_literal`
/// (unlike `CjkAutolinkBoundaryPlugin`, which stays gated on it), so it
/// must still correct CJK emphasis flanking in transcluded content with
/// autolinking off.
#[test]
fn transcluded_cjk_emphasis_corrects_even_when_autolink_literal_is_off() {
    let dir = tmpdir();
    let include = snippet(dir.path(), CJK_EMPHASIS_BODY);
    let gfm = ResolvedGfmConstructs {
        autolink_literal: false,
        ..ResolvedGfmConstructs::ALL_ON
    };

    let transcluded = render_in_cjk(dir.path(), gfm, true, &include);

    assert!(
        transcluded.contains("<strong>重要。</strong>"),
        "CJK emphasis must correct in transcluded content even with \
         autolink_literal off: {transcluded}"
    );
}

const HARD_BREAK_BODY: &str = "first line\nsecond line";

#[test]
fn transcluded_hard_break_becomes_br_on_the_html_path() {
    let dir = tmpdir();
    let include = snippet(dir.path(), HARD_BREAK_BODY);

    let with_hard_breaks = render_in_full(
        dir.path(),
        ResolvedGfmConstructs::ALL_ON,
        false,
        true,
        &include,
    );
    let without_hard_breaks = render_in_full(
        dir.path(),
        ResolvedGfmConstructs::ALL_ON,
        false,
        false,
        &include,
    );

    assert!(
        with_hard_breaks.contains("<br"),
        "soft break in transcluded content must become <br> when hardBreaks \
         is on: {with_hard_breaks}"
    );
    assert!(
        !without_hard_breaks.contains("<br"),
        "hardBreaks off must leave transcluded content unchanged: {without_hard_breaks}"
    );
    // Positive control: the file WAS included.
    assert!(
        without_hard_breaks.contains("second"),
        "the include must still be spliced in: {without_hard_breaks}"
    );
}

#[test]
fn jsx_emit_transcluded_hard_break_becomes_br() {
    let dir = tmpdir();
    let include = snippet(dir.path(), HARD_BREAK_BODY);

    let with_hard_breaks = compile_jsx_in_full(
        dir.path(),
        ResolvedGfmConstructs::ALL_ON,
        false,
        true,
        &include,
    );
    let without_hard_breaks = compile_jsx_in_full(
        dir.path(),
        ResolvedGfmConstructs::ALL_ON,
        false,
        false,
        &include,
    );

    assert!(
        with_hard_breaks.contains("_components.br"),
        "soft break in transcluded content must become <br> on the JSX \
         path: {with_hard_breaks}"
    );
    assert!(
        !without_hard_breaks.contains("_components.br"),
        "hardBreaks off must leave transcluded content unchanged on the \
         JSX path: {without_hard_breaks}"
    );
}

/// An `:::include` written inside a literal, author-written
/// `<Note>…</Note>`. `expand_includes_in_node` descends into
/// `MdxJsxFlowElement`, so the included subtree does NOT splice in as a
/// top-level sibling here — it becomes a JSX body child.
fn jsx_wrapped_include(include: &str) -> String {
    format!("<Note>\n\n{include}\n\n</Note>\n")
}

/// zfb#2402: the include site's placement decides whether
/// `HardBreaksPlugin` may run, and this shape is the one where it may
/// not. The original motivation was that an `MdxJsxFlowElement` on the
/// HTML path went through `reconstruct_jsx`'s lossy catch-all, which
/// stringified a `Break` to the EMPTY string — so normalising hard
/// breaks into a JSX-nested include DELETED the author's newline
/// instead of rendering `<br>`. zfb#2401 has since fixed that renderer
/// (a `Break` in a JSX body now renders `<br />`), but the placement
/// rule this test pins is unchanged and independent of it: no `Break`
/// is ever injected here, so the author's literal newline is what must
/// survive.
///
/// Unlike the collapsed-directive-body site (whose end-to-end reach is
/// masked by the zfb#2401 chain-ordering interaction documented below),
/// nothing masks this one: `TranscludePlugin` is registered AFTER the
/// pipeline's own top-level `HardBreaksPlugin`, so the spliced subtree is
/// never touched by the chain's copy, and the include paragraph itself is
/// single-line.
#[test]
fn transcluded_hard_break_inside_a_literal_jsx_element_keeps_the_newline_on_the_html_path() {
    let dir = tmpdir();
    let include = snippet(dir.path(), HARD_BREAK_BODY);

    let rendered = render_in_full(
        dir.path(),
        ResolvedGfmConstructs::ALL_ON,
        false,
        true,
        &jsx_wrapped_include(&include),
    );

    // Positive control: the include really was expanded inside the JSX
    // element, so the assertions below are not passing on an empty body.
    assert!(
        rendered.contains("first line") && rendered.contains("second line"),
        "the include must still be spliced into the JSX body: {rendered}"
    );
    assert!(
        rendered.contains("first line\nsecond line"),
        "the author's newline must survive inside a JSX body — the placement \
         gate injects no Break here, so there is nothing for zfb#2401's \
         `<br />` rendering to act on (zfb#2402): {rendered}"
    );
}

/// The Jsx-target counterpart of the test above, so the fix cannot be
/// "stop applying `HardBreaksPlugin` to JSX-nested includes at all": the
/// JSX emitter has a real arm for `Break`, so the same fixture must still
/// produce one there.
#[test]
fn transcluded_hard_break_inside_a_literal_jsx_element_still_breaks_on_the_jsx_path() {
    let dir = tmpdir();
    let include = snippet(dir.path(), HARD_BREAK_BODY);

    let jsx = compile_jsx_in_full(
        dir.path(),
        ResolvedGfmConstructs::ALL_ON,
        false,
        true,
        &jsx_wrapped_include(&include),
    );

    assert!(
        jsx.contains("_components.br"),
        "a JSX-nested include must still get its <br> on the JSX path: {jsx}"
    );
}

/// zfb#2398: because `normalize_included_subtree` runs before
/// `expand_includes_in_node` recurses into a freshly-included subtree
/// (`transclude.rs`), a two-level `:::include` chain gets CJK/hard-breaks
/// normalisation applied at BOTH levels automatically — proved here rather
/// than assumed.
#[test]
fn nested_transclude_applies_cjk_and_hard_breaks_at_both_levels() {
    let dir = tmpdir();
    let inner_body = format!("{CJK_EMPHASIS_BODY}\n{HARD_BREAK_BODY}");
    let inner_include = snippet_named(dir.path(), "inner.md", &inner_body);
    let outer_include = snippet_named(dir.path(), "outer.md", &inner_include);

    let rendered = render_in_full(
        dir.path(),
        ResolvedGfmConstructs::ALL_ON,
        true,
        true,
        &outer_include,
    );

    assert!(
        rendered.contains("<strong>重要。</strong>"),
        "CJK emphasis must be corrected two include levels deep: {rendered}"
    );
    assert!(
        rendered.contains("<br"),
        "a soft break must become <br> two include levels deep: {rendered}"
    );
}

// ── zfb#2401: a `Break` nested in a JSX body is no longer swallowed ─────
//
// `reconstruct_jsx`'s catch-all (`other => other.to_string()`) treats
// `Break` as one of markdown-rs's "voids" and renders it as `""`, so a
// hard break inside an `MdxJsxFlowElement` body was DELETED — fusing the
// words on either side of it with not even a space between them. The fix
// gates the fallback arm on `subtree_contains_break` in addition to
// `subtree_contains_footnote`, routing such a subtree through the one
// shared `jsx_body_stringify` mirror, which renders `Break` as `<br />`.

const COLLAPSED_DIRECTIVE_HARD_BREAK_SRC: &str = ":::note\nfirst line\nsecond line\n:::\n";

fn text_node(value: &str) -> markdown::mdast::Node {
    markdown::mdast::Node::Text(markdown::mdast::Text {
        value: value.to_string(),
        position: None,
    })
}

/// The real repro from zfb#2401, end-to-end through the FULL pipeline —
/// deliberately not a hand-built tree.
///
/// The chain ordering documented in the trailing comment block below is
/// exactly what makes this shape reach `reconstruct_jsx`: the top-level
/// `HardBreaksPlugin` splits the collapsed directive's single `Text`
/// child into `Text`/`Break`/`Text` before `DirectiveRegistry` runs,
/// destroying `single_text_collapsed`'s precondition, so recognition
/// falls through to `transform_block_container` — which wraps the body
/// in a `Paragraph` (`paragraph_from_lines`) and hands the whole
/// `MdxJsxFlowElement` to the HTML path's lossy stringifier with the
/// `Break` nested one level down. That nesting is why a direct
/// `MdastNode::Break(_)` arm on `reconstruct_jsx` would never fire here.
#[test]
fn collapsed_directive_body_hard_break_renders_br_on_the_html_path() {
    let dir = tmpdir();

    let rendered = render_in_full(
        dir.path(),
        ResolvedGfmConstructs::ALL_ON,
        false,
        true,
        COLLAPSED_DIRECTIVE_HARD_BREAK_SRC,
    );

    assert!(
        rendered.contains("<br"),
        "the collapsed directive body's hard break must render as <br>: {rendered}"
    );
    assert!(
        !rendered.contains("first linesecond line"),
        "the two lines must never be fused into one word run (zfb#2401): {rendered}"
    );
    assert_eq!(
        rendered, "<Note>first line<br />second line</Note>",
        "unexpected rendering of the zfb#2401 repro"
    );
}

/// `hardBreaks: false` for the same input, pinned to its LITERAL
/// serialized output rather than a vague "unchanged": with no
/// `HardBreaksPlugin` in the chain the body stays a single `Text` child
/// carrying the newline, `subtree_contains_break` is `false`, and the
/// subtree takes the byte-identical `other.to_string()` catch-all.
#[test]
fn collapsed_directive_body_without_hard_breaks_keeps_the_literal_newline() {
    let dir = tmpdir();

    let rendered = render_in_full(
        dir.path(),
        ResolvedGfmConstructs::ALL_ON,
        false,
        false,
        COLLAPSED_DIRECTIVE_HARD_BREAK_SRC,
    );

    assert_eq!(rendered, "<Note>first line\nsecond line</Note>");
}

/// Focused counterpart to the end-to-end test above: the exact nested
/// shape — an `MdxJsxFlowElement` whose only child is a
/// `Paragraph(Text, Break, Text)` — driven straight through
/// `mdast_to_hast` (the `JsxEmitStrategy::HtmlPath` entry point). Pins
/// the structure a direct-child `Break` arm would miss, independently of
/// which plugin chain happens to produce it.
#[test]
fn jsx_body_break_nested_in_a_paragraph_reconstructs_as_br() {
    use markdown::mdast;

    let jsx = mdast::Node::MdxJsxFlowElement(mdast::MdxJsxFlowElement {
        name: Some("Note".to_string()),
        attributes: vec![],
        children: vec![mdast::Node::Paragraph(mdast::Paragraph {
            children: vec![
                text_node("first line"),
                mdast::Node::Break(mdast::Break { position: None }),
                text_node("second line"),
            ],
            position: None,
        })],
        position: None,
    });

    let rendered = zfb_content::serializer::serialize(&zfb_content::pipeline::mdast_to_hast(&jsx));

    assert_eq!(rendered, "<Note>first line<br />second line</Note>");
}

/// Byte-identity control: a JSX body whose subtree contains NEITHER a
/// footnote NOR a `Break` still takes the untouched `other.to_string()`
/// catch-all, which drops `Strong`'s formatting while retaining every
/// character. Guards the widened gate from becoming "recurse generally",
/// which the issue puts explicitly out of scope.
#[test]
fn jsx_body_without_a_break_or_footnote_still_takes_the_lossy_catch_all() {
    use markdown::mdast;

    let jsx = mdast::Node::MdxJsxFlowElement(mdast::MdxJsxFlowElement {
        name: Some("Note".to_string()),
        attributes: vec![],
        children: vec![mdast::Node::Paragraph(mdast::Paragraph {
            children: vec![
                text_node("plain "),
                mdast::Node::Strong(mdast::Strong {
                    children: vec![text_node("bold")],
                    position: None,
                }),
            ],
            position: None,
        })],
        position: None,
    });

    let rendered = zfb_content::serializer::serialize(&zfb_content::pipeline::mdast_to_hast(&jsx));

    assert_eq!(rendered, "<Note>plain bold</Note>");
}

/// Both features present in ONE fallback subtree — the interaction the
/// single shared stringifier exists to protect. A second, parallel
/// `Break`-only mirror would render the break but lose the footnote
/// marker (or vice versa) depending on which gate won.
#[test]
fn collapsed_directive_body_renders_a_footnote_and_a_hard_break_together() {
    let dir = tmpdir();

    let rendered = render_in_full(
        dir.path(),
        ResolvedGfmConstructs::ALL_ON,
        false,
        true,
        ":::note\nfirst line[^n]\nsecond line\n:::\n\n[^n]: the note body.\n",
    );

    assert!(
        rendered.contains("data-footnote-ref"),
        "the footnote reference marker must survive beside a Break: {rendered}"
    );
    assert!(
        rendered.contains("<br"),
        "the hard break must survive beside a footnote reference: {rendered}"
    );
    assert!(
        !rendered.contains("first linesecond line"),
        "the two lines must not be fused: {rendered}"
    );
    // The marker and the break render in source order, inside the body.
    let body_end = rendered.find("</Note>").expect("Note body renders");
    let marker = rendered
        .find("data-footnote-ref")
        .expect("marker inside the body");
    let br = rendered.find("<br").expect("break inside the body");
    assert!(
        marker < br && br < body_end,
        "marker then break, both inside the JSX body: {rendered}"
    );
    // And the definition still renders exactly once, in the section.
    assert_eq!(
        rendered.matches("the note body.").count(),
        1,
        "the definition body must render once, in the footnote section: {rendered}"
    );
}

// ── zfb#2408: the `cjkFriendly` sibling of the `Break` case, MEASURED ──
//
// zfb#2401 speculated that `markdown.cjkFriendly` interacts with a
// collapsed directive body the same way `hardBreaks` did. It reaches
// `reconstruct_jsx`'s fallback by the same route — `CjkFriendlyPlugin` is
// wired into the top-level mdast chain before `DirectiveRegistry`, so it
// retokenises the body's single multi-line `Text` child into
// `Text`/`Strong`/`Text`, and `single_text_collapsed` (`directives.rs`)
// bails on `children.len() != 1` exactly as it did for the `Break` nodes
// `HardBreaksPlugin` injected. But the OUTCOME is a different defect
// class, which is why zfb#2408 changed no rendering code.
//
// `Break` is one of `to_string()`'s voids: it rendered as `""`, DELETING
// the author's newline and fusing `first linesecond line`. `Strong` is a
// container, and `to_string()` returns a container's children's text — so
// every character of the body survives and only the `<strong>` wrapper is
// dropped. That is the catch-all's deliberate, documented lossiness (see
// `reconstruct_jsx`'s doc comment in `pipeline.rs`), not content deletion,
// so it stays on the catch-all and is documented instead — in both locales
// of `docs/src/content/docs*/markdown-features/cjk-friendly.mdx`.
//
// These tests exist so the next reader does not have to re-measure.

/// Wrap `body` in a collapsed (blank-line-less) `:::note` container.
fn collapsed_note(body: &str) -> String {
    format!(":::note\n{body}\n:::\n")
}

/// A collapsed body carrying the shared CJK-flanked repro plus a second
/// line, so the embedded `\n` `single_text_collapsed` also requires is
/// present and the precondition it loses is genuinely the child count.
fn collapsed_cjk_emphasis_src() -> String {
    collapsed_note(&format!("{CJK_EMPHASIS_BODY}\n二行目"))
}

/// The zfb#2408 measurement, pinned to its LITERAL output.
///
/// The `<strong>` is gone, but `これは`, `重要。` and `テスト` are all still
/// there — no character of the body text is lost, and the second line's
/// newline survives verbatim (`hardBreaks` is off, so no `Break` is
/// injected). The prose control proves `cjkFriendly` really did tokenise
/// the marker, so the flattening is attributable to the HTML path's
/// stringifier rather than to the emphasis never being recognised at all.
#[test]
fn collapsed_directive_body_cjk_emphasis_loses_formatting_but_not_content() {
    let dir = tmpdir();
    let render =
        |src: &str| render_in_full(dir.path(), ResolvedGfmConstructs::ALL_ON, true, false, src);

    assert_eq!(
        render(&collapsed_cjk_emphasis_src()),
        "<Note>これは重要。テスト\n二行目</Note>"
    );
    // A single-line body loses the emphasis identically — the flattening
    // is the stringifier's, not a consequence of the multi-line shape.
    assert_eq!(
        render(&collapsed_note(CJK_EMPHASIS_BODY)),
        "<Note>これは重要。テスト</Note>"
    );
    // Control: the same markers as ordinary prose keep their <strong>.
    assert_eq!(
        render(CJK_EMPHASIS_BODY),
        "<p>これは<strong>重要。</strong>テスト</p>"
    );
}

/// The formatting loss above belongs to `reconstruct_jsx`'s catch-all, not
/// to `cjkFriendly` and not to the collapsed shape. Plain ASCII emphasis
/// with `cjkFriendly` OFF flattens identically — markdown-rs tokenises
/// `**bold**` into a `Strong` during the initial parse, so
/// `single_text_collapsed`'s one-`Text`-child precondition is already gone
/// before any visitor runs — and so does a blank-line-separated body,
/// which that function never inspects. `cjkFriendly` only decides whether
/// the CJK-flanked `**` becomes a `Strong` at all; with it off the marker
/// stays literal text instead of being flattened away.
#[test]
fn directive_body_emphasis_loss_is_not_specific_to_cjk_friendly_or_to_collapsed_bodies() {
    let dir = tmpdir();
    let render =
        |src: &str| render_in_full(dir.path(), ResolvedGfmConstructs::ALL_ON, false, false, src);

    assert_eq!(
        render(&collapsed_note("plain **bold** text\nsecond line")),
        "<Note>plain bold text\nsecond line</Note>"
    );
    assert_eq!(
        render(":::note\n\nplain **bold** text\n\n:::\n"),
        "<Note>plain bold text</Note>"
    );
    // Control: the same markers as ordinary prose keep their <strong>.
    assert_eq!(
        render("plain **bold** text"),
        "<p>plain <strong>bold</strong> text</p>"
    );
    // And with cjkFriendly off the CJK-flanked marker is never tokenised,
    // so `**` survives as literal text rather than being flattened away.
    assert_eq!(
        render(&collapsed_cjk_emphasis_src()),
        "<Note>これは**重要。**テスト\n二行目</Note>"
    );
}

/// The scope boundary the docs note leans on: `zfb build` compiles content
/// through the MDX/JSX emit path (`zfb_render`'s loader calls
/// `mdx_to_jsx_module_with_pipeline`), which renders a JSX body
/// recursively, so the flattening above is confined to the HTML render
/// path — `zfb_content::facade::render_html`, exposed to JS as
/// `zfb-md-wasm`'s `renderHtml`. Same source, emphasis intact.
#[test]
fn collapsed_directive_body_cjk_emphasis_survives_on_the_jsx_path() {
    let dir = tmpdir();

    let jsx = compile_jsx_in_full(
        dir.path(),
        ResolvedGfmConstructs::ALL_ON,
        true,
        false,
        &collapsed_cjk_emphasis_src(),
    );

    assert!(
        jsx.contains("<_components.strong>重要。</_components.strong>"),
        "the JSX path must keep the emphasis the HTML path flattens: {jsx}"
    );
}

// ── zfb#2413: collapsed-run recognition is tolerant of the SPLIT shape ──
//
// The two sections above measured the pre-emption's consequences for a
// SIMPLE collapsed body. Its real cost was elsewhere: with the split shape
// routed to `transform_block_container`, which at the time had no colon
// stack and no >3-colon opener rule, a NESTED collapsed directive leaked a
// literal
// `:::tip` plus a stray `<p>:::</p>` into the rendered output — on BOTH
// emit paths, `zfb build` included — and a `:::::note` outer fence went
// fully literal. The collapsed form is the only form in which directive
// nesting works at all, so `markdown.hardBreaks: true` (or `cjkFriendly`
// splitting a CJK-flanked `**…**` in the body) silently deleted a
// documented feature.
//
// zfb#2413 makes the line-level re-segmenter read the paragraph through
// the `InlineLine` view instead of a raw `Text` value, so BOTH shapes take
// the same path and the fixtures below render identically with hardBreaks
// on and off. The chain order, `transclude.rs`, `reparse_block`'s
// normalisation, and the `SecondaryParsePlacement` gate are all unchanged
// — see the trailing block at the end of this file.

const NESTED_COLLAPSED_SRC: &str = ":::note\nprose above\n:::tip\ninner body\n:::\n:::\n";

/// The outer fence written with FIVE colons, the CommonMark-Directives
/// spelling for "this container wraps another". The block-level opener
/// parse required exactly three colons at the time, so under the split
/// shape this whole paragraph used to render as one literal `<p>`.
const DEEP_NESTED_COLLAPSED_SRC: &str = ":::::note\nprose above\n:::tip\ninner body\n:::\n:::::\n";

/// A nested collapsed run renders identically whether or not
/// `markdown.hardBreaks` split the paragraph first — pinned to the literal
/// output on the HTML path. Before zfb#2413 the `hardBreaks: true` half was
/// `<Note>prose above<br />:::tip<br />inner body</Note><p>:::</p>`.
#[test]
fn nested_collapsed_directive_renders_the_same_with_and_without_hard_breaks() {
    let dir = tmpdir();
    let render = |hard_breaks: bool| {
        render_in_full(
            dir.path(),
            ResolvedGfmConstructs::ALL_ON,
            false,
            hard_breaks,
            NESTED_COLLAPSED_SRC,
        )
    };

    assert_eq!(
        render(false),
        "<Note>prose above<Tip>inner body</Tip></Note>"
    );
    assert_eq!(
        render(true),
        "<Note>prose above<Tip>inner body</Tip></Note>"
    );
}

/// The same fixture on the JSX path — what `zfb build` actually emits. The
/// HTML assertion above cannot stand in for this one: the two paths run
/// different emitters, and before zfb#2413 the compiled module carried the
/// literal `:::tip` text instead of a `Tip` element.
#[test]
fn nested_collapsed_directive_compiles_to_a_tip_element_with_hard_breaks_on() {
    let dir = tmpdir();
    let compile = |hard_breaks: bool| {
        compile_jsx_in_full(
            dir.path(),
            ResolvedGfmConstructs::ALL_ON,
            false,
            hard_breaks,
            NESTED_COLLAPSED_SRC,
        )
    };

    for hard_breaks in [false, true] {
        let jsx = compile(hard_breaks);
        assert!(
            jsx.contains("Tip"),
            "the nested directive must compile to a Tip element with \
             hardBreaks={hard_breaks}: {jsx}"
        );
        assert!(
            !jsx.contains(":::tip"),
            "no literal fence may survive into the compiled module with \
             hardBreaks={hard_breaks}: {jsx}"
        );
    }
}

/// A >3-colon outer fence needs the colon-STACK rule, which
/// `transform_block_container` did not have at the time: it matched the
/// innermost closer first and mis-nested, or (for a 5-colon opener)
/// declined outright. Pinned on both emit paths.
#[test]
fn deep_outer_fence_nests_innermost_first_under_hard_breaks() {
    let dir = tmpdir();

    for hard_breaks in [false, true] {
        let rendered = render_in_full(
            dir.path(),
            ResolvedGfmConstructs::ALL_ON,
            false,
            hard_breaks,
            DEEP_NESTED_COLLAPSED_SRC,
        );
        assert_eq!(
            rendered, "<Note>prose above<Tip>inner body</Tip></Note>",
            "5-colon outer fence with hardBreaks={hard_breaks}"
        );

        let jsx = compile_jsx_in_full(
            dir.path(),
            ResolvedGfmConstructs::ALL_ON,
            false,
            hard_breaks,
            DEEP_NESTED_COLLAPSED_SRC,
        );
        assert!(
            jsx.contains("Tip") && !jsx.contains(":::tip"),
            "5-colon outer fence on the JSX path with \
             hardBreaks={hard_breaks}: {jsx}"
        );
    }
}

/// The `cjkFriendly` half of the same defect: no hard break anywhere, but
/// the CJK-flanked `**…**` in the body retokenises into `Text`/`Strong`/
/// `Text`, which loses `single_text_collapsed`'s precondition just as a
/// `Break` does. The nested run must still be recognised, every character
/// must survive, and the `<strong>` must still flatten on the HTML path —
/// that flattening is `reconstruct_jsx`'s documented lossiness (zfb#2408),
/// deliberately NOT this issue's scope, so it is pinned here rather than
/// quietly fixed.
#[test]
fn nested_collapsed_directive_is_recognised_when_cjk_friendly_splits_the_body() {
    let dir = tmpdir();
    let src = ":::note\nこれは**重要。**テスト\n:::tip\ninner body\n:::\n:::\n";

    let rendered = render_in_full(dir.path(), ResolvedGfmConstructs::ALL_ON, true, false, src);
    assert_eq!(
        rendered,
        "<Note>これは重要。テスト<Tip>inner body</Tip></Note>"
    );

    let jsx = compile_jsx_in_full(dir.path(), ResolvedGfmConstructs::ALL_ON, true, false, src);
    assert!(
        jsx.contains("<_components.strong>重要。</_components.strong>"),
        "the JSX path must keep the emphasis the HTML path flattens: {jsx}"
    );
    assert!(
        jsx.contains("Tip") && !jsx.contains(":::tip"),
        "the nested directive must compile to a Tip element: {jsx}"
    );
}

/// Transcluded content is split by `normalize_included_subtree` at splice
/// time (`zfb_md_extras::transclude`), so it reaches `DirectiveRegistry`
/// pre-split no matter where the top-level visitors sit — which is why the
/// fix had to live in recognition rather than in the chain order
/// (zfb#2412, axis 2). Same fixture, same output, through the include path.
#[test]
fn transcluded_nested_collapsed_directive_renders_the_same_with_hard_breaks_on() {
    let dir = tmpdir();
    let include = snippet(dir.path(), NESTED_COLLAPSED_SRC);

    for hard_breaks in [false, true] {
        let rendered = render_in_full(
            dir.path(),
            ResolvedGfmConstructs::ALL_ON,
            false,
            hard_breaks,
            &include,
        );
        assert_eq!(
            rendered, "<Note>prose above<Tip>inner body</Tip></Note>",
            "transcluded nested run with hardBreaks={hard_breaks}"
        );
    }
}

// ── zfb#2416: the blank-line-separated ("spaced") nesting form ──
//
// Everything above measures the COLLAPSED form. Its blank-line-separated
// twin never transformed its OUTER fence: `transform_block_container`'s
// sibling scan stopped at the first opener-shaped sibling — for a spaced
// nested run, that IS the inner directive — so the outer opener was
// recorded unclosed and leaked as `<p>:::note</p>` … `<p>:::</p>` around a
// lone `<Tip>`. The block-level scan now feeds a flattened fence-line view
// to the same colon stack the collapsed path uses, so both forms nest.
//
// `hardBreaks` parity is an acceptance criterion here, not a nicety: #2415
// measured the defect as identical with the setting on and off, so every
// fixture below is asserted on both.

/// The reporter's fixture (zfb#2415) — spaced outer, glued inner run.
const SPACED_NESTED_SRC: &str = ":::note\n\nprose above\n\n:::tip\ninner body\n:::\n\n:::\n";

/// The same nesting with a `:::::` outer fence, the CommonMark-Directives
/// spelling for "this container wraps another". At block level the opener
/// colon count used to be hardcoded to 3, so this went fully literal —
/// without even an unclosed diagnostic.
const SPACED_DEEP_NESTED_SRC: &str =
    ":::::note\n\nprose above\n\n:::tip\ninner body\n:::\n\n:::::\n";

/// The spaced form of the nested fixture renders identically to its
/// collapsed twin (`NESTED_COLLAPSED_SRC`, asserted above) and identically
/// with `markdown.hardBreaks` on and off. Before zfb#2416 both halves were
/// `<p>:::note</p><p>prose above</p><Tip>inner body</Tip><p>:::</p>`.
#[test]
fn spaced_nested_directive_renders_the_same_with_and_without_hard_breaks() {
    let dir = tmpdir();

    for hard_breaks in [false, true] {
        assert_eq!(
            render_in_full(
                dir.path(),
                ResolvedGfmConstructs::ALL_ON,
                false,
                hard_breaks,
                SPACED_NESTED_SRC,
            ),
            "<Note>prose above<Tip>inner body</Tip></Note>",
            "spaced nested run with hardBreaks={hard_breaks}"
        );
    }
}

/// The COLLAPSED twin of the fixture above, re-asserted here as the
/// control: the block-level change must leave the line-level path's output
/// byte-identical. (`nested_collapsed_directive_renders_the_same_with_and_without_hard_breaks`
/// pins the same string from the other side of the change.)
#[test]
fn collapsed_nested_directive_output_is_unchanged_beside_the_spaced_form() {
    let dir = tmpdir();

    for hard_breaks in [false, true] {
        assert_eq!(
            render_in_full(
                dir.path(),
                ResolvedGfmConstructs::ALL_ON,
                false,
                hard_breaks,
                NESTED_COLLAPSED_SRC,
            ),
            "<Note>prose above<Tip>inner body</Tip></Note>",
            "collapsed nested run with hardBreaks={hard_breaks}"
        );
    }
}

/// A `:::::` spaced outer fence, on both emit paths — the HTML assertion
/// cannot stand in for the JSX one (different emitters), and `zfb build`
/// takes the JSX path.
#[test]
fn spaced_deep_outer_fence_nests_innermost_first_on_both_emit_paths() {
    let dir = tmpdir();

    for hard_breaks in [false, true] {
        assert_eq!(
            render_in_full(
                dir.path(),
                ResolvedGfmConstructs::ALL_ON,
                false,
                hard_breaks,
                SPACED_DEEP_NESTED_SRC,
            ),
            "<Note>prose above<Tip>inner body</Tip></Note>",
            "5-colon spaced outer fence with hardBreaks={hard_breaks}"
        );

        let jsx = compile_jsx_in_full(
            dir.path(),
            ResolvedGfmConstructs::ALL_ON,
            false,
            hard_breaks,
            SPACED_DEEP_NESTED_SRC,
        );
        assert!(
            jsx.contains("Tip") && !jsx.contains(":::"),
            "no literal fence may survive into the compiled module with \
             hardBreaks={hard_breaks}: {jsx}"
        );
    }
}

/// The spaced fixture through the INCLUDE path, beside its collapsed
/// counterpart (`transcluded_nested_collapsed_directive_renders_the_same_with_hard_breaks_on`).
/// Transcluded content is split by `normalize_included_subtree` at splice
/// time, so it reaches `DirectiveRegistry` pre-split — the shape most
/// likely to diverge if the fix depended on a pristine single-`Text`
/// paragraph.
#[test]
fn transcluded_nested_spaced_directive_renders_the_same_with_hard_breaks_on() {
    let dir = tmpdir();
    let include = snippet(dir.path(), SPACED_NESTED_SRC);

    for hard_breaks in [false, true] {
        assert_eq!(
            render_in_full(
                dir.path(),
                ResolvedGfmConstructs::ALL_ON,
                false,
                hard_breaks,
                &include,
            ),
            "<Note>prose above<Tip>inner body</Tip></Note>",
            "transcluded spaced nested run with hardBreaks={hard_breaks}"
        );
    }
}

/// The anti-cascade control at the pipeline level: a genuinely unclosed
/// outer opener followed by a well-formed padded directive. The later
/// directive must transform on its own and the unclosed opener must stay
/// literal — the cross-sibling stack never claims a closer for an
/// unbalanced range. (`real_parser_unclosed_opener_leaves_later_padded_directive_intact`
/// pins the same shape at the registry level, diagnostic included.)
#[test]
fn spaced_unclosed_opener_leaves_the_later_padded_directive_intact() {
    let dir = tmpdir();
    let src = ":::warning\nnever closed\n\n:::note\n\npadded body\n\n:::\n";

    assert_eq!(
        render_in_full(dir.path(), ResolvedGfmConstructs::ALL_ON, false, false, src),
        "<p>:::warning\nnever closed</p><Note>padded body</Note>"
    );
    assert_eq!(
        render_in_full(dir.path(), ResolvedGfmConstructs::ALL_ON, false, true, src),
        "<p>:::warning<br/>never closed</p><Note>padded body</Note>"
    );
}

/// The simple transcluded collapsed body, pinned to its literal output on
/// both `hardBreaks` settings — the control proving the tolerance change
/// did not disturb the shape that already worked through the include path.
#[test]
fn transcluded_simple_collapsed_directive_keeps_its_literal_output() {
    let dir = tmpdir();
    let include = snippet(dir.path(), COLLAPSED_DIRECTIVE_HARD_BREAK_SRC);

    assert_eq!(
        render_in_full(
            dir.path(),
            ResolvedGfmConstructs::ALL_ON,
            false,
            false,
            &include
        ),
        "<Note>first line\nsecond line</Note>"
    );
    assert_eq!(
        render_in_full(
            dir.path(),
            ResolvedGfmConstructs::ALL_ON,
            false,
            true,
            &include
        ),
        "<Note>first line<br />second line</Note>"
    );
}

/// Sibling collapsed runs in one paragraph already worked under the split
/// shape (`transform_block_container` handled them), and must keep working:
/// the entry point in `transform_children` now takes them over.
#[test]
fn sibling_collapsed_runs_still_transform_under_hard_breaks() {
    let dir = tmpdir();
    let src = ":::note\nfirst.\n:::\n:::tip\nsecond.\n:::\n";

    for hard_breaks in [false, true] {
        let rendered = render_in_full(
            dir.path(),
            ResolvedGfmConstructs::ALL_ON,
            false,
            hard_breaks,
            src,
        );
        assert_eq!(
            rendered, "<Note>first.</Note><Tip>second.</Tip>",
            "sibling collapsed runs with hardBreaks={hard_breaks}"
        );
    }
}

/// An unknown name and a genuinely unclosed fence keep their literal
/// fallback under the split shape — the tolerance change must not turn a
/// malformed run into a transform.
#[test]
fn unknown_and_unclosed_collapsed_fences_stay_literal_under_hard_breaks() {
    let dir = tmpdir();

    let unknown = render_in_full(
        dir.path(),
        ResolvedGfmConstructs::ALL_ON,
        false,
        true,
        ":::nosuchname\nbody line\n:::\n",
    );
    assert_eq!(
        unknown, "<p>:::nosuchname<br/>body line<br/>:::</p>",
        "an unknown name stays literal"
    );

    let unclosed = render_in_full(
        dir.path(),
        ResolvedGfmConstructs::ALL_ON,
        false,
        true,
        ":::note\nbody line\n",
    );
    assert_eq!(
        unclosed, "<p>:::note<br/>body line</p>",
        "an unclosed fence stays literal"
    );
}

// The directive-body `reparse_block` call site's Jsx-only gate is NOT
// covered by an end-to-end HTML-render test here, deliberately: a
// collapsed (blank-line-less) directive is, by construction, ALWAYS a
// multi-line paragraph with an embedded `\n` in its single `Text` child —
// and when `markdown.hardBreaks` is on, the pipeline's OWN top-level
// `HardBreaksPlugin` (wired earlier in the mdast chain than
// `DirectiveRegistry`, unrelated to and pre-dating zfb#2398) ALREADY
// splits that `Text` child into `Text`/`Break`/`Text` before
// `DirectiveRegistry` ever runs — destroying `single_text_collapsed`'s
// one-Text-child precondition. Since zfb#2413 that no longer routes the
// directive through `transform_block_container`: `recognise_collapsed_run`
// recovers the run from the split shape. But a body split this way carries
// `Break` provenance, and the assembly policy assembles any preserved-node
// range with `paragraph_from_lines` rather than re-parsing it
// (`plain_lines_text` returns `None`, so the `reparse_block` arm is never
// taken). `reparse_block` is consequently STILL UNREACHABLE for this shape
// via the full `Pipeline`, and any HTML-level
// assertion here would (a) not exercise the code this sub-issue touches
// and (b) not be revert-sensitive to it. The Jsx-only gate is instead
// proven, correctly and revert-sensitively, by driving `DirectiveRegistry`
// directly (bypassing the top-level chain) — see
// `plugins::directives`'s `collapsed_directive_body_inserts_break_on_the_jsx_target`
// / `..._does_not_insert_break_on_the_html_target`, the same isolation
// technique this file's own module docs describe for the GFM-parity
// tests at this site.

// `flush_prose` (reached, like the directive-body site above, only from
// inside a recognized collapsed run — via `single_text_collapsed` for the
// pristine shape, via `recognise_collapsed_run` since zfb#2413 for the
// split one) is likewise NOT covered by an end-to-end
// HTML-render test here — same reason, same pre-existing pre-emption. A
// realistic "inter-run prose between two collapsed directive runs" fixture
// is, by construction, itself one multi-line paragraph with embedded `\n`s
// in a single `Text` child, so it is destroyed by the pipeline's own
// top-level `HardBreaksPlugin` / `CjkFriendlyPlugin` (both wired earlier in
// the mdast chain than `DirectiveRegistry`, both pre-dating zfb#2398)
// before `flush_prose`'s OWN call ever runs — confirmed empirically:
// disabling `flush_prose`'s new plugin applications entirely does not turn
// an HTML-level `<br>` / `<strong>` assertion for this fixture red, because
// the pre-emptive top-level pass already produces the same observable
// output. An HTML-level assertion here would not be revert-sensitive to
// this sub-issue's change. The revert-sensitive proof — driving
// `DirectiveRegistry` directly, bypassing the top-level chain — is
// `plugins::directives`'s `inter_run_prose_inserts_break_on_both_targets`.
//
// This is a genuine, pre-existing interaction between `markdown.hardBreaks`
// / `markdown.cjkFriendly` (post-parse VISITORS in the top-level mdast
// chain) and the collapsed-directive machinery, structurally different
// from GFM/math (PARSER-level constructs resolved once during the initial
// parse, before any visitor runs) — the reason #2390/#2397 never hit this.
// It is unrelated to and out of scope for #2398 (which only threads the two
// plugins through `reparse_block`'s own re-parse). The SPLIT ITSELF is still
// present — the chain order is deliberately unchanged (see below) — but as
// of zfb#2413 it no longer degrades recognition. `reparse_block` /
// `flush_prose` remain unreachable for these shapes through the full
// `Pipeline`, so everything above about revert-sensitivity stands.
//
// What #2401 DID fix is the downstream consequence of that pre-emption on
// the HTML render path: the `Break` nodes `HardBreaksPlugin` injects before
// `DirectiveRegistry` runs travel into the `MdxJsxFlowElement`
// `transform_block_container` builds, and `reconstruct_jsx` used to
// stringify them to the EMPTY string — deleting the author's newline with
// no separator at all (`first linesecond line`). They now render as
// `<br />`, pinned end-to-end by
// `collapsed_directive_body_hard_break_renders_br_on_the_html_path` above.
//
// The chain-ordering question this comment used to leave open is CLOSED.
// zfb#2412 characterized all five candidate directions against the tree and
// chose recognition tolerance over a reorder: `DirectiveRegistry` keeps its
// position, and `recognise_collapsed_run` (zfb#2413) recovers a collapsed
// run from the split shape instead. Moving the two visitors was rejected —
// both no-recurse at `MdxJsxFlowElement`, so running them after directives
// would blind them to every directive body, and compensating through
// `reparse_block` would re-fire the `SecondaryParsePlacement` gate and flip
// the `<br />` pin named above. Do not re-open it as a reorder; the
// reasoning is recorded on zfb#2412.
//
// The `markdown.cjkFriendly` half of the same pre-emption was MEASURED in
// #2408 and needed no renderer fix: `CjkFriendlyPlugin` injects `Strong`,
// a CONTAINER whose `to_string()` returns its children's text, so the body
// keeps every character and loses only the `<strong>` — the catch-all's
// deliberate lossiness, not deletion. Pinned by the zfb#2408 section
// above; documented in both locales of
// `docs/src/content/docs*/markdown-features/cjk-friendly.mdx`.
