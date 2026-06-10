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
    diagnostics::{CollectingSink, DiagnosticSeverity, MarkdownDiagnostic},
    heading_registry::{HeadingEntry, HeadingRegistry},
    LinkValidationConfig, MarkdownFeaturesConfig,
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
        cross_file_links: None,
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
    // Pre-populate an entry for the source file so entry-presence gating
    // allows the fragment check to fire (the fragment "no-such-heading"
    // is intentionally absent to trigger the broken-link report).
    registry.insert(
        source.clone(),
        HeadingEntry {
            id: "other-heading".to_string(),
            text: "Other Heading".to_string(),
            depth: 2,
        },
    );
    let md = "[foo](#no-such-heading)\n";
    let cfg = LinkValidationConfig {
        fail_on_broken: Some(true),
    };
    let diags = run(md, source, PathBuf::from("/project"), &mut registry, cfg);
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
        cross_file_links: None,
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
    // Pre-populate an entry for the source file so entry-presence gating
    // allows fragment checks to fire. The tested hrefs (#missing-a, #missing-b)
    // are intentionally absent so both produce a diagnostic.
    registry.insert(
        source.clone(),
        HeadingEntry {
            id: "some-heading".to_string(),
            text: "Some Heading".to_string(),
            depth: 2,
        },
    );
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

// ── Fixture 12: site-absolute hrefs are skipped (URL-space) ──────────────────

/// `[x](/docs/intro/)` and `[x](/docs/intro/#section)` → no diagnostics.
#[test]
fn site_absolute_href_skipped() {
    let source = PathBuf::from("/project/docs/page.md");
    let mut registry = HeadingRegistry::new();
    let md = "[x](/docs/intro/)\n\n[y](/docs/intro/#section)\n";
    let diags = run(
        md,
        source,
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert!(
        diags.is_empty(),
        "site-absolute hrefs must not emit diagnostics: {diags:?}"
    );
}

// ── Fixture 13: cross-file fragment degrades to existence-only when no entry ──

/// Registry has entries for OTHER files only; `[x](./other.md#whatever)` with
/// `other.md` on disk → no diagnostic (existence-only); with `other.md`
/// absent → one BrokenLink.
#[test]
fn cross_file_fragment_without_target_entry_degrades_to_existence_only() {
    let tmpdir = tempdir::TempDir::new("zfb-link-val-test").expect("tempdir");
    let project_root = tmpdir.path().to_path_buf();
    let source_path = tmpdir.path().join("page.md");
    let other_path = tmpdir.path().join("other.md");
    std::fs::write(&source_path, "").expect("write page.md");
    std::fs::write(&other_path, "# Other\n").expect("write other.md");

    let unrelated_path = tmpdir.path().join("unrelated.md");
    let mut registry = HeadingRegistry::new();
    // Registry has entries for a DIFFERENT file, not for `other.md`.
    registry.insert(
        unrelated_path,
        HeadingEntry {
            id: "something".to_string(),
            text: "Something".to_string(),
            depth: 2,
        },
    );

    // other.md on disk, no entry for it → existence-only → no diagnostic.
    let diags = run(
        "[x](./other.md#whatever)\n",
        source_path.clone(),
        project_root.clone(),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert!(
        diags.is_empty(),
        "existing target without registry entry must not emit diagnostic: {diags:?}"
    );

    // Remove other.md → BrokenLink (file missing).
    std::fs::remove_file(&other_path).expect("remove other.md");
    let diags2 = run(
        "[x](./other.md#whatever)\n",
        source_path,
        project_root,
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert_eq!(
        diags2.len(),
        1,
        "missing target must emit BrokenLink: {diags2:?}"
    );
    assert!(
        matches!(&diags2[0], MarkdownDiagnostic::BrokenLink { url, .. }
            if url == "./other.md#whatever"),
        "url must be raw href: {diags2:?}"
    );
}

// ── Fixture 14: bare fragment skipped when source has no registry entry ───────

/// Registry `Some` but no entry for the source file; `[x](#anything)` → no
/// diagnostic.
#[test]
fn bare_fragment_without_source_entry_skipped() {
    let source = PathBuf::from("/project/docs/page.md");
    let mut registry = HeadingRegistry::new();
    // No entry for source — entry-presence gating must skip.
    let md = "[x](#anything)\n";
    let diags = run(
        md,
        source,
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert!(
        diags.is_empty(),
        "bare fragment with no source entry must not emit diagnostic: {diags:?}"
    );
}

// ── Fixture 15: empty and percent-encoded fragments skipped ──────────────────

/// `[x](#)` and `[x](#a%20b)` (entry for source present) → no diagnostics.
#[test]
fn empty_and_percent_encoded_fragments_skipped() {
    let source = PathBuf::from("/project/docs/page.md");
    let mut registry = HeadingRegistry::new();
    // Pre-populate an entry so entry-presence gating fires.
    registry.insert(
        source.clone(),
        HeadingEntry {
            id: "existing".to_string(),
            text: "Existing".to_string(),
            depth: 2,
        },
    );
    let md = "[x](#)\n\n[y](#a%20b)\n";
    let diags = run(
        md,
        source,
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert!(
        diags.is_empty(),
        "empty and percent-encoded fragments must be skipped: {diags:?}"
    );
}

// ── Fixture 16: query string file link validates path only ────────────────────

/// `[x](./other.md?x=1)`, target on disk → no diagnostic (query stripped).
#[test]
fn query_string_file_link_validates_path_only() {
    let tmpdir = tempdir::TempDir::new("zfb-link-val-test").expect("tempdir");
    let project_root = tmpdir.path().to_path_buf();
    let source_path = tmpdir.path().join("page.md");
    let other_path = tmpdir.path().join("other.md");
    std::fs::write(&source_path, "").expect("write page.md");
    std::fs::write(&other_path, "# Other\n").expect("write other.md");

    let mut registry = HeadingRegistry::new();
    let md = "[x](./other.md?x=1)\n";
    let diags = run(
        md,
        source_path,
        project_root,
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert!(
        diags.is_empty(),
        "query-string file link with existing target must not emit diagnostic: {diags:?}"
    );
}
