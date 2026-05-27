//! Fixture-based snapshot test harness for `zfb-md-extras`.
//!
//! # Fixture directory layout
//!
//! ```text
//! crates/zfb-md-extras/tests/fixtures/<feature-name>/
//! ├── input.md          — Markdown source
//! ├── expected.html     — Expected HTML output (committed, generated once
//! │                       with scripts/gen-fixture.mjs)
//! └── normalize.txt     — Optional. If present and contains "verbatim" on
//!                         the first non-blank line, both sides are compared
//!                         as raw strings (no normalize_html pass).
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! // In a test file for a specific feature:
//! use zfb_md_extras::test_harness::run_fixture;
//!
//! #[test]
//! fn test_my_feature() {
//!     let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/my-feature");
//!     run_fixture(fixtures, |input| markdown::to_html(input));
//! }
//! ```
//!
//! # Wave 2 note
//!
//! The `transform` closure currently accepts a `&str` and returns `String`.
//! Wave 2 (#569) will introduce the real `Pipeline` type; callers will replace
//! the trivial closure with one that constructs a `Pipeline::new().with_mdx()`
//! chain. The closure signature is intentionally kept simple here so Wave 2
//! can refine it without touching this module.
//!
//! # Gate
//!
//! This module is gated behind `#[cfg(any(test, feature = "test-utils"))]` in
//! `lib.rs` — it is never compiled into production builds.

use std::path::Path;

use zfb_test_utils::normalize_html;

/// Run a single fixture directory against the given transform closure.
///
/// `fixture_dir` must contain `input.md` and `expected.html`. An optional
/// `normalize.txt` file with the single word `verbatim` on its first non-blank
/// line disables the `normalize_html` pass and compares raw strings.
///
/// # Panics
///
/// Panics on I/O errors or if the actual output diverges from the expected
/// HTML.
///
/// # Wave 2 placeholder
///
/// The `transform` parameter is a `FnOnce(&str) -> String` closure so that
/// the test infrastructure can land before the real `Pipeline` type exists.
/// Wave 2 (#569) will pass a closure that routes input through the actual
/// `Pipeline::new().with_mdx()` + visitor chain.
pub fn run_fixture<F>(fixture_dir: impl AsRef<Path>, transform: F)
where
    F: FnOnce(&str) -> String,
{
    let dir = fixture_dir.as_ref();

    let input_path = dir.join("input.md");
    let expected_path = dir.join("expected.html");

    let input = std::fs::read_to_string(&input_path).unwrap_or_else(|e| {
        panic!("failed to read {}: {e}", input_path.display())
    });

    let expected_raw = std::fs::read_to_string(&expected_path).unwrap_or_else(|e| {
        panic!("failed to read {}: {e}", expected_path.display())
    });

    let verbatim = is_verbatim(dir);

    if verbatim {
        // Verbatim mode: pass the raw input to the transform (no trimming) and
        // compare the raw output string. This preserves intentional leading/
        // trailing whitespace that callers may rely on.
        let actual_raw = transform(&input);
        assert_eq!(
            actual_raw, expected_raw,
            "fixture {}: verbatim output mismatch",
            dir.display()
        );
    } else {
        // Normalizing mode: pass the Markdown input through unchanged —
        // the fixture's `input.md` is the parser's source of truth, and
        // trimming it can flip parsing for indented code blocks, fenced
        // blocks with leading blank lines, and other constructs that
        // depend on document boundaries. Normalization applies only to
        // the HTML *output* on both sides.
        let actual_raw = transform(&input);
        let actual = normalize_html(&actual_raw);
        let expected = normalize_html(&expected_raw);
        assert_eq!(
            actual, expected,
            "fixture {}: normalized HTML mismatch\n  actual:   {actual}\n  expected: {expected}",
            dir.display()
        );
    }
}

