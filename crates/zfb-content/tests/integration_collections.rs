//! End-to-end test for `walk_collection`.
//!
//! Uses the static fixture at `tests/fixtures/blog-collection/` (3 valid
//! posts) and a per-test temporary directory for the failure case so
//! adding a malformed file doesn't pollute the static fixture.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use zfb_content::collection::{walk_collection, CollectionError, Entry};

#[derive(Debug, Deserialize, garde::Validate)]
struct BlogPost {
    #[garde(length(min = 1))]
    title: String,
    #[garde(length(min = 1))]
    date: String,
}

fn fixtures_blog_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("blog-collection")
}

#[test]
fn walks_valid_collection_returns_three_entries_sorted() {
    let dir = fixtures_blog_dir();
    let entries: Vec<Entry<BlogPost>> =
        walk_collection(&dir).expect("valid blog collection should walk cleanly");
    assert_eq!(entries.len(), 3, "expected 3 valid posts, got {entries:?}");

    // Entries are sorted by rel_path. Filenames are 01-, 02-, 03- so the
    // alphabetical order is also the chronological order.
    let rels: Vec<_> = entries
        .iter()
        .map(|e| e.rel_path.to_string_lossy().into_owned())
        .collect();
    let mut sorted = rels.clone();
    sorted.sort();
    assert_eq!(rels, sorted, "entries must be sorted by rel_path: {rels:?}");

    // Sanity-check the first entry's parsed schema.
    let first = &entries[0];
    assert_eq!(first.slug, "01-first");
    assert_eq!(first.data.title, "First Post");
    assert_eq!(first.data.date, "2026-01-01");
    assert!(
        first.body.contains("First body"),
        "body must include source after frontmatter: {:?}",
        first.body
    );
}

#[test]
fn invalid_frontmatter_returns_multiple_error() {
    // Build a temp collection that mirrors the static one plus a 4th
    // file with an invalid (empty) title that violates the schema.
    let tmp = TmpDir::new("invalid-blog");

    // Copy the 3 valid fixtures.
    for name in ["01-first.md", "02-second.md", "03-third.md"] {
        let body = std::fs::read_to_string(fixtures_blog_dir().join(name)).unwrap();
        tmp.write(name, &body);
    }
    // Add a 4th file with empty title -> garde length(min=1) fails.
    tmp.write(
        "04-bad.md",
        "---\ntitle: \"\"\ndate: \"2026-04-01\"\n---\nbody\n",
    );

    let err = walk_collection::<BlogPost>(tmp.path())
        .expect_err("walk must fail when one entry is invalid");
    match err {
        CollectionError::Multiple { errors, summary } => {
            assert_eq!(
                errors.len(),
                1,
                "expected exactly 1 aggregated error, got {errors:?}",
            );
            assert!(
                matches!(errors[0], CollectionError::Schema { .. }),
                "expected Schema error variant, got {:?}",
                errors[0],
            );
            assert!(!summary.is_empty(), "summary must be non-empty");
        }
        other => panic!("expected Multiple error, got {other:?}"),
    }
}

#[test]
fn missing_required_field_returns_multiple_error() {
    // A 4th file missing the `title` field -> serde_yaml deserialization
    // fails -> Frontmatter error variant (the schema's Deserialize
    // rejects the input before garde::Validate runs).
    let tmp = TmpDir::new("missing-title");
    for name in ["01-first.md", "02-second.md", "03-third.md"] {
        let body = std::fs::read_to_string(fixtures_blog_dir().join(name)).unwrap();
        tmp.write(name, &body);
    }
    tmp.write("04-no-title.md", "---\ndate: \"2026-04-01\"\n---\nbody\n");
    let err = walk_collection::<BlogPost>(tmp.path()).expect_err("missing title must fail");
    match err {
        CollectionError::Multiple { errors, .. } => {
            assert_eq!(errors.len(), 1);
            assert!(
                matches!(errors[0], CollectionError::Frontmatter { .. }),
                "expected Frontmatter (deserialize) error, got {:?}",
                errors[0],
            );
        }
        other => panic!("expected Multiple, got {other:?}"),
    }
}

// -----------------------------------------------------------------------------
// TmpDir helper (std-only — no `tempfile` dep).
// -----------------------------------------------------------------------------

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

struct TmpDir {
    path: PathBuf,
}

impl TmpDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "zfb-content-it-collection-{label}-{nanos}-{n}-{pid}",
            pid = std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create tmp dir");
        Self { path: dir }
    }
    fn path(&self) -> &Path {
        &self.path
    }
    fn write(&self, rel: &str, contents: &str) {
        let p = self.path.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&p, contents).expect("write file");
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
