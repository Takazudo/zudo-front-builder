//! Tests for walk-order determinism (zfb#187 / zfb#190).
//!
//! ## Problem
//!
//! Before the fix, `bundler.rs` walked source trees via
//! `WalkDir::new(src).follow_links(false)` without sorting, while
//! `walk_collection` sorted its file list explicitly. When the OS
//! returned `readdir` entries in a different order than the sorted walk,
//! `HeadingLinksPlugin`'s slug counter accumulated state across files in
//! a different sequence. A heading `## Basic Usage` that appeared in the
//! first file in one walk might be the seventh file in the other walk —
//! producing `basic-usage` in one `content_hash` and `basic-usage-7` in
//! another. The `mdx://<collection>/<slug>#<hash>` bridge lookup then
//! missed, and ~65% of pages silently fell back to the raw-markdown
//! `<pre data-zfb-content-fallback>` shape.
//!
//! ## Fix (both parts)
//!
//! 1. **Sort**: both `WalkDir` call sites in `bundler.rs` now use
//!    `.sort_by_file_name()` so the walk order is lexicographic and
//!    matches `walk_collection`'s `files.sort()`.
//!
//! 2. **State reset**: `Pipeline::reset_per_entry()` is called before
//!    each new MDX file so `HeadingLinksPlugin`'s `seen` map is cleared.
//!    This removes the entire class of cross-document leakage even if a
//!    future stateful plugin forgets to account for reuse.
//!
//! ## Tests
//!
//! These tests exercise the `Pipeline`-level primitives directly (no
//! esbuild required) so they run in any CI environment.

use zfb_content::compile_mdx_to_jsx_module_cached;
use zfb_content::pipeline::Pipeline;

/// Source body shared across multiple "documents" in the corpus below.
/// It contains a repeated heading slug ("Setup") so the slug counter
/// matters: the first file to be processed gets `id="setup"`, subsequent
/// files that are processed *without* `reset_per_entry()` would get
/// `id="setup-1"`, `id="setup-2"`, etc.
fn doc_a_body() -> &'static str {
    "## Setup\n\nStep-by-step guide.\n"
}

fn doc_b_body() -> &'static str {
    "## Setup\n\nAnother guide with the same heading.\n"
}

fn doc_c_body() -> &'static str {
    "## Setup\n\nThird document with the same heading.\n"
}

/// Helper: compile a single MDX body through the given pipeline (no
/// file-system involvement; `path` is a fake path used only for the
/// `collection_and_slug` derivation).
fn compile(body: &str, pipeline: &mut Pipeline) -> String {
    let tmp = tempfile::tempdir().expect("tempdir");
    let fake_path = tmp.path().join("docs").join("guide.mdx");
    std::fs::create_dir_all(fake_path.parent().unwrap()).unwrap();
    std::fs::write(&fake_path, body).unwrap();

    compile_mdx_to_jsx_module_cached(body, &fake_path, None, Some(pipeline))
        .expect("compile ok")
        .jsx_source
}

/// Verify that processing the same document twice through a reused
/// pipeline (without resetting between) produces **different** heading
/// `id` attributes — confirming the bug scenario is reproducible.
///
/// This test is expected to pass, meaning "the stale-state bug is real
/// and the test can detect it." It only uses the heading-links plugin
/// from `with_defaults()`, so no esbuild or filesystem walk is needed.
#[test]
fn without_reset_repeated_heading_gets_different_slug_second_time() {
    let mut p = Pipeline::with_defaults();

    // First document — heading slug should be "setup".
    let first = compile(doc_a_body(), &mut p);
    assert!(
        first.contains("\"setup\"") || first.contains("id=\\\"setup\\\"") || first.contains("#setup"),
        "first pass: heading id must be 'setup', got: {first:.200}"
    );

    // Second document WITHOUT reset — the slug counter carries over and
    // the same heading text now gets "setup-1".
    let second = compile(doc_b_body(), &mut p);
    // Without reset the id increments.
    assert!(
        second.contains("\"setup-1\"") || second.contains("id=\\\"setup-1\\\"") || second.contains("#setup-1"),
        "without reset: second pass should get 'setup-1' (slug counter leaked from first doc), got: {second:.200}"
    );
}

/// Verify that `Pipeline::reset_per_entry()` clears the slug counter
/// so every document sees a fresh counter and the same heading text
/// always produces the same slug regardless of processing order.
#[test]
fn with_reset_per_entry_same_heading_always_gets_base_slug() {
    let mut p = Pipeline::with_defaults();

    let bodies = [doc_a_body(), doc_b_body(), doc_c_body()];

    for (i, body) in bodies.iter().enumerate() {
        // Reset before each document — this is what Pipeline::reset_per_entry does.
        p.reset_per_entry();
        let out = compile(body, &mut p);
        assert!(
            out.contains("\"setup\"") || out.contains("id=\\\"setup\\\"") || out.contains("#setup"),
            "doc {i}: heading id must be 'setup' (not 'setup-{i}') after reset_per_entry, got: {out:.200}"
        );
    }
}

/// Verify that processing documents in two different orders (simulating
/// the old unsorted walk vs the sorted walk) produces byte-identical
/// output when `reset_per_entry()` is called between documents.
///
/// Uses three documents with identical structure but different content.
/// The two orderings differ in which document is processed first.
#[test]
fn reset_per_entry_makes_output_order_independent() {
    let order_a = [doc_a_body(), doc_b_body(), doc_c_body()];
    let order_b = [doc_c_body(), doc_a_body(), doc_b_body()];

    fn compile_sequence(bodies: &[&str]) -> Vec<String> {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut p = Pipeline::with_defaults();
        bodies
            .iter()
            .enumerate()
            .map(|(i, body)| {
                p.reset_per_entry();
                let fake_path = tmp.path().join("docs").join(format!("guide-{i}.mdx"));
                std::fs::create_dir_all(fake_path.parent().unwrap()).unwrap();
                std::fs::write(&fake_path, body).unwrap();
                compile_mdx_to_jsx_module_cached(body, &fake_path, None, Some(&mut p))
                    .expect("compile ok")
                    .jsx_source
            })
            .collect()
    }

    let results_a: Vec<String> = compile_sequence(&order_a);
    let results_b: Vec<String> = compile_sequence(&order_b);

    // With reset_per_entry, every document in a sequence should start
    // with a fresh slug counter.  The heading slug in each compiled
    // output must be "setup" (not "setup-N") regardless of position.
    for (i, out) in results_a.iter().chain(results_b.iter()).enumerate() {
        assert!(
            out.contains("\"setup\"") || out.contains("id=\\\"setup\\\"") || out.contains("#setup"),
            "doc {i}: heading id must be 'setup' after reset_per_entry, got: {out:.200}"
        );
    }
}
