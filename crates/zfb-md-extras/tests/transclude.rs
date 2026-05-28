//! Fixture-based integration tests for the transclude feature.
//!
//! Each fixture directory under `tests/fixtures/transclude/` is a self-contained
//! test case. Success fixtures have `input.md`, `expected.html`, and any
//! additional snippet/source files referenced by `:::include` directives.
//! Error fixtures have `input.md`, optional snippet files, and
//! `expected-error.txt` containing a fragment of the expected error message.
//!
//! # Custom runner
//!
//! Unlike other features, the transclude tests cannot use `run_fixture`
//! directly because transclusion requires a `BuildContext` (source_path +
//! project_root). The custom `run_transclude_fixture` helper wires a
//! `Pipeline::run_with_context` call with the fixture directory as both the
//! source root and project root.

use std::path::Path;

use zfb_content::pipeline::Pipeline;
use zfb_content::serializer::serialize;
use zfb_md_ast::{diagnostics::CollectingSink, BuildContext, TranscludeConfig};
use zfb_md_extras::MarkdownFeaturesConfig;
use zfb_test_utils::normalize_html;

// ── Custom runner ─────────────────────────────────────────────────────────────

/// Build a pipeline with `transclude` enabled using the given config.
fn pipeline_with_transclude(config: TranscludeConfig) -> Pipeline {
    let features = MarkdownFeaturesConfig {
        transclude: Some(config),
        ..Default::default()
    };
    Pipeline::with_defaults_and_features(&features)
}

/// Run a success fixture: expects `input.md` + `expected.html` in `dir`.
///
/// The fixture directory is used as both the source file directory and the
/// project root. `input.md` is treated as the "current file" so that
/// `:::include{file="./snippet.md"}` resolves relative to it.
fn run_success_fixture(name: &str) {
    let dir = format!(
        "{}/tests/fixtures/transclude/{}",
        env!("CARGO_MANIFEST_DIR"),
        name
    );
    let dir = Path::new(&dir);

    let input_path = dir.join("input.md");
    let expected_path = dir.join("expected.html");
    let source_path = input_path.clone();
    let project_root = dir.to_path_buf();

    let input = std::fs::read_to_string(&input_path).unwrap_or_else(|e| {
        panic!("failed to read {}: {e}", input_path.display())
    });
    let expected_raw = std::fs::read_to_string(&expected_path).unwrap_or_else(|e| {
        panic!("failed to read {}: {e}", expected_path.display())
    });

    let mut sink = CollectingSink::new();
    let mut ctx = BuildContext {
        source_path: Some(source_path),
        project_root,
        public_dir: dir.to_path_buf(),
        heading_registry: None,
        diagnostics: Some(&mut sink),
    };

    let mut pipeline = pipeline_with_transclude(TranscludeConfig::default());
    let hast = pipeline
        .run_with_context(&input, &mut ctx)
        .expect("pipeline failed");
    let actual_raw = serialize(&hast);

    let diags = sink.take();
    if !diags.is_empty() {
        panic!(
            "fixture '{}': unexpected diagnostics: {diags:?}",
            name
        );
    }

    let actual = normalize_html(&actual_raw);
    let expected = normalize_html(&expected_raw);
    assert_eq!(
        actual, expected,
        "fixture '{name}': HTML mismatch\n  actual:   {actual}\n  expected: {expected}"
    );
}

/// Run an error fixture: expects `input.md` + `expected-error.txt` in `dir`.
///
/// The test verifies that at least one Error-severity diagnostic is emitted
/// whose message contains the text in `expected-error.txt`.
fn run_error_fixture(name: &str, config: TranscludeConfig) {
    let dir = format!(
        "{}/tests/fixtures/transclude/{}",
        env!("CARGO_MANIFEST_DIR"),
        name
    );
    let dir = Path::new(&dir);

    let input_path = dir.join("input.md");
    let expected_error_path = dir.join("expected-error.txt");
    let source_path = input_path.clone();
    let project_root = dir.to_path_buf();

    let input = std::fs::read_to_string(&input_path).unwrap_or_else(|e| {
        panic!("failed to read {}: {e}", input_path.display())
    });
    let expected_error = std::fs::read_to_string(&expected_error_path).unwrap_or_else(|e| {
        panic!("failed to read {}: {e}", expected_error_path.display())
    });
    let expected_fragment = expected_error.trim();

    let mut sink = CollectingSink::new();
    let mut ctx = BuildContext {
        source_path: Some(source_path),
        project_root,
        public_dir: dir.to_path_buf(),
        heading_registry: None,
        diagnostics: Some(&mut sink),
    };

    let mut pipeline = pipeline_with_transclude(config);
    let _ = pipeline.run_with_context(&input, &mut ctx);

    let diags = sink.take();
    let has_matching_error = diags.iter().any(|d| {
        use zfb_md_ast::diagnostics::{DiagnosticSeverity, MarkdownDiagnostic};
        matches!(d, MarkdownDiagnostic::Generic { severity: DiagnosticSeverity::Error, message, .. } if message.contains(expected_fragment))
    });
    assert!(
        has_matching_error,
        "fixture '{name}': expected error containing \"{expected_fragment}\", got: {diags:?}"
    );
}

