//! Integration tests for the `link_validation` feature (Wave 6, #580).
//!
//! Exercises `LinkValidationPlugin` through the full pipeline with a
//! `BuildContext` carrying a pre-populated `HeadingRegistry` and a
//! `CollectingSink`. Each test case corresponds to one acceptance criterion
//! from the sub-issue spec.
//!
//! # Cross-file test setup
//!
//! The heading registry is pre-populated with entries for the "target" file
//! before running the pipeline. This mirrors how a full multi-file build would
//! operate (every file is processed through `HeadingLinksPlugin` with context
//! before link validation runs on the linking file).
//!
//! All source-file and target-file paths for filesystem tests use `tempdir` so
//! existence checks and project-root boundary checks work correctly.

use std::path::PathBuf;

use zfb_content::pipeline::{BuildContext, Pipeline};
use zfb_md_ast::{
    LinkValidationConfig, MarkdownFeaturesConfig,
    diagnostics::{CollectingSink, DiagnosticSeverity, MarkdownDiagnostic},
    heading_registry::{HeadingEntry, HeadingRegistry},
};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build a pipeline with `linkValidation` enabled using the given config.
fn make_pipeline(cfg: LinkValidationConfig) -> Pipeline {
    let features = MarkdownFeaturesConfig {
        link_validation: Some(cfg),
        ..Default::default()
    };
    Pipeline::with_defaults_and_features(&features)
}

/// Run the pipeline on `md` with a pre-populated registry and collect
/// diagnostics. `source_path` is the file being rendered; `project_root` is
/// the boundary used for path-traversal checks.
fn run(
    md: &str,
    source_path: PathBuf,
    project_root: PathBuf,
    registry: &mut HeadingRegistry,
    cfg: LinkValidationConfig,
) -> Vec<MarkdownDiagnostic> {
    let mut sink = CollectingSink::new();
    let mut ctx = BuildContext {
        source_path: Some(source_path),
        project_root,
        public_dir: PathBuf::from("/project/public"),
        heading_registry: Some(registry),
        diagnostics: Some(&mut sink),
    };
    let mut pipeline = make_pipeline(cfg);
    pipeline
        .run_with_context(md, &mut ctx)
        .expect("pipeline must not fail");
    sink.take()
}

// ── Fixture 1: external URLs are skipped ──────────────────────────────────────

