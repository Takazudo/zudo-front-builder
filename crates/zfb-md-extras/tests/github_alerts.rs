//! Fixture-based integration tests for the GitHub-alerts feature.
//!
//! Each subdirectory under `tests/fixtures/github_alerts/` is a self-contained
//! fixture: `input.md` holds the Markdown source and `expected.html` holds the
//! expected HTML output after running through a fully-wired pipeline with
//! `githubAlerts: true`.
//!
//! The `unknown-type` fixture is the only one that exercises the disabled-path
//! (unknown `[!CUSTOM]` is left as a plain blockquote even when the feature is
//! on). A separate inline test below also verifies that setting
//! `githubAlerts: false` leaves a known alert prefix as a blockquote.

use zfb_content::pipeline::Pipeline;
use zfb_content::serializer::serialize;
use zfb_md_ast::FeatureToggle;
use zfb_md_extras::{test_harness::run_fixture, MarkdownFeaturesConfig};

/// Build a pipeline with `githubAlerts` set to the given toggle.
fn pipeline_with_alerts(enabled: bool) -> Pipeline {
    let features = MarkdownFeaturesConfig {
        github_alerts: Some(FeatureToggle::Bool(enabled)),
        ..Default::default()
    };
    Pipeline::with_defaults_and_features(&features)
}

/// Run a fixture directory against the github_alerts-enabled pipeline.
fn run(name: &str) {
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/github_alerts/"
    )
    .to_string()
        + name;
    run_fixture(&dir, |input| {
        let mut p = pipeline_with_alerts(true);
        let hast = p.run(input).expect("pipeline failed");
        serialize(&hast)
    });
}

// ── Five alert types ─────────────────────────────────────────────────────────

#[test]
fn note() {
    run("note");
}

#[test]
fn tip() {
    run("tip");
}

#[test]
fn important() {
    run("important");
}

#[test]
fn warning() {
    run("warning");
}

#[test]
fn caution() {
    run("caution");
}

// ── Edge cases ────────────────────────────────────────────────────────────────

#[test]
fn multiple_paragraphs() {
    run("multiple-paragraphs");
}

#[test]
fn alert_in_list() {
    run("alert-in-list");
}

/// An unknown alert type (`[!CUSTOM]`) must be left as a plain blockquote even
/// when the github_alerts feature is enabled.
#[test]
fn unknown_type_stays_blockquote() {
    run("unknown-type");
}

// ── Feature-disabled smoke test ───────────────────────────────────────────────

/// When `githubAlerts: false`, a `[!NOTE]` blockquote must remain as HTML
/// `<blockquote>…</blockquote>` rather than being rewritten to `<Note>`.
#[test]
fn disabled_leaves_blockquote_unchanged() {
    let input = "> [!NOTE]\n> This should stay as a blockquote.";
    let mut p = pipeline_with_alerts(false);
    let hast = p.run(input).expect("pipeline failed");
    let html = serialize(&hast);
    assert!(
        html.contains("<blockquote>"),
        "disabled github_alerts must leave blockquote: {html}"
    );
    assert!(
        !html.contains("<Note>"),
        "disabled github_alerts must NOT produce <Note>: {html}"
    );
}

// ── Both features on: consistent output ──────────────────────────────────────

/// When both `admonitionsPreset` and `githubAlerts` are enabled, a GitHub-
/// style alert must still render correctly (github_alerts runs first and
/// converts the blockquote before admonitionsPreset sees it).
#[test]
fn both_features_on_alert_renders_correctly() {
    let features = MarkdownFeaturesConfig {
        github_alerts: Some(FeatureToggle::Bool(true)),
        admonitions_preset: Some(FeatureToggle::Bool(true)),
        ..Default::default()
    };
    let mut p = Pipeline::with_defaults_and_features(&features);
    let hast = p
        .run("> [!WARNING]\n> Watch out!")
        .expect("pipeline failed");
    let html = serialize(&hast);
    assert_eq!(
        html, "<Warning>Watch out!</Warning>",
        "both features on: alert must render as <Warning>: {html}"
    );
}
