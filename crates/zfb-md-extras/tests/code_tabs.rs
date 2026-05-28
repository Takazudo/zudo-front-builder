//! Fixture-based integration tests for the code-tabs feature.
//!
//! Each subdirectory under `tests/fixtures/code_tabs/` is a self-contained
//! fixture: `input.md` holds the Markdown source and `expected.html` holds
//! the expected HTML output after running through a fully-wired pipeline
//! with `codeTabs: true`.
//!
//! Fixture catalogue:
//! - `explicit-titles`  — 3 code blocks with explicit `title="…"` meta
//! - `lang-fallback`    — 2 code blocks without titles (fallback to `lang`)
//! - `two-tabs-explicit` — 2 code blocks with explicit titles (basic happy path)
//! - `mixed-titles`     — mix of titled and untitled code blocks
//! - `no-lang-fallback` — code blocks with no lang or title (fall back to `"tab"`)

use zfb_content::pipeline::Pipeline;
use zfb_content::serializer::serialize;
use zfb_md_ast::FeatureToggle;
use zfb_md_extras::{test_harness::run_fixture, MarkdownFeaturesConfig};

/// Build a pipeline with `codeTabs` set to the given toggle.
fn pipeline_with_code_tabs(enabled: bool) -> Pipeline {
    let features = MarkdownFeaturesConfig {
        code_tabs: Some(FeatureToggle::Bool(enabled)),
        ..Default::default()
    };
    Pipeline::with_defaults_and_features(&features)
}

/// Run a fixture directory against the code-tabs-enabled pipeline.
fn run(name: &str) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/code_tabs/").to_string() + name;
    run_fixture(&dir, |input| {
        let mut p = pipeline_with_code_tabs(true);
        let hast = p.run(input).expect("pipeline failed");
        serialize(&hast)
    });
}

// ── Fixture tests ──────────────────────────────────────────────────────────

#[test]
fn explicit_titles() {
    run("explicit-titles");
}

#[test]
fn lang_fallback() {
    run("lang-fallback");
}

#[test]
fn two_tabs_explicit() {
    run("two-tabs-explicit");
}

#[test]
fn mixed_titles() {
    run("mixed-titles");
}

#[test]
fn no_lang_fallback() {
    run("no-lang-fallback");
}

// ── Feature-disabled smoke test ───────────────────────────────────────────────

/// When `codeTabs: false`, a `:::code-group` container must NOT be rewritten.
/// The opener paragraph remains as a plain paragraph in the output.
#[test]
fn disabled_leaves_code_group_unchanged() {
    let input =
        ":::code-group\n\n```ts\nconst x = 1;\n```\n\n:::\n";
    let mut p = pipeline_with_code_tabs(false);
    let hast = p.run(input).expect("pipeline failed");
    let html = serialize(&hast);
    // The opener paragraph should render as a plain <p> with the text.
    assert!(
        html.contains("code-group"),
        "disabled code_tabs must leave :::code-group as plain paragraph: {html}"
    );
    assert!(
        !html.contains("CodeGroup"),
        "disabled code_tabs must NOT produce <CodeGroup>: {html}"
    );
}