/// `[foo](https://example.com)` → no diagnostic (external, skipped by default).
#[test]
fn external_url_skipped() {
    let mut registry = HeadingRegistry::new();
    let source = PathBuf::from("/project/docs/page.md");
    let md = "[foo](https://example.com)\n";
    let diags = run(
        md,
        source,
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert!(
        diags.is_empty(),
        "external URL must not emit a diagnostic: {diags:?}"
    );
}

// ── Fixture 2: bare anchor — known heading → no diagnostic ────────────────────

/// `[foo](#known-heading)` where the current file has that heading → ok.
#[test]
fn bare_anchor_known_heading_no_diagnostic() {
    let source = PathBuf::from("/project/docs/page.md");
    let mut registry = HeadingRegistry::new();
    registry.insert(
        source.clone(),
        HeadingEntry {
            id: "known-heading".to_string(),
            text: "Known Heading".to_string(),
            depth: 2,
        },
    );
    let md = "[foo](#known-heading)\n";
    let diags = run(
        md,
        source,
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert!(
        diags.is_empty(),
        "known anchor must not emit a diagnostic: {diags:?}"
    );
}

// ── Fixture 3: bare anchor — missing heading → diagnostic ────────────────────

/// `[foo](#missing-heading)` where no such heading exists → warning emitted.
#[test]
fn bare_anchor_missing_heading_emits_warning() {
    let source = PathBuf::from("/project/docs/page.md");
    let mut registry = HeadingRegistry::new();
    // Registry has a different heading, not "missing-heading".
    registry.insert(
        source.clone(),
        HeadingEntry {
            id: "other-heading".to_string(),
            text: "Other Heading".to_string(),
            depth: 2,
        },
    );
    let md = "[foo](#missing-heading)\n";
    let diags = run(
        md,
        source,
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert_eq!(diags.len(), 1, "expected one diagnostic: {diags:?}");
    assert_eq!(
        diags[0].severity(),
        DiagnosticSeverity::Warning,
        "default severity must be Warning: {diags:?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "#missing-heading"),
        "diagnostic url must be the raw href: {diags:?}"
    );
}

// ── Fixture 4: cross-file anchor — known heading → no diagnostic ──────────────

/// `[foo](./other.md#known-heading)` where `other.md` defines that heading → ok.
#[test]
fn cross_file_known_anchor_no_diagnostic() {
    // Create a real tempdir so the filesystem-existence and project-root checks pass.
    let tmpdir = tempdir::TempDir::new("zfb-link-val-test").expect("tempdir");
    let project_root = tmpdir.path().to_path_buf();
    let source_path = tmpdir.path().join("page.md");
    let other_path = tmpdir.path().join("other.md");
    // Create files on disk so existence checks pass.
    std::fs::write(&other_path, "# Other\n").expect("write other.md");
    std::fs::write(&source_path, "").expect("write page.md");

    let mut registry = HeadingRegistry::new();
    registry.insert(
        other_path,
        HeadingEntry {
            id: "known-heading".to_string(),
            text: "Known Heading".to_string(),
            depth: 2,
        },
    );

    let md = "[foo](./other.md#known-heading)\n";
    let diags = run(
        md,
        source_path,
        project_root,
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert!(
        diags.is_empty(),
        "cross-file known anchor must not emit a diagnostic: {diags:?}"
    );
}

// ── Fixture 5: cross-file anchor — missing heading → diagnostic ───────────────

/// `[foo](./other.md#missing-heading)` → warning.
#[test]
fn cross_file_missing_anchor_emits_warning() {
    let tmpdir = tempdir::TempDir::new("zfb-link-val-test").expect("tempdir");
    let project_root = tmpdir.path().to_path_buf();
    let source_path = tmpdir.path().join("page.md");
    let other_path = tmpdir.path().join("other.md");
    std::fs::write(&other_path, "# Other\n").expect("write other.md");
    std::fs::write(&source_path, "").expect("write page.md");

    let mut registry = HeadingRegistry::new();
    // Register the file but with a DIFFERENT heading ID.
    registry.insert(
        other_path,
        HeadingEntry {
            id: "existing-heading".to_string(),
            text: "Existing Heading".to_string(),
            depth: 2,
        },
    );

    let md = "[foo](./other.md#missing-heading)\n";
    let diags = run(
        md,
        source_path,
        project_root,
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert_eq!(diags.len(), 1, "expected one diagnostic: {diags:?}");
    assert_eq!(diags[0].severity(), DiagnosticSeverity::Warning);
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. }
            if url == "./other.md#missing-heading"),
        "url must be the raw href: {diags:?}"
    );
}

// ── Fixture 6: missing file → diagnostic ──────────────────────────────────────

/// `[foo](./missing.md)` where the file does not exist → warning.
#[test]
fn missing_file_emits_warning() {
    let tmpdir = tempdir::TempDir::new("zfb-link-val-test").expect("tempdir");
    let project_root = tmpdir.path().to_path_buf();
    let source_path = tmpdir.path().join("page.md");
    std::fs::write(&source_path, "").expect("write page.md");

    let mut registry = HeadingRegistry::new();
    let md = "[foo](./missing.md)\n";
    let diags = run(
        md,
        source_path,
        project_root,
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert_eq!(diags.len(), 1, "expected one diagnostic: {diags:?}");
    assert_eq!(diags[0].severity(), DiagnosticSeverity::Warning);
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. }
            if url == "./missing.md"),
        "url must be the raw href: {diags:?}"
    );
}

// ── Fixture 7: failOnBroken: true → Error severity ────────────────────────────

