//! Fixture-based integration tests for the ruby annotation feature.
//!
//! Each subdirectory under `tests/fixtures/ruby/` is a self-contained
//! fixture: `input.md` holds the Markdown source and `expected.html` holds
//! the expected HTML output after running through a fully-wired pipeline with
//! `ruby: true`.

use zfb_content::pipeline::Pipeline;
use zfb_content::serializer::serialize;
use zfb_md_ast::FeatureToggle;
use zfb_md_extras::{test_harness::run_fixture, MarkdownFeaturesConfig};

/// Build a pipeline with `ruby` set to the given toggle.
fn pipeline_with_ruby(enabled: bool) -> Pipeline {
    let features = MarkdownFeaturesConfig {
        ruby: Some(FeatureToggle::Bool(enabled)),
        ..Default::default()
    };
    Pipeline::with_defaults_and_features(&features)
}

/// Run a fixture directory against the ruby-enabled pipeline.
fn run(name: &str) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ruby/").to_string() + name;
    run_fixture(&dir, |input| {
        let mut p = pipeline_with_ruby(true);
        let hast = p.run(input).expect("pipeline failed");
        serialize(&hast)
    });
}

// ── Core fixtures ─────────────────────────────────────────────────────────────

/// Basic acceptance criterion: `{漢字|かんじ}` → `<ruby>…</ruby>`.
#[test]
fn basic() {
    run("basic");
}

/// Multiple annotations in one paragraph.
#[test]
fn multiple() {
    run("multiple");
}

/// Annotation surrounded by plain text.
#[test]
fn mixed_text() {
    run("mixed-text");
}

/// Invalid syntax (unbalanced/no-pipe) is left as literal text.
#[test]
fn invalid_syntax() {
    run("invalid-syntax");
}

/// Adjacent annotations (no space between them).
#[test]
fn adjacent() {
    run("adjacent");
}

// ── Feature-disabled smoke tests ──────────────────────────────────────────────

/// When `ruby: false`, `{漢字|かんじ}` must remain as literal text.
#[test]
fn disabled_leaves_annotation_as_text() {
    let input = "{漢字|かんじ}";
    let mut p = pipeline_with_ruby(false);
    let hast = p.run(input).expect("pipeline failed");
    let html = serialize(&hast);
    assert!(
        !html.contains("<ruby>"),
        "disabled ruby must NOT produce <ruby>: {html}"
    );
    assert!(
        html.contains("漢字"),
        "disabled ruby must preserve base text: {html}"
    );
}

/// When `ruby: true`, empty base or empty ruby annotation is left as text.
#[test]
fn empty_base_left_as_text() {
    let input = "{|かんじ}";
    let mut p = pipeline_with_ruby(true);
    let hast = p.run(input).expect("pipeline failed");
    let html = serialize(&hast);
    assert!(
        !html.contains("<ruby>"),
        "empty base must NOT produce <ruby>: {html}"
    );
}

/// When `ruby: true`, empty ruby annotation is left as text.
#[test]
fn empty_ruby_left_as_text() {
    let input = "{漢字|}";
    let mut p = pipeline_with_ruby(true);
    let hast = p.run(input).expect("pipeline failed");
    let html = serialize(&hast);
    assert!(
        !html.contains("<ruby>"),
        "empty ruby annotation must NOT produce <ruby>: {html}"
    );
}
