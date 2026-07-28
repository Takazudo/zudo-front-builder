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
    let tmpdir = tempfile::Builder::new()
        .prefix("zfb-link-val-test")
        .tempdir()
        .expect("tempdir");
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
    let tmpdir = tempfile::Builder::new()
        .prefix("zfb-link-val-test")
        .tempdir()
        .expect("tempdir");
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
    let tmpdir = tempfile::Builder::new()
        .prefix("zfb-link-val-test")
        .tempdir()
        .expect("tempdir");
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
    let tmpdir = tempfile::Builder::new()
        .prefix("zfb-link-val-test")
        .tempdir()
        .expect("tempdir");
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
    let tmpdir = tempfile::Builder::new()
        .prefix("zfb-link-val-test")
        .tempdir()
        .expect("tempdir");
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
    let tmpdir = tempfile::Builder::new()
        .prefix("zfb-link-val-test")
        .tempdir()
        .expect("tempdir");
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

// ── Fixture 14: bare fragment skipped when the file was never tracked ─────────

/// Registry `Some` but **no entry at all** for the source file — the
/// genuinely-untracked / cache-hit shape where `HeadingLinksPlugin` never ran
/// (a compile-cache hit replays output without re-running the hast chain, see
/// the registry contract in `pipeline.rs`). Entry-presence gating must degrade
/// to skip here so a replayed file does not produce a spurious diagnostic.
///
/// This drives `LinkValidationPlugin` directly (NOT through the full pipeline):
/// on the `run_with_context` path `HeadingLinksPlugin` now marks every compiled
/// file tracked (`Some(&[])`), so a "no entry" source is unreachable there —
/// that headingless-but-compiled case is the one #1093 fixes and is covered by
/// `transclude_link_validation_broken_link_in_snippet` in
/// `cross_feature_integration.rs`. The skip-on-`None` branch this fixture
/// guards only fires when the file was never tracked at all.
#[test]
fn bare_fragment_without_source_entry_skipped() {
    use zfb_content::pipeline::{HastNode, HastVisitor};
    use zfb_md_extras::link_validation::LinkValidationPlugin;

    let source = PathBuf::from("/project/docs/page.md");
    // Registry has NO entry for source — file was never tracked.
    let mut registry = HeadingRegistry::new();
    let mut sink = CollectingSink::new();

    // A minimal hast tree: <a href="#anything">x</a>.
    let mut tree = HastNode::Root {
        children: vec![HastNode::Element {
            tag: "a".to_string(),
            attrs: vec![("href".to_string(), "#anything".to_string())],
            children: vec![HastNode::Text("x".to_string())],
            void: false,
        }],
    };

    let mut ctx = BuildContext {
        source_path: Some(source),
        project_root: PathBuf::from("/project"),
        public_dir: PathBuf::from("/project/public"),
        heading_registry: Some(&mut registry),
        diagnostics: Some(&mut sink),
        cross_file_links: None,
    };

    let mut plugin = LinkValidationPlugin::new(LinkValidationConfig::default());
    plugin.visit_with_context(&mut tree, &mut ctx);

    let diags = sink.take();
    assert!(
        diags.is_empty(),
        "bare fragment in a never-tracked file must not emit diagnostic: {diags:?}"
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
    let tmpdir = tempfile::Builder::new()
        .prefix("zfb-link-val-test")
        .tempdir()
        .expect("tempdir");
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

// ── Fixture 17: bare anchor to explicit element id (#1095) ───────────────────

/// In a headingless file, `[x](#foo)` where the hast tree contains an element
/// with `id="foo"` (e.g. a `<div id="foo">` produced by a MDX JSX element)
/// must NOT emit a `BrokenLink` diagnostic.
///
/// The pipeline passes through `HeadingLinksPlugin` (which records the id in
/// `anchor_ids`) before `LinkValidationPlugin` runs.
#[test]
fn bare_anchor_to_explicit_element_id_no_diagnostic() {
    use zfb_content::pipeline::{BuildContext, HastNode, HastVisitor};
    use zfb_content::plugins::heading_links::HeadingLinksPlugin;
    use zfb_md_extras::link_validation::LinkValidationPlugin;

    let source = PathBuf::from("/project/docs/headingless.md");
    let mut registry = HeadingRegistry::new();
    let mut sink = CollectingSink::new();

    // Hast tree: <div id="foo">...</div> + <a href="#foo">link</a>
    // The <div> is a non-heading element with an explicit id — simulates
    // a MDX component or custom element that renders with an id attribute.
    let mut tree = HastNode::Root {
        children: vec![
            HastNode::Element {
                tag: "div".to_string(),
                attrs: vec![("id".to_string(), "foo".to_string())],
                children: vec![HastNode::Text("content".to_string())],
                void: false,
            },
            HastNode::Element {
                tag: "a".to_string(),
                attrs: vec![("href".to_string(), "#foo".to_string())],
                children: vec![HastNode::Text("link".to_string())],
                void: false,
            },
        ],
    };

    let mut ctx = BuildContext {
        source_path: Some(source.clone()),
        project_root: PathBuf::from("/project"),
        public_dir: PathBuf::from("/project/public"),
        heading_registry: Some(&mut registry),
        diagnostics: Some(&mut sink),
        cross_file_links: None,
    };

    // Run HeadingLinksPlugin first (records anchor ids into the registry).
    HeadingLinksPlugin::new().visit_with_context(&mut tree, &mut ctx);
    // Then run LinkValidationPlugin (consults the registry).
    let mut plugin = LinkValidationPlugin::new(LinkValidationConfig::default());
    plugin.visit_with_context(&mut tree, &mut ctx);

    let diags = sink.take();
    assert!(
        diags.is_empty(),
        "bare anchor to explicit element id must not emit BrokenLink: {diags:?}"
    );
}

// ── Fixture 18: genuinely broken bare anchor in headingless file (#1095) ─────

/// A headingless file with `[x](#nonexistent)` where NO element has `id="nonexistent"`
/// must STILL emit `BrokenLink` — the fix must not suppress valid diagnostics.
#[test]
fn bare_broken_anchor_in_headingless_file_emits_diagnostic() {
    use zfb_content::pipeline::{BuildContext, HastNode, HastVisitor};
    use zfb_content::plugins::heading_links::HeadingLinksPlugin;
    use zfb_md_extras::link_validation::LinkValidationPlugin;

    let source = PathBuf::from("/project/docs/headingless.md");
    let mut registry = HeadingRegistry::new();
    let mut sink = CollectingSink::new();

    // The file has a div with a DIFFERENT id, plus a broken anchor.
    let mut tree = HastNode::Root {
        children: vec![
            HastNode::Element {
                tag: "div".to_string(),
                attrs: vec![("id".to_string(), "other".to_string())],
                children: vec![],
                void: false,
            },
            HastNode::Element {
                tag: "a".to_string(),
                attrs: vec![("href".to_string(), "#nonexistent".to_string())],
                children: vec![HastNode::Text("bad link".to_string())],
                void: false,
            },
        ],
    };

    let mut ctx = BuildContext {
        source_path: Some(source.clone()),
        project_root: PathBuf::from("/project"),
        public_dir: PathBuf::from("/project/public"),
        heading_registry: Some(&mut registry),
        diagnostics: Some(&mut sink),
        cross_file_links: None,
    };

    HeadingLinksPlugin::new().visit_with_context(&mut tree, &mut ctx);
    let mut plugin = LinkValidationPlugin::new(LinkValidationConfig::default());
    plugin.visit_with_context(&mut tree, &mut ctx);

    let diags = sink.take();
    assert_eq!(
        diags.len(),
        1,
        "genuinely broken bare anchor must still emit BrokenLink: {diags:?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "#nonexistent"),
        "diagnostic url must be the raw href: {diags:?}"
    );
}

// ── Fixture 19: bare anchor to `<a name="…">` (#1095) ────────────────────────

/// An `<a name="section">` legacy named anchor in a headingless file:
/// `[x](#section)` must NOT emit a `BrokenLink`.
#[test]
fn bare_anchor_to_a_name_no_diagnostic() {
    use zfb_content::pipeline::{BuildContext, HastNode, HastVisitor};
    use zfb_content::plugins::heading_links::HeadingLinksPlugin;
    use zfb_md_extras::link_validation::LinkValidationPlugin;

    let source = PathBuf::from("/project/docs/headingless.md");
    let mut registry = HeadingRegistry::new();
    let mut sink = CollectingSink::new();

    // `<a name="section">` followed by a link to `#section`.
    let mut tree = HastNode::Root {
        children: vec![
            HastNode::Element {
                tag: "a".to_string(),
                attrs: vec![("name".to_string(), "section".to_string())],
                children: vec![],
                void: false,
            },
            HastNode::Element {
                tag: "a".to_string(),
                attrs: vec![("href".to_string(), "#section".to_string())],
                children: vec![HastNode::Text("link".to_string())],
                void: false,
            },
        ],
    };

    let mut ctx = BuildContext {
        source_path: Some(source.clone()),
        project_root: PathBuf::from("/project"),
        public_dir: PathBuf::from("/project/public"),
        heading_registry: Some(&mut registry),
        diagnostics: Some(&mut sink),
        cross_file_links: None,
    };

    HeadingLinksPlugin::new().visit_with_context(&mut tree, &mut ctx);
    let mut plugin = LinkValidationPlugin::new(LinkValidationConfig::default());
    plugin.visit_with_context(&mut tree, &mut ctx);

    let diags = sink.take();
    assert!(
        diags.is_empty(),
        "bare anchor to <a name> must not emit BrokenLink: {diags:?}"
    );
}

// ── Fixture 20: heading-bearing file unaffected (#1095) ──────────────────────

/// A file WITH headings plus an explicit element id: both kinds of anchor
/// should work, and broken ones should still fail. Ensures the #1095 fix
/// does not alter behaviour for heading-bearing files.
#[test]
fn heading_file_with_element_id_both_accepted() {
    let source = PathBuf::from("/project/docs/page.md");
    let mut registry = HeadingRegistry::new();
    // Pre-populate a heading entry for the source file.
    registry.insert(
        source.clone(),
        HeadingEntry {
            id: "introduction".to_string(),
            text: "Introduction".to_string(),
            depth: 2,
        },
    );
    // Also record an explicit anchor id (as HeadingLinksPlugin would).
    registry.insert_anchor_id(source.clone(), "custom-section".to_string());

    // Both `#introduction` (heading) and `#custom-section` (explicit id) must pass.
    let md = "[a](#introduction)\n\n[b](#custom-section)\n\n[c](#broken)\n";
    let diags = run(
        md,
        source,
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert_eq!(
        diags.len(),
        1,
        "only the genuinely broken anchor must produce a diagnostic: {diags:?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "#broken"),
        "diagnostic url must be #broken: {diags:?}"
    );
}

// ── Fixture 21: cross-file link to explicit element id (#1095) ───────────────

/// `[x](./other.md#foo)` where `other.md` has `<div id="foo">` but NO headings
/// must NOT emit `BrokenLink` — the fix must also cover cross-file fragment
/// validation, not just bare same-file fragments.
#[test]
fn cross_file_link_to_explicit_element_id_no_diagnostic() {
    let tmpdir = tempfile::Builder::new()
        .prefix("zfb-link-val-test")
        .tempdir()
        .expect("tempdir");
    let project_root = tmpdir.path().to_path_buf();
    let source_path = tmpdir.path().join("page.md");
    let other_path = tmpdir.path().join("other.md");
    std::fs::write(&source_path, "").expect("write page.md");
    std::fs::write(&other_path, "no headings here\n").expect("write other.md");

    let mut registry = HeadingRegistry::new();
    // Pre-populate the target file's anchor id (as HeadingLinksPlugin would
    // have done when processing other.md with a <div id="foo"> element).
    registry.mark_tracked(other_path.clone());
    registry.insert_anchor_id(other_path.clone(), "foo".to_string());

    let md = "[link](./other.md#foo)\n";
    let diags = run(
        md,
        source_path,
        project_root,
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert!(
        diags.is_empty(),
        "cross-file link to explicit element id must not emit BrokenLink: {diags:?}"
    );
}

/// Verify the regression contract: a cross-file link to a genuinely absent
/// anchor in a headingless file must still emit `BrokenLink`.
#[test]
fn cross_file_link_broken_anchor_in_headingless_file_emits_diagnostic() {
    let tmpdir = tempfile::Builder::new()
        .prefix("zfb-link-val-test")
        .tempdir()
        .expect("tempdir");
    let project_root = tmpdir.path().to_path_buf();
    let source_path = tmpdir.path().join("page.md");
    let other_path = tmpdir.path().join("other.md");
    std::fs::write(&source_path, "").expect("write page.md");
    std::fs::write(&other_path, "no headings here\n").expect("write other.md");

    let mut registry = HeadingRegistry::new();
    // Mark other.md as tracked with only one anchor id (not "missing").
    registry.mark_tracked(other_path.clone());
    registry.insert_anchor_id(other_path.clone(), "present".to_string());

    let md = "[link](./other.md#missing)\n";
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
        "cross-file link to absent anchor must emit BrokenLink: {diags:?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "./other.md#missing"),
        "diagnostic url must be the raw href: {diags:?}"
    );
}

// ── Fixture 12: bare-fragment matching is exact-only (ASCII + CJK) ───────────
//
// zfb#2116 (Link Gating epic #2112): investigated an UNCONFIRMED observation
// of an EN/JA broken-link warning asymmetry on "leaf-slug" bare-anchor
// shorthand (a link using only the trailing segment of a hierarchical
// heading id, e.g. `#child-heading` when the actual id is
// `parent-heading-child-heading`). `validate_fragment_in_file`'s match rule
// (`entries.iter().any(|e| e.id == fragment)`) is plain string equality with
// no suffix/leaf fallback — this pin proves that holds identically for a
// CJK heading id, which had no test coverage in this suite before this
// fixture. The suite's exhaustive per-fixture-token grep already showed no
// asymmetry in the matching code path itself; this pins the finding.

/// `[foo](#leaf-heading)` where the only heading id is the hierarchical
/// `parent-heading-leaf-heading` → still a broken-link warning (no suffix
/// fallback). ASCII control for the CJK case below.
#[test]
fn bare_fragment_ascii_leaf_slug_shorthand_does_not_match_hierarchical_heading() {
    let source = PathBuf::from("/project/docs/page.md");
    let mut registry = HeadingRegistry::new();
    registry.insert(
        source.clone(),
        HeadingEntry {
            id: "parent-heading-leaf-heading".to_string(),
            text: "Leaf Heading".to_string(),
            depth: 3,
        },
    );
    let md = "[foo](#leaf-heading)\n";
    let diags = run(
        md,
        source,
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert_eq!(
        diags.len(),
        1,
        "leaf-slug shorthand must not match the hierarchical id: {diags:?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "#leaf-heading"),
        "diagnostic url must be the raw href: {diags:?}"
    );
}

/// CJK counterpart: `[foo](#子見出し)` where the only registered heading id
/// is the hierarchical `親見出し-子見出し` → still a broken-link warning.
/// This is the first CJK fixture in this suite (confirmed absent by grep
/// before this test was added) and rules out any CJK-specific suffix
/// fallback as the source of the #2116 asymmetry.
#[test]
fn bare_fragment_cjk_leaf_slug_shorthand_does_not_match_hierarchical_heading() {
    let source = PathBuf::from("/project/docs/page.md");
    let mut registry = HeadingRegistry::new();
    registry.insert(
        source.clone(),
        HeadingEntry {
            id: "親見出し-子見出し".to_string(),
            text: "子見出し".to_string(),
            depth: 3,
        },
    );
    let md = "[foo](#子見出し)\n";
    let diags = run(
        md,
        source,
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert_eq!(
        diags.len(),
        1,
        "CJK leaf-slug shorthand must not match the hierarchical id: {diags:?}"
    );
    assert!(
        matches!(&diags[0], MarkdownDiagnostic::BrokenLink { url, .. } if url == "#子見出し"),
        "diagnostic url must be the raw href: {diags:?}"
    );
}

/// Control: the CJK hierarchical id matched EXACTLY (not via a leaf-slug
/// shorthand) must still resolve with no diagnostic, proving the exact-match
/// path itself is CJK-safe and the two tests above are pinning the absence
/// of a suffix fallback, not a broader CJK matching bug.
#[test]
fn bare_fragment_cjk_exact_hierarchical_id_matches_no_diagnostic() {
    let source = PathBuf::from("/project/docs/page.md");
    let mut registry = HeadingRegistry::new();
    registry.insert(
        source.clone(),
        HeadingEntry {
            id: "親見出し-子見出し".to_string(),
            text: "子見出し".to_string(),
            depth: 3,
        },
    );
    let md = "[foo](#親見出し-子見出し)\n";
    let diags = run(
        md,
        source,
        PathBuf::from("/project"),
        &mut registry,
        LinkValidationConfig::default(),
    );
    assert!(
        diags.is_empty(),
        "exact CJK heading id match must not emit a diagnostic: {diags:?}"
    );
}