/// When `failOnBroken: true`, broken links emit `Error` diagnostics.
#[test]
fn fail_on_broken_true_emits_error() {
    let source = PathBuf::from("/project/docs/page.md");
    let mut registry = HeadingRegistry::new();
    let md = "[foo](#no-such-heading)\n";
    let cfg = LinkValidationConfig {
        fail_on_broken: Some(true),
        allow_external: None,
    };
    let diags = run(
        md,
        source,
        PathBuf::from("/project"),
        &mut registry,
        cfg,
    );
    assert_eq!(diags.len(), 1, "expected one diagnostic: {diags:?}");
    assert_eq!(
        diags[0].severity(),
        DiagnosticSeverity::Error,
        "failOnBroken:true must emit Error severity: {diags:?}"
    );
}

// ── Fixture 8: feature disabled → no diagnostics emitted ─────────────────────

/// When `linkValidation` is NOT enabled, no diagnostics are collected.
#[test]
fn feature_disabled_no_diagnostics() {
    let source = PathBuf::from("/project/docs/page.md");
    let mut registry = HeadingRegistry::new();
    let mut sink = CollectingSink::new();
    let mut ctx = BuildContext {
        source_path: Some(source),
        project_root: PathBuf::from("/project"),
        public_dir: PathBuf::from("/project/public"),
        heading_registry: Some(&mut registry),
        diagnostics: Some(&mut sink),
    };
    // Pipeline WITHOUT link_validation feature.
    let features = MarkdownFeaturesConfig::default();
    let mut pipeline = Pipeline::with_defaults_and_features(&features);
    let md = "[foo](#no-such-heading)\n";
    pipeline
        .run_with_context(md, &mut ctx)
        .expect("pipeline must not fail");
    let diags = sink.take();
    assert!(
        diags.is_empty(),
        "disabled feature must not emit diagnostics: {diags:?}"
    );
}

// ── Fixture 9: multiple broken links — all emitted ───────────────────────────

/// Multiple broken links in one document → one diagnostic per broken link.
#[test]
fn multiple_broken_links_all_emitted() {
    let source = PathBuf::from("/project/docs/page.md");
    let mut registry = HeadingRegistry::new();
    let md = "[a](#missing-a)\n\n[b](#missing-b)\n";
    let diags = run(
        md,
        source,
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert_eq!(
        diags.len(),
        2,
        "two broken links must emit two diagnostics: {diags:?}"
    );
}

// ── Fixture 10: valid file without anchor → no diagnostic ────────────────────

/// `[foo](./existing.md)` where the file exists → no diagnostic.
#[test]
fn existing_file_no_anchor_no_diagnostic() {
    let tmpdir = tempdir::TempDir::new("zfb-link-val-test").expect("tempdir");
    let project_root = tmpdir.path().to_path_buf();
    let source_path = tmpdir.path().join("page.md");
    let target_path = tmpdir.path().join("existing.md");
    std::fs::write(&target_path, "# Target\n").expect("write existing.md");
    std::fs::write(&source_path, "").expect("write page.md");

    let mut registry = HeadingRegistry::new();
    let md = "[foo](./existing.md)\n";
    let diags = run(
        md,
        source_path,
        project_root,
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert!(
        diags.is_empty(),
        "existing file without anchor must not emit a diagnostic: {diags:?}"
    );
}

// ── Fixture 11: path traversal → diagnostic ───────────────────────────────────

/// `[foo](../outside.md)` that escapes project_root → diagnostic, even if
/// the file happens to exist on disk.
#[test]
fn path_traversal_outside_project_root_emits_diagnostic() {
    let tmpdir = tempdir::TempDir::new("zfb-link-val-test").expect("tempdir");
    // Create a sub-directory as the project root; source is inside it.
    let project_root = tmpdir.path().join("project");
    std::fs::create_dir_all(&project_root).expect("create project dir");
    let source_path = project_root.join("page.md");
    std::fs::write(&source_path, "").expect("write page.md");
    // Create a file OUTSIDE the project root so existence alone would pass.
    let outside = tmpdir.path().join("outside.md");
    std::fs::write(&outside, "# Outside\n").expect("write outside.md");

    let mut registry = HeadingRegistry::new();
    let md = "[foo](../outside.md)\n";
    let diags = run(
        md,
        source_path,
        project_root,
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert_eq!(
        diags.len(),
        1,
        "path traversal outside project root must emit a diagnostic: {diags:?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. }
            if url == "../outside.md"),
        "url must be the raw href: {diags:?}"
    );
}
