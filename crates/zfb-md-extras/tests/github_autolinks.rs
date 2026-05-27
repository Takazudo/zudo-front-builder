//! Fixture-based integration tests for the GitHub-autolinks feature.
//!
//! Each subdirectory under `tests/fixtures/github_autolinks/` is a self-contained
//! fixture: `input.md` holds the Markdown source and `expected.html` holds the
//! expected HTML output after running through a fully-wired pipeline with
//! `githubAutolinks: { repo: "owner/myrepo" }`.

use zfb_content::pipeline::Pipeline;
use zfb_content::serializer::serialize;
use zfb_md_extras::{test_harness::run_fixture, GithubAutolinksConfig, MarkdownFeaturesConfig};

/// The repository used for all autolink URL expansion in these fixtures.
const TEST_REPO: &str = "owner/myrepo";

/// Build a pipeline with `githubAutolinks` set to the given repo.
fn pipeline_with_autolinks(repo: &str) -> Pipeline {
    let features = MarkdownFeaturesConfig {
        github_autolinks: Some(GithubAutolinksConfig {
            repo: Some(repo.to_string()),
        }),
        ..Default::default()
    };
    Pipeline::with_defaults_and_features(&features)
}

/// Build a pipeline with `githubAutolinks` disabled (no config).
fn pipeline_without_autolinks() -> Pipeline {
    Pipeline::with_defaults_and_features(&MarkdownFeaturesConfig::default())
}

/// Run a fixture directory against the github_autolinks-enabled pipeline.
fn run(name: &str) {
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/github_autolinks/"
    )
    .to_string()
        + name;
    run_fixture(&dir, |input| {
        let mut p = pipeline_with_autolinks(TEST_REPO);
        let hast = p.run(input).expect("pipeline failed");
        serialize(&hast)
    });
}

// ── Fixture tests ─────────────────────────────────────────────────────────────

/// Bare `#123` → `<a href="https://github.com/owner/myrepo/issues/123">#123</a>`.
#[test]
fn bare_issue() {
    run("bare-issue");
}

/// `user/repo#456` → `<a href="https://github.com/user/repo/issues/456">user/repo#456</a>`.
#[test]
fn cross_repo_issue() {
    run("cross-repo-issue");
}

/// 7-char hex SHA → `<a href="https://github.com/owner/myrepo/commit/abc1234">abc1234</a>`.
#[test]
fn commit_sha() {
    run("commit-sha");
}

/// `#123` inside a code span must NOT be rewritten.
#[test]
fn in_code_span() {
    run("in-code-span");
}

/// References inside a fenced code block must NOT be rewritten.
#[test]
fn in_code_block() {
    run("in-code-block");
}

/// Multiple reference types in one paragraph.
#[test]
fn mixed() {
    run("mixed");
}

// ── Feature-disabled smoke test ───────────────────────────────────────────────

/// When `githubAutolinks` is not configured, `#123` must remain plain text.
#[test]
fn disabled_leaves_text_unchanged() {
    let input = "See #123 for the fix.\n";
    let mut p = pipeline_without_autolinks();
    let hast = p.run(input).expect("pipeline failed");
    let html = serialize(&hast);
    assert!(
        html.contains("#123"),
        "disabled github_autolinks must preserve plain text: {html}"
    );
    assert!(
        !html.contains("<a"),
        "disabled github_autolinks must NOT produce <a>: {html}"
    );
}

// ── Acceptance-criteria spot checks ──────────────────────────────────────────

/// `#123` → correct issue URL with configured repo.
#[test]
fn bare_issue_url_shape() {
    let input = "See #1.\n";
    let mut p = pipeline_with_autolinks(TEST_REPO);
    let hast = p.run(input).expect("pipeline failed");
    let html = serialize(&hast);
    assert!(
        html.contains("https://github.com/owner/myrepo/issues/1"),
        "bare issue must use configured repo URL: {html}"
    );
}

/// `user/repo#456` uses the inline repo, not the configured repo.
#[test]
fn cross_repo_issue_url_shape() {
    let input = "See other/repo#7.\n";
    let mut p = pipeline_with_autolinks(TEST_REPO);
    let hast = p.run(input).expect("pipeline failed");
    let html = serialize(&hast);
    assert!(
        html.contains("https://github.com/other/repo/issues/7"),
        "cross-repo issue must use inline repo URL: {html}"
    );
}

/// SHA produces a `/commit/` URL.
#[test]
fn sha_url_shape() {
    let input = "See abc1234.\n";
    let mut p = pipeline_with_autolinks(TEST_REPO);
    let hast = p.run(input).expect("pipeline failed");
    let html = serialize(&hast);
    assert!(
        html.contains("https://github.com/owner/myrepo/commit/abc1234"),
        "SHA must produce commit URL: {html}"
    );
}
