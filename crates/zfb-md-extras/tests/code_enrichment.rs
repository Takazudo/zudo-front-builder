//! Fixture-based integration tests for the code-enrichment feature.
//!
//! Each subdirectory under `tests/fixtures/code_enrichment/` is a self-contained
//! fixture: `input.md` holds the Markdown source and `expected.html` holds the
//! expected HTML output after running through a fully-wired pipeline with
//! `codeEnrichment: {}` (diff markers, line highlighting, and word emphasis
//! enabled).

use zfb_content::pipeline::Pipeline;
use zfb_content::serializer::serialize;
use zfb_content::{CodeHighlightMode, PipelineSpec};
use zfb_md_ast::CodeEnrichmentConfig;
use zfb_md_extras::{test_harness::run_fixture, MarkdownFeaturesConfig};

/// Build a pipeline with `codeEnrichment` set using a full config object.
fn pipeline_with_enrichment(cfg: CodeEnrichmentConfig) -> Pipeline {
    let features = MarkdownFeaturesConfig {
        code_enrichment: Some(cfg),
        ..Default::default()
    };
    Pipeline::with_defaults_and_features(&features)
}

/// Build a pipeline with all enrichment features on (the default).
fn pipeline_all_on() -> Pipeline {
    pipeline_with_enrichment(CodeEnrichmentConfig::default())
}

/// Run a fixture directory against the code-enrichment-enabled pipeline.
fn run(name: &str) {
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/code_enrichment/"
    )
    .to_string()
        + name;
    run_fixture(&dir, |input| {
        let mut p = pipeline_all_on();
        let hast = p.run(input).expect("pipeline failed");
        serialize(&hast)
    });
}

/// Build a class-mode pipeline (`codeHighlight.mode: "class"`, Highlight
/// Tokens epic zfb#1528) with all enrichment features on — the confirm
/// sub's #1535 check 1: enrichment × class mode.
fn pipeline_class_all_on() -> Pipeline {
    let features = MarkdownFeaturesConfig {
        code_enrichment: Some(CodeEnrichmentConfig::default()),
        ..Default::default()
    };
    let spec = PipelineSpec {
        code_highlight_mode: CodeHighlightMode::Class,
        features: Some(features),
        ..Default::default()
    };
    spec.build_pipeline()
        .expect("class-mode enrichment pipeline build must not fail: no themesDir is configured")
}

/// Run a fixture directory against the class-mode + enrichment-enabled
/// pipeline (mirrors [`run`] but for the class-emission path).
fn run_class(name: &str) {
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/code_enrichment/"
    )
    .to_string()
        + name;
    run_fixture(&dir, |input| {
        let mut p = pipeline_class_all_on();
        let hast = p.run(input).expect("pipeline failed");
        serialize(&hast)
    });
}

/// Run a newly-authored exact-output fixture while tolerating the fixture
/// file's conventional final newline. Existing normalized fixtures predate
/// this helper and intentionally remain on [`run_fixture`].
fn run_word_fixture(name: &str, class_mode: bool) {
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/code_enrichment/"
    )
    .to_string()
        + name;
    let input = std::fs::read_to_string(format!("{dir}/input.md")).expect("fixture input");
    let expected =
        std::fs::read_to_string(format!("{dir}/expected.html")).expect("fixture expected HTML");
    let mut pipeline = if class_mode {
        pipeline_class_all_on()
    } else {
        pipeline_all_on()
    };
    let actual = serialize(&pipeline.run(&input).expect("pipeline failed"));
    assert_eq!(actual.trim_end(), expected.trim_end(), "fixture {name}");
}

// ── Line highlighting ──────────────────────────────────────────────────────────

/// Fence with `js {1}` — line 1 gets `data-line-highlight="true"`.
#[test]
fn line_highlight_basic() {
    run("line-highlight-basic");
}

/// Fence with `js {1,3-5}` — lines 1, 3, 4, 5 highlighted.
#[test]
fn line_highlight_range() {
    run("line-highlight-range");
}

/// Line highlight + code title in meta — both features coexist.
#[test]
fn line_highlight_with_title() {
    run("line-highlight-with-title");
}

// ── Diff markers ───────────────────────────────────────────────────────────────

/// A line with `// [!code ++]` → `data-line-diff="added"`, marker stripped.
#[test]
fn diff_add_marker() {
    run("diff-add-marker");
}

/// A line with `// [!code --]` → `data-line-diff="removed"`, marker stripped.
#[test]
fn diff_del_marker() {
    run("diff-del-marker");
}

/// Both `++` and `--` markers in the same block — each line annotated correctly.
#[test]
fn diff_both_markers() {
    run("diff-both-markers");
}

/// Diff marker on the last line of a code block.
#[test]
fn diff_last_line() {
    run("diff-last-line");
}

/// Diff marker on a blank line (blank content before the marker).
#[test]
fn diff_empty_lines() {
    run("diff-empty-lines");
}

// ── Combined ───────────────────────────────────────────────────────────────────

/// Line highlight + diff marker in the same block.
#[test]
fn both_features_combined() {
    run("both-features-combined");
}

// ── Word highlighting ─────────────────────────────────────────────────

/// Word emphasis coexists with a code title, line range, diff marker, and a
/// second metadata expression in inline-style highlighting mode.
#[test]
fn word_highlight_combined_inline_mode() {
    run_word_fixture("word-highlight-combined", false);
}

/// The same stable marker preserves semantic syntax-role classes.
#[test]
fn word_highlight_class_mode() {
    run_word_fixture("class-mode-word-highlight", true);
}