/// Returns `true` if `<fixture_dir>/normalize.txt` exists and its first
/// non-blank line is `verbatim`.
fn is_verbatim(dir: &Path) -> bool {
    let path = dir.join("normalize.txt");
    match std::fs::read_to_string(path) {
        Ok(contents) => contents
            .lines()
            .find(|l| !l.trim().is_empty())
            .map(|l| l.trim() == "verbatim")
            .unwrap_or(false),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Helper: create a temporary fixture directory with given files.
    fn make_fixture_dir(
        files: &[(&str, &str)],
    ) -> tempdir::TempDir {
        let dir = tempdir::TempDir::new("zfb_fixture_test").expect("tempdir");
        for (name, content) in files {
            let path = dir.path().join(name);
            std::fs::write(&path, content).expect("write fixture file");
        }
        dir
    }

    // ── run_fixture: match path ───────────────────────────────────────────

    #[test]
    fn test_run_fixture_pass_on_match() {
        // A trivial transform: just pass through the input unchanged.
        // Input and expected are identical so the test must pass.
        let dir = make_fixture_dir(&[
            ("input.md", "hello"),
            ("expected.html", "<p>hello</p>"),
        ]);
        // Use markdown::to_html as the transform — produces <p>hello</p>.
        run_fixture(dir.path(), |input| markdown::to_html(input));
    }

    // ── run_fixture: divergence path ─────────────────────────────────────

    #[test]
    fn test_run_fixture_fail_on_divergence() {
        // run_fixture must panic when the actual output diverges from expected.
        let dir = make_fixture_dir(&[
            ("input.md", "hello"),
            ("expected.html", "<p>completely-different</p>"),
        ]);
        let result = std::panic::catch_unwind(|| {
            run_fixture(dir.path(), |input| markdown::to_html(input));
        });
        assert!(result.is_err(), "run_fixture must panic on divergence");
    }

    // ── verbatim opt-out ─────────────────────────────────────────────────

    #[test]
    fn test_verbatim_opt_out_passes_when_equal() {
        // With normalize.txt=verbatim, comparison is done on raw strings.
        // Extra whitespace that normalize_html would collapse is preserved.
        let dir = make_fixture_dir(&[
            ("input.md", "hello"),
            // Both sides must be equal raw strings for verbatim mode.
            ("expected.html", "hello"),
            ("normalize.txt", "verbatim\n"),
        ]);
        run_fixture(dir.path(), |_| "hello".to_string());
    }

    #[test]
    fn test_verbatim_opt_out_fails_when_not_equal() {
        // Verbatim mode: even a trivial normalization difference causes failure.
        let dir = make_fixture_dir(&[
            ("input.md", "hello"),
            ("expected.html", "<p>hello</p>"),
            ("normalize.txt", "verbatim\n"),
        ]);
        let result = std::panic::catch_unwind(|| {
            // Transform returns a string that differs from expected.html.
            // Verbatim mode: raw strings are compared, so this must fail.
            run_fixture(dir.path(), |_| "different output".to_string());
        });
        assert!(result.is_err(), "verbatim mode must panic on string mismatch");
    }

    #[test]
    fn test_no_normalize_txt_uses_normalization() {
        // Without normalize.txt the comparison goes through normalize_html.
        // `<br>` and `<br />` are distinct raw strings but equal after normalization.
        let dir = make_fixture_dir(&[
            ("input.md", "test"),
            ("expected.html", "<p>a<br>b</p>"),
        ]);
        // Transform returns the XHTML form; normalize_html makes them equal.
        run_fixture(dir.path(), |_| "<p>a<br />b</p>".to_string());
    }

    // ── is_verbatim ───────────────────────────────────────────────────────

    #[test]
    fn test_is_verbatim_true_when_file_says_verbatim() {
        let dir = make_fixture_dir(&[("normalize.txt", "verbatim\n")]);
        assert!(is_verbatim(dir.path()));
    }

    #[test]
    fn test_is_verbatim_false_when_no_file() {
        let dir = make_fixture_dir(&[]);
        assert!(!is_verbatim(dir.path()));
    }

    #[test]
    fn test_is_verbatim_false_when_file_has_other_content() {
        let dir = make_fixture_dir(&[("normalize.txt", "normalize\n")]);
        assert!(!is_verbatim(dir.path()));
    }

    #[test]
    fn test_is_verbatim_true_with_leading_blank_lines() {
        let dir = make_fixture_dir(&[("normalize.txt", "\n\nverbatim\n")]);
        assert!(is_verbatim(dir.path()));
    }
}
