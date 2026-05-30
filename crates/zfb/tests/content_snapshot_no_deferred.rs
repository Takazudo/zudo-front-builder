//! Regression test for issue #495 — content snapshot is built even when there
//! are no deferred dynamic routes.
//!
//! ## What this tests
//!
//! A project shaped as "static homepage lists a collection via `getStaticProps`
//! and `getCollection`, but has NO dynamic `[slug].tsx` page" used to get empty
//! `getCollection(...)` results in the production build. The bug was that
//! `build.rs` only called `build_content_snapshot_json` when
//! `!still_deferred.is_empty()`, so a project with no deferred `paths()` routes
//! received a `None` snapshot and `getCollection` silently returned `[]`.
//!
//! ## Fixture
//!
//! `tests/fixtures/collection-static-getStaticProps/` contains:
//! - `zfb.config.json` with one `posts` collection
//! - `content/posts/hello.md` — one post with frontmatter `title: "Hello World"`
//! - `pages/index.tsx` — calls `getCollection("posts")` inside `getStaticProps`,
//!   renders `post-count:<N>` and the post title; no `[slug].tsx` page
//!
//! ## Assertion
//!
//! After `zfb build`, `dist/index.html` must contain `post-count:1` and
//! "Hello World". On the old code (gate present), the snapshot was skipped and
//! the output contained `post-count:0`.
//!
//! ## Skip behaviour
//!
//! The test requires the embedded esbuild and V8 runtime. These are present in
//! the default feature set (`embed_v8` is in `[features] default`). The test
//! guards against a missing binary via the same skip pattern used elsewhere —
//! it checks for a non-success exit and a recognisable error message, and skips
//! gracefully with `eprintln!` rather than panicking.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use zfb_test_utils::zfb_binary;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("collection-static-getStaticProps")
}

/// Run `zfb build` against the fixture in a fresh tempdir copy so parallel
/// test runs don't stomp on each other's `dist/` outputs.
#[test]
fn getstaticprops_collection_appears_in_rendered_html() {
    // Copy the fixture into a tempdir so multiple test runs stay isolated.
    let tmp = tempfile::tempdir().expect("create tempdir for fixture copy");
    let root = tmp.path();

    let fixture = fixture_dir();
    copy_dir(&fixture, root).expect("copy fixture into tempdir");

    // Run `zfb build` against the copy.
    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(root)
        .output()
        .expect("spawn zfb binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");

    // Skip gracefully when the embedded runtime is unavailable (e.g. build
    // was done without the embed_v8 feature, or in a stripped CI image).
    if !output.status.success() {
        if combined.contains("embed_v8") || combined.contains("no esbuild") {
            eprintln!(
                "[content_snapshot_no_deferred] zfb build exited non-zero with \
                 a known-skip indicator; skipping test.\n\
                 stdout: {stdout}\nstderr: {stderr}"
            );
            return;
        }
        panic!(
            "zfb build failed unexpectedly for collection-static-getStaticProps fixture.\n\
             status: {:?}\nstdout: {stdout}\nstderr: {stderr}",
            output.status,
        );
    }

    // Read the rendered homepage.
    let index_html = root.join("dist").join("index.html");
    assert!(
        index_html.exists(),
        "dist/index.html must be written by zfb build; \
         stdout: {stdout}\nstderr: {stderr}"
    );
    let html = fs::read_to_string(&index_html)
        .expect("read dist/index.html");

    // The load-bearing assertion for issue #495: the content snapshot must
    // be embedded and getCollection("posts") must see the one fixture post.
    //
    // Before the fix (gate present): snapshot was skipped because
    // `still_deferred.is_empty()` was true, so getCollection returned `[]`
    // and the rendered HTML contained `post-count:0`.
    //
    // After the fix (gate removed): snapshot is always built when collections
    // are configured, so getCollection returns the real entry and the rendered
    // HTML contains `post-count:1`.
    assert!(
        html.contains("post-count:1"),
        "rendered dist/index.html must contain 'post-count:1' — \
         getCollection(\"posts\") must see the fixture's hello.md entry. \
         This assertion fails on main HEAD (issue #495: gate skips snapshot \
         when there are no deferred paths()). \
         Got html:\n{html}"
    );

    // Belt-and-suspenders: the post title must also appear.
    assert!(
        html.contains("Hello World"),
        "rendered dist/index.html must contain 'Hello World' from the \
         fixture's hello.md frontmatter title. \
         Got html:\n{html}"
    );
}

/// Recursive directory copy (files only; creates target subdirs as needed).
fn copy_dir(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&entry.path(), &dst_path)?;
        } else {
            fs::copy(entry.path(), dst_path)?;
        }
    }
    Ok(())
}
