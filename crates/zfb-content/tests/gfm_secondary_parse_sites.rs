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

/// A pipeline with both secondary parse sites wired: transclude, plus a
/// `:::note` container directive.
///
/// `cjk_friendly` is a parameter because the CJK autolink boundary tests
/// need it on while the parity tests deliberately keep it off (so a
/// boundary rewrite can never be mistaken for the construct threading
/// under test).
fn pipeline(gfm: ResolvedGfmConstructs, cjk_friendly: bool) -> Pipeline {
    let mut directives = HashMap::new();
    directives.insert("note".to_string(), DirectiveSpec::Short("Note".to_string()));
    let features = MarkdownFeaturesConfig {
        transclude: Some(TranscludeConfig::default()),
        directives: Some(directives),
        ..Default::default()
    };
    Pipeline::with_defaults_and_full_config(None, gfm, None, cjk_friendly, false, Some(&features))
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
    let mut p = pipeline(gfm, cjk_friendly);
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
    std::fs::write(dir.join("snippet.md"), body).expect("write snippet");
    r#":::include{file="./snippet.md"}"#.to_string()
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
    let mut p = pipeline(gfm, cjk_friendly);
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
