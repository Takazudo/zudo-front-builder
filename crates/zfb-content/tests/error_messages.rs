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
//! `getCollection("doesnotexist")` failure mode is covered below; the
//! `zfb.config.ts` failure mode is tracked separately (see
//! `zfb-render/tests/error_messages.rs`).

use std::path::PathBuf;

use serde::Deserialize;

use zfb_content::collection::{walk_collection, CollectionError, Entry};
use zfb_content::{build_snapshot, BridgeError, CollectionConfig};

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
// Failure mode 4 — getCollection("doesnotexist").
//
// `ContentSnapshot::get_collection(name, call_site)` (crate root:
// `content_bridge.rs`) is the runtime lookup surface for a name string
// against the registered collections. This test pins its error message.
// ---------------------------------------------------------------------------

/// `ContentSnapshot::get_collection` with an unregistered name must surface
/// an error that includes the call site (file path), the requested name,
/// and the list of names that *are* registered.
#[test]
fn unknown_collection_name_lists_available_collections() {
    let tmp = mk_tmp("unknown-collection");
    let blog_dir = tmp.path.join("blog");
    let docs_dir = tmp.path.join("docs");
    std::fs::create_dir_all(&blog_dir).unwrap();
    std::fs::create_dir_all(&docs_dir).unwrap();
    std::fs::write(
        blog_dir.join("post.md"),
        "---\ntitle: \"Post\"\n---\nbody for post\n",
    )
    .unwrap();
    std::fs::write(
        docs_dir.join("intro.md"),
        "---\ntitle: \"Intro\"\n---\nbody for intro\n",
    )
    .unwrap();

    let snap = build_snapshot(&[
        CollectionConfig::new("blog", &blog_dir),
        CollectionConfig::new("docs", &docs_dir),
    ])
    .expect("snapshot must build");

    let call_site = tmp.path.join("pages/index.tsx");
    let err = snap
        .get_collection("doesnotexist", &call_site)
        .expect_err("unknown collection name must fail");

    let BridgeError::UnknownCollection { .. } = &err else {
        unreachable!("expected UnknownCollection variant, got {err:?}");
    };

    let msg = err.to_string();
    assert!(
        msg.contains(&call_site.to_string_lossy().to_string()),
        "error must mention the call site, got: {msg}",
    );
    assert!(
        msg.contains("doesnotexist"),
        "error must mention the requested name, got: {msg}",
    );
    assert!(msg.contains("blog"), "error must list `blog`, got: {msg}");
    assert!(msg.contains("docs"), "error must list `docs`, got: {msg}");

    // Positive path: a registered name resolves.
    let blog = snap
        .get_collection("blog", &call_site)
        .expect("blog collection must be found");
    assert_eq!(blog.len(), 1);
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
