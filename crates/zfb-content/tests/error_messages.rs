//! Cross-crate "error-quality" acceptance test for `zfb-content`.
//!
//! Pins the user-facing error produced when a markdown file in a content
//! collection has invalid YAML frontmatter. Bar:
//!
//! - file path
//! - line/col of the YAML error (provided by serde_yaml)
//! - what went wrong
//! - what was expected
//!
//! Companion to `zfb-render/tests/error_messages.rs`. The
//! `getCollection("doesnotexist")` failure mode and the `zfb.config.ts`
//! failure mode are tracked separately — see the Sub 4 log for the
//! "no surface yet" follow-up.

use std::path::PathBuf;

use serde::Deserialize;

use zfb_content::collection::{walk_collection, CollectionError, Entry};

#[derive(Debug, Deserialize, garde::Validate)]
#[allow(dead_code)]
struct PostSchema {
    #[garde(length(min = 1))]
    title: String,
}

// ---------------------------------------------------------------------------
// Failure mode 2 — invalid YAML frontmatter in a collection entry.
// ---------------------------------------------------------------------------

/// A `.md` file with malformed YAML frontmatter must surface an error that
/// includes the offending file path **and** a line/column from the YAML
/// parser.
#[test]
fn malformed_frontmatter_error_points_at_file_and_yaml_location() {
    let tmp = mk_tmp("bad-yaml");
    let bad = tmp.path.join("bad.md");
    // An unterminated flow sequence — serde_yaml rejects it and reports the
    // line/column of the failure.
    std::fs::write(
        &bad,
        "---\ntitle: [unclosed, broken\nother: ok\n---\nbody\n",
    )
    .unwrap();

    let err = walk_collection::<PostSchema>(&tmp.path, None)
        .expect_err("malformed frontmatter should fail");

    let CollectionError::Multiple { errors, .. } = &err else {
        unreachable!("expected Multiple aggregate, got {err:?}");
    };
    assert_eq!(errors.len(), 1, "expected a single aggregated error");

    let inner = &errors[0];
    let CollectionError::Frontmatter { path, message } = inner else {
        unreachable!("expected Frontmatter variant, got {inner:?}");
    };

    assert_eq!(path, &bad, "error must carry the offending file path");

    // serde_yaml's stringified error always says "at line N column M" when it
    // has a Location, which it does for syntactic errors like this one.
    assert!(
        message.contains("line"),
        "expected serde_yaml line/col context in error message, got: {message}",
    );
    assert!(
        message.contains("column"),
        "expected serde_yaml column in error message, got: {message}",
    );

    // Top-level Display of the aggregated error must also include the file
    // path so build logs are actionable without unwrapping the variant.
    let top_msg = err.to_string();
    assert!(
        top_msg.contains(bad.to_string_lossy().as_ref()),
        "top-level error message should include file path, got: {top_msg}",
    );
}

// ---------------------------------------------------------------------------
// Failure mode 4 — getCollection("doesnotexist"): SKIPPED.
//
// `zfb-content` exposes the static `walk_collection<T>` API and
// `emit_types_dts` for the TypeScript surface; there is no runtime
// `getCollection(name)` lookup that takes a name string and validates it
// against the registered collections yet. When that surface lands, this
// test should call it with an unknown name and assert the error includes
// the call site, the requested name, and the list of available names.
// Tracked in the Sub 4 log as a follow-up.
// ---------------------------------------------------------------------------

/// Mark the gap explicitly so a future regression — i.e. someone *adds* the
/// surface but forgets to wire the file-pointing error — is at least
/// surfaced as a failing-but-`#[ignore]`d test that future work flips on.
#[test]
#[ignore = "pending-feature: https://github.com/Takazudo/zudo-front-builder/issues/1352"]
fn unknown_collection_name_lists_available_collections() {
    // Once a `get_collection(name: &str)` (or equivalent) surface exists,
    // call it with `"doesnotexist"` and assert that:
    //   - the error mentions the call site (file path)
    //   - the error mentions the bad name
    //   - the error lists the names that *are* registered
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

struct TmpDir {
    path: PathBuf,
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn mk_tmp(label: &str) -> TmpDir {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "zfb-content-error-quality-{label}-{nanos}-{n}-{pid}",
        pid = std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create tmp dir");
    TmpDir { path: dir }
}

#[allow(dead_code)]
fn ensure_entry_unused() {
    // Touch types so unused-import warnings stay quiet on stable.
    let _: Option<Entry<PostSchema>> = None;
}
