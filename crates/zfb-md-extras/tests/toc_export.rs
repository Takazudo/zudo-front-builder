//! Fixture-based integration tests for the TOC-export feature.
//!
//! Each subdirectory under `tests/fixtures/toc_export/` is a self-contained
//! fixture: `input.md` holds the Markdown source and `expected.html` holds
//! the expected serialized output (including the `export const toc = [...]`
//! named export prepended by `TocExportPlugin`).
//!
//! The `verbatim` normalization mode is used for all fixtures because the
//! export line contains a precise JSON literal that must match exactly —
//! `normalize_html` would not change it, but verbatim comparison is more
//! explicit about the required contract.

use zfb_content::pipeline::Pipeline;
use zfb_content::serializer::serialize;
use zfb_md_extras::{MarkdownFeaturesConfig, TocExportConfig, test_harness::run_fixture};

/// Build a pipeline with `tocExport` enabled using the given config.
fn pipeline_with_toc_export(cfg: TocExportConfig) -> Pipeline {
    let features = MarkdownFeaturesConfig {
        toc_export: Some(cfg),
        ..Default::default()
    };
    Pipeline::with_defaults_and_features(&features)
}

/// Run a fixture directory with `tocExport` enabled (default config).
fn run(name: &str) {
    run_with_cfg(name, TocExportConfig::default());
}

/// Run a fixture directory with an explicit `TocExportConfig`.
fn run_with_cfg(name: &str, cfg: TocExportConfig) {
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/toc_export/"
    )
    .to_string()
        + name;
    run_fixture(&dir, |input| {
        let mut p = pipeline_with_toc_export(cfg.clone());
        let hast = p.run(input).expect("pipeline failed");
        serialize(&hast)
    });
}

// ── Acceptance criteria from the spec ───────────────────────────────────────

/// 3 h2s with nested h3s under each → structured toc with correct nesting.
#[test]
fn three_h2s_with_h3s() {
    run("three-h2s-with-h3s");
}

/// No headings → empty toc array (does not break consumers reading entry.toc).
#[test]
fn no_headings() {
    run("no-headings");
}

/// `maxDepth: 2` caps the export at h2 only.
#[test]
fn max_depth_2() {
    run_with_cfg("max-depth-2", TocExportConfig { max_depth: 2 });
}

/// h4 is excluded by the default maxDepth of 3.
#[test]
fn h4_excluded_by_default() {
    run("h4-excluded-by-default");
}

/// Special characters in heading text are properly JSON-escaped.
#[test]
fn special_chars() {
    run("special-chars");
}

/// Duplicate headings get deduplicated IDs from HeadingLinksPlugin;
/// the exported toc must reflect those deduplicated IDs (no drift).
#[test]
fn id_drift_check() {
    run("id-drift-check");
}

// ── Feature-disabled smoke test ──────────────────────────────────────────────

/// When `tocExport` is NOT enabled, no `export const toc` line appears.
#[test]
fn disabled_no_export() {
    let features = MarkdownFeaturesConfig::default();
    let mut p = Pipeline::with_defaults_and_features(&features);
    let hast = p.run("## Hello\n\nWorld.").expect("pipeline failed");
    let html = serialize(&hast);
    assert!(
        !html.contains("export const toc"),
        "disabled tocExport must not inject export: {html}"
    );
}

/// Verify that IDs in the export match the IDs HeadingLinksPlugin placed on
/// the heading elements (no drift).
#[test]
fn ids_match_heading_link_ids() {
    let mut p = pipeline_with_toc_export(TocExportConfig::default());
    let hast = p.run("## Foo Bar\n\nParagraph.").expect("pipeline failed");
    let html = serialize(&hast);
    // HeadingLinksPlugin slugifies "Foo Bar" to "foo-bar".
    assert!(
        html.contains("\"id\":\"foo-bar\""),
        "toc id must match heading slug: {html}"
    );
    assert!(
        html.contains("id=\"foo-bar\""),
        "heading element must have matching id: {html}"
    );
}