// ── Success fixtures ─────────────────────────────────────────────────────────

/// A simple single-level include: document + snippet.md.
#[test]
fn basic_include() {
    run_success_fixture("basic");
}

/// Two-level include chain: input.md → a.md → b.md.
#[test]
fn chain_include() {
    run_success_fixture("chain");
}

// ── Error fixtures ────────────────────────────────────────────────────────────

/// Cycle A→B→A: expect a "cycle detected" error diagnostic.
#[test]
fn cycle_detection() {
    run_error_fixture("cycle", TranscludeConfig::default());
}

/// maxDepth: 1 — a 2-level chain is rejected with a "max depth" error.
#[test]
fn max_depth_one_rejects_chain() {
    run_error_fixture("max-depth-1", TranscludeConfig { max_depth: 1 });
}

// ── Inline smoke tests (without fixtures) ────────────────────────────────────

/// `code=true lang="rust"` wraps content in a `<pre><code class="language-rust">` block.
#[test]
fn code_block_wraps_in_pre() {
    let dir_path = format!(
        "{}/tests/fixtures/transclude/code-block",
        env!("CARGO_MANIFEST_DIR")
    );
    let dir = Path::new(&dir_path);
    let input_path = dir.join("input.md");
    let source_path = input_path.clone();
    let project_root = dir.to_path_buf();

    let input = std::fs::read_to_string(&input_path).expect("read input.md");

    let mut sink = CollectingSink::new();
    let mut ctx = BuildContext {
        source_path: Some(source_path),
        project_root,
        public_dir: dir.to_path_buf(),
        heading_registry: None,
        diagnostics: Some(&mut sink),
    };

    let mut pipeline = pipeline_with_transclude(TranscludeConfig::default());
    let hast = pipeline.run_with_context(&input, &mut ctx).expect("pipeline ok");
    let html = serialize(&hast);

    // The pipeline runs syntect, so the code block appears as highlighted HTML
    // with span elements wrapping each token. Check for identifier presence.
    assert!(
        html.contains("main"),
        "expected code content identifier 'main', got: {html}"
    );
    assert!(
        html.contains("println"),
        "expected code content identifier 'println', got: {html}"
    );
    let diags = sink.take();
    assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
}

/// `lines="2-3"` slices only the specified lines before wrapping in code.
#[test]
fn lines_range_slices_content() {
    let dir_path = format!(
        "{}/tests/fixtures/transclude/lines",
        env!("CARGO_MANIFEST_DIR")
    );
    let dir = Path::new(&dir_path);
    let input_path = dir.join("input.md");
    let source_path = input_path.clone();
    let project_root = dir.to_path_buf();

    let input = std::fs::read_to_string(&input_path).expect("read input.md");

    let mut sink = CollectingSink::new();
    let mut ctx = BuildContext {
        source_path: Some(source_path),
        project_root,
        public_dir: dir.to_path_buf(),
        heading_registry: None,
        diagnostics: Some(&mut sink),
    };

    let mut pipeline = pipeline_with_transclude(TranscludeConfig::default());
    let hast = pipeline.run_with_context(&input, &mut ctx).expect("pipeline ok");
    let html = serialize(&hast);

    // Lines 2-3 of data.rs: "fn second() {}" and "fn third() {}"
    // Syntect adds spans so we check just the identifiers, not the full pattern.
    assert!(html.contains("second"), "expected line 2 identifier 'second': {html}");
    assert!(html.contains("third"), "expected line 3 identifier 'third': {html}");
    // Line 1 and 4 should NOT appear
    assert!(!html.contains("first"), "line 1 should be excluded: {html}");
    assert!(!html.contains("fourth"), "line 4 should be excluded: {html}");

    let diags = sink.take();
    assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
}
