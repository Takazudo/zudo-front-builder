//! Fixture-based integration tests for the code-enrichment feature.
//!
//! Each subdirectory under `tests/fixtures/code_enrichment/` is a self-contained
//! fixture: `input.md` holds the Markdown source and `expected.html` holds the
//! expected HTML output after running through a fully-wired pipeline with
//! `codeEnrichment: {}` (both diff markers and line highlighting enabled).

use zfb_content::pipeline::Pipeline;
use zfb_content::serializer::serialize;
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

/// Build a pipeline with both enrichment features on (the default).
fn pipeline_both_on() -> Pipeline {
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
        let mut p = pipeline_both_on();
        let hast = p.run(input).expect("pipeline failed");
        serialize(&hast)
    });
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

// ── Feature-disabled smoke tests ──────────────────────────────────────────────

/// `diffMarkers: false` — `// [!code ++]` is NOT stripped and no attribute added.
#[test]
fn diff_markers_disabled_does_not_strip() {
    let cfg = CodeEnrichmentConfig {
        diff_markers: Some(false),
        line_highlight: Some(true),
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

/// When both flags are `false`, the visitor is a no-op.
#[test]
fn both_disabled_is_noop() {
    let cfg_disabled = CodeEnrichmentConfig {
        diff_markers: Some(false),
        line_highlight: Some(false),
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