/// Escaped slash delimiters survive Markdown fence metadata parsing and match
/// the decoded fallback text of an unknown language.
#[test]
fn word_highlight_escaped_slash_expression() {
    let mut pipeline = pipeline_all_on();
    let input = "```unknown /path\\/name/\npath/name\n```\n";
    let html = serialize(&pipeline.run(input).expect("pipeline failed"));
    assert!(
        html.contains(r#"<span class="highlighted-word">path/name</span>"#),
        "got: {html}"
    );
}

// ── Class mode (Highlight Tokens epic zfb#1528, confirm sub #1535) ─────────────

/// `{1,3}` line highlighting AND `[!code ++]`/`[!code --]` diff markers
/// through a CLASS-MODE pipeline (`codeHighlight.mode: "class"`).
///
/// Proves two things at once:
/// - `data-line-highlight`/`data-line-diff` land on `<span class="line">`
///   exactly as in inline mode — enrichment reads/writes the `<span
///   class="line">` wrapper structure, which is identical across
///   inline/dual/class (see `syntect_plugin.rs::build_line_spans`).
/// - The diff-marker strip path removes the marker's WHOLE `<span>` even
///   when that span carries `class="hi-com"` instead of inline
///   `style="color:…"` — `try_strip_whole_span`
///   (`crates/zfb-md-extras/src/code_enrichment.rs`) matches on the
///   literal `<span` / `>` / `</span>` boundaries, not on any specific
///   attribute, so it is attribute-scheme-agnostic by construction. This
///   fixture is the first regression guard that actually exercises that
///   codepath against a `class="…"` (not `style="…"`) comment span.
///
/// Regenerated by running the pipeline directly (see
/// `crates/zfb-md-extras/scripts/gen-fixture.mjs`'s header note: that
/// script targets porting upstream remark/rehype plugins and does not
/// apply to this Rust-pipeline fixture set — every existing fixture under
/// `tests/fixtures/code_enrichment/` was produced the same way, by running
/// the real `Pipeline` once and committing its output).
#[test]
fn class_mode_line_highlight_and_diff_markers_combined() {
    run_class("class-mode-combined");
}

// ── Feature-disabled smoke tests ──────────────────────────────────────────────

/// `diffMarkers: false` — `// [!code ++]` is NOT stripped and no attribute added.
#[test]
fn diff_markers_disabled_does_not_strip() {
    let cfg = CodeEnrichmentConfig {
        diff_markers: Some(false),
        line_highlight: Some(true),
        word_highlight: Some(true),
    };
    let mut p = pipeline_with_enrichment(cfg);
    let input = "```js\nconst x = 1; // [!code ++]\n```\n";
    let hast = p.run(input).expect("pipeline failed");
    let html = serialize(&hast);
    // The marker must NOT be stripped (it remains in the output HTML).
    assert!(
        html.contains("[!code ++]"),
        "diffMarkers=false must leave marker visible: {html}"
    );
    assert!(
        !html.contains("data-line-diff"),
        "diffMarkers=false must not add data-line-diff: {html}"
    );
}

/// `lineHighlight: false` — `{1}` in meta does NOT produce `data-line-highlight`.
#[test]
fn line_highlight_disabled_does_not_annotate() {
    let cfg = CodeEnrichmentConfig {
        diff_markers: Some(true),
        line_highlight: Some(false),
        word_highlight: Some(true),
    };
    let mut p = pipeline_with_enrichment(cfg);
    let input = "```js {1}\nconst x = 1;\n```\n";
    let hast = p.run(input).expect("pipeline failed");
    let html = serialize(&hast);
    assert!(
        !html.contains("data-line-highlight"),
        "lineHighlight=false must not add data-line-highlight: {html}"
    );
}

/// `wordHighlight: false` suppresses only word emphasis while line and diff
/// processing remain enabled.
#[test]
fn word_highlight_disabled_keeps_other_enrichments() {
    let cfg = CodeEnrichmentConfig {
        diff_markers: Some(true),
        line_highlight: Some(true),
        word_highlight: Some(false),
    };
    let mut p = pipeline_with_enrichment(cfg);
    let input = "```js {1} /answer/\nconst answer = 1; // [!code ++]\n```\n";
    let html = serialize(&p.run(input).expect("pipeline failed"));
    assert!(html.contains(r#"data-line-diff="added""#), "got: {html}");
    assert!(
        html.contains(r#"data-line-highlight="true""#),
        "got: {html}"
    );
    assert!(!html.contains("highlighted-word"), "got: {html}");
    assert!(!html.contains("[!code ++]"), "got: {html}");
}

/// When all flags are `false`, the visitor is a no-op.
#[test]
fn both_disabled_is_noop() {
    let cfg_disabled = CodeEnrichmentConfig {
        diff_markers: Some(false),
        line_highlight: Some(false),
        word_highlight: Some(false),
    };
    let cfg_none = MarkdownFeaturesConfig::default();

    let input = "```js {1}\nconst x = 1; // [!code ++]\n```\n";

    let html_disabled = {
        let mut p = pipeline_with_enrichment(cfg_disabled);
        serialize(&p.run(input).expect("pipeline failed"))
    };
    let html_nofeature = {
        let mut p = Pipeline::with_defaults_and_features(&cfg_none);
        serialize(&p.run(input).expect("pipeline failed"))
    };

    // Without the feature, there must be no enrichment attributes.
    assert!(
        !html_disabled.contains("data-line-highlight"),
        "both-off must not annotate lines: {html_disabled}"
    );
    assert!(
        !html_disabled.contains("data-line-diff"),
        "both-off must not add diff attrs: {html_disabled}"
    );
    // No feature at all: same expectation.
    let _ = html_nofeature;
}
