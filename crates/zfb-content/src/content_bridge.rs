//! Content query Rust contract for the Rust → JS bridge.
//!
//! This module defines the serializable shape that flows from the Rust
//! build into the JS runtime so TSX page modules can call
//! `getCollection("docs")` and `getEntry("docs", "slug")` synchronously
//! during SSR. The actual deno_core wiring (embedding the snapshot into
//! `globalThis.__zfb.content`) is gated by ADR-001 and lands in a
//! follow-up epic — this sub delivers ONLY the Rust contract and the
//! TypeScript surface.
//!
//! See `docs/architecture/adr-003-content-bridge.md` for the JS-side
//! contract spec.
//!
//! # Determinism
//!
//! [`build_snapshot`] is deterministic: collections are keyed in a
//! [`BTreeMap`] (sorted ascending by collection name) and each
//! collection's `Vec<EntrySnapshot>` is sorted ascending by slug. Two
//! calls with the same `[CollectionConfig]` input always produce
//! byte-identical `serde_json` output, which the build cache relies on.
//!
//! # Why a parallel walker
//!
//! [`crate::collection::walk_collection`] is generic over `T: Validate`,
//! which is exactly what build-time consumers want when they *have* a
//! schema. The bridge does not — it ships the raw frontmatter as
//! `serde_json::Value` so the JS side can read whatever fields a TSX
//! page asks for. To reuse the existing walker without duplicating the
//! filesystem-traversal logic, we wrap [`serde_json::Value`] in a
//! transparent `UntypedFrontmatter` newtype that implements
//! [`garde::Validate`] as a no-op, then convert each `Entry<UntypedFrontmatter>`
//! into an `EntrySnapshot`. This keeps `walk_collection` as the single
//! source of truth for slug derivation, rel_path computation,
//! frontmatter parsing, and `module_specifier` generation.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::collection::{walk_collection, CollectionError, Entry};

/// A single content entry, in the shape the JS bridge sees.
///
/// All paths are normalized to forward-slash strings so the JSON
/// serialization is platform-independent. `frontmatter` is the parsed
/// frontmatter as JSON (YAML for `.md`/`.mdx`, the `export const
/// frontmatter` literal for `.tsx`). `body` is the markdown body for
/// `.md`/`.mdx` and the empty string for `.tsx` (TSX has no separate
/// markdown body).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntrySnapshot {
    /// Slug from the file stem (no extension).
    pub slug: String,
    /// Parsed frontmatter as JSON. `Null` when the source had none.
    pub frontmatter: JsonValue,
    /// Markdown body. Empty string for TSX entries.
    pub body: String,
    /// Stable specifier addressing the compiled module
    /// (`mdx://collection/slug#hash` or `tsx://collection/slug#hash`).
    pub module_specifier: String,
    /// Path relative to the collection root, normalized to use `/`
    /// separators so JSON output is stable across OSes.
    pub rel_path: String,
}

/// A point-in-time snapshot of every configured collection.
///
/// Collections are stored in a [`BTreeMap`] so iteration order is
/// deterministic (sorted ascending by collection name). Each
/// collection's `Vec<EntrySnapshot>` is sorted ascending by slug.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentSnapshot {
    /// Sorted map of collection name → entries.
    pub collections: BTreeMap<String, Vec<EntrySnapshot>>,
}

/// Configuration for one content collection in the snapshot.
///
/// `name` is the public-facing collection name JS code uses with
/// `getCollection(name)`. `root` is the directory the walker scans —
/// any `.md`, `.mdx`, or `.tsx` file under it becomes an
/// [`EntrySnapshot`]. A non-existent `root` is treated as an empty
/// collection (matches [`walk_collection`] today; documented on
/// [`build_snapshot`]).
#[derive(Debug, Clone)]
pub struct CollectionConfig {
    /// Public collection name (the key in [`ContentSnapshot::collections`]).
    pub name: String,
    /// Directory containing the collection's source files.
    pub root: PathBuf,
}

impl CollectionConfig {
    /// Convenience constructor.
    pub fn new(name: impl Into<String>, root: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            root: root.into(),
        }
    }
}

/// Errors produced by [`build_snapshot`].
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    /// Underlying collection walker failed (IO, frontmatter parse, MDX
    /// compile, schema). The variant is per-collection so the caller
    /// can tell which collection blew up.
    #[error("collection `{collection}` failed to walk: {source}")]
    Walk {
        /// Name of the failing collection.
        collection: String,
        /// Source error from the walker.
        #[source]
        source: CollectionError,
    },
}

/// Build a deterministic [`ContentSnapshot`] from the configured
/// collections.
///
/// Walks each collection in `collections` via [`walk_collection`] (with
/// `pipeline = None`), converts each entry into an [`EntrySnapshot`],
/// and inserts the resulting `Vec` into a [`BTreeMap`] keyed by
/// collection name.
///
/// Sort order:
/// - top-level: collection name ascending (free, courtesy of `BTreeMap`).
/// - per-collection: slug ascending (explicitly sorted; the walker's
///   own ordering is by `rel_path` which can differ from slug when
///   files live in nested directories).
///
/// # Missing collection roots
///
/// A `CollectionConfig` whose `root` does not exist on disk is treated
/// as an empty collection (zero entries). This matches the behavior of
/// [`walk_collection`] / `collect_collection_files`, which return
/// `Ok(())` for absent directories. Tests cover this contract so
/// regressions are loud.
///
/// # Errors
///
/// Returns [`BridgeError::Walk`] if any underlying walker call fails
/// (typically a malformed frontmatter, schema validation failure, or
/// MDX compile error inside one of the configured roots).
pub fn build_snapshot(collections: &[CollectionConfig]) -> Result<ContentSnapshot, BridgeError> {
    let mut out: BTreeMap<String, Vec<EntrySnapshot>> = BTreeMap::new();

    for cfg in collections {
        // The bridge ships frontmatter as raw JSON, so we walk with the
        // no-op `UntypedFrontmatter` schema. See module-level docs for
        // why the parallel walker is implemented as a `T` substitution
        // rather than a duplicate FS traversal.
        let entries: Vec<Entry<UntypedFrontmatter>> = walk_collection(&cfg.root, None)
            .map_err(|source| BridgeError::Walk {
                collection: cfg.name.clone(),
                source,
            })?;

        let mut snapshots: Vec<EntrySnapshot> = entries
            .into_iter()
            .map(|e| EntrySnapshot {
                slug: e.slug,
                frontmatter: e.data.0,
                body: e.body,
                module_specifier: e.module_specifier,
                rel_path: rel_path_to_string(&e.rel_path),
            })
            .collect();

        // Sort by slug ascending. `walk_collection` already sorts by
        // rel_path, but slugs and rel_paths can disagree (nested dirs,
        // different file extensions on the same slug, etc.) so we
        // re-sort explicitly to match the documented bridge contract.
        snapshots.sort_by(|a, b| a.slug.cmp(&b.slug));

        out.insert(cfg.name.clone(), snapshots);
    }

    Ok(ContentSnapshot { collections: out })
}

// -----------------------------------------------------------------------------
// internals
// -----------------------------------------------------------------------------

/// Newtype wrapper around [`serde_json::Value`] that satisfies the
/// `T: DeserializeOwned + garde::Validate<Context = ()>` bound on
/// [`walk_collection`] without imposing any schema. `#[serde(transparent)]`
/// makes deserialization equivalent to deserializing the inner Value.
#[derive(Debug, Clone, Deserialize)]
#[serde(transparent)]
struct UntypedFrontmatter(JsonValue);

impl garde::Validate for UntypedFrontmatter {
    type Context = ();

    fn validate_into(
        &self,
        _ctx: &Self::Context,
        _parent: &mut dyn FnMut() -> garde::Path,
        _report: &mut garde::Report,
    ) {
        // Bridge accepts any frontmatter shape — schema validation is a
        // build-time concern handled elsewhere (e.g. typed
        // walk_collection callers).
    }
}

/// Render a `Path` (relative) as a forward-slash string. `walk_collection`
/// builds rel_paths via `Path::strip_prefix`, which on Windows would emit
/// backslashes. Forcing `/` here keeps the JSON snapshot identical across
/// platforms — important for the determinism / cache-key contract.
fn rel_path_to_string(p: &std::path::Path) -> String {
    p.components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str().map(str::to_owned),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

// -----------------------------------------------------------------------------
// tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Self-cleaning temp dir helper (mirrors the one in `collection.rs`).
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
                "zfb-content-bridge-{label}-{nanos}-{n}-{pid}",
                pid = std::process::id()
            ));
            fs::create_dir_all(&dir).expect("create tmp dir");
            Self { path: dir }
        }
        fn write(&self, rel: &str, contents: &str) -> PathBuf {
            let p = self.path.join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(&p, contents).expect("write file");
            p
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn md(title: &str) -> String {
        format!("---\ntitle: \"{title}\"\n---\nbody for {title}\n")
    }

    fn tsx(title: &str) -> String {
        format!(
            "export const frontmatter = {{ title: '{title}' }};\n\
             export default function Page() {{ return null; }}\n"
        )
    }

    fn hash_canonical(snap: &ContentSnapshot) -> String {
        // serde_json on a `BTreeMap` emits keys in sorted order, and
        // each entry vec is already sorted by slug. The serializer is
        // therefore canonical for our purposes: identical input →
        // identical bytes.
        let s = serde_json::to_string(snap).expect("snapshot serializes");
        let mut h = Sha256::new();
        h.update(s.as_bytes());
        hex::encode(h.finalize())
    }

    #[test]
    fn build_snapshot_is_deterministic_under_repeated_calls() {
        let tmp = TmpDir::new("determinism");
        // Two collections, each with multiple entries. Mixed kinds.
        tmp.write("blog/alpha.md", &md("Alpha"));
        tmp.write("blog/zulu.mdx", &md("Zulu"));
        tmp.write("blog/mike.tsx", &tsx("Mike"));
        tmp.write("docs/intro.md", &md("Intro"));
        tmp.write("docs/advanced.mdx", &md("Advanced"));

        let cfgs = vec![
            CollectionConfig::new("blog", tmp.path.join("blog")),
            CollectionConfig::new("docs", tmp.path.join("docs")),
        ];

        let s1 = build_snapshot(&cfgs).expect("snapshot 1");
        let s2 = build_snapshot(&cfgs).expect("snapshot 2");

        assert_eq!(
            hash_canonical(&s1),
            hash_canonical(&s2),
            "two snapshot calls over the same fixture must hash identically",
        );
    }

    #[test]
    fn build_snapshot_is_deterministic_under_reversed_config_order() {
        let tmp = TmpDir::new("determinism-reorder");
        tmp.write("a/one.md", &md("One"));
        tmp.write("b/two.md", &md("Two"));

        let forward = vec![
            CollectionConfig::new("a", tmp.path.join("a")),
            CollectionConfig::new("b", tmp.path.join("b")),
        ];
        let reversed = vec![
            CollectionConfig::new("b", tmp.path.join("b")),
            CollectionConfig::new("a", tmp.path.join("a")),
        ];

        // BTreeMap sorts keys → the resulting snapshots are equivalent
        // regardless of the order configs were supplied in.
        assert_eq!(
            hash_canonical(&build_snapshot(&forward).unwrap()),
            hash_canonical(&build_snapshot(&reversed).unwrap()),
            "config order must not affect snapshot bytes",
        );
    }

    #[test]
    fn entries_within_collection_are_sorted_by_slug_ascending() {
        let tmp = TmpDir::new("sort");
        // Names chosen so that `rel_path` ordering and slug ordering
        // agree here (single-level dir), but the explicit slug sort
        // would still fire if the walker behaved differently.
        tmp.write("docs/zebra.md", &md("Z"));
        tmp.write("docs/apple.md", &md("A"));
        tmp.write("docs/mango.md", &md("M"));

        let snap = build_snapshot(&[CollectionConfig::new("docs", tmp.path.join("docs"))])
            .expect("snapshot ok");
        let docs = snap.collections.get("docs").expect("docs present");
        let slugs: Vec<&str> = docs.iter().map(|e| e.slug.as_str()).collect();
        assert_eq!(slugs, vec!["apple", "mango", "zebra"]);
    }

    #[test]
    fn missing_collection_root_yields_empty_vec_not_error() {
        // Documented contract: aligning with `walk_collection`'s lenient
        // treatment of a non-existent dir. This test pins the choice.
        let tmp = TmpDir::new("missing");
        let cfg = CollectionConfig::new("ghost", tmp.path.join("does-not-exist"));
        let snap = build_snapshot(&[cfg]).expect("missing root must not error");
        let ghost = snap.collections.get("ghost").expect("ghost key present");
        assert!(ghost.is_empty(), "missing root → empty collection");
    }

    #[test]
    fn entry_snapshot_carries_frontmatter_body_and_specifier() {
        let tmp = TmpDir::new("shape");
        tmp.write("blog/post.md", &md("Hello"));
        tmp.write("blog/page.tsx", &tsx("Tsx"));

        let snap = build_snapshot(&[CollectionConfig::new("blog", tmp.path.join("blog"))])
            .expect("snapshot ok");
        let blog = snap.collections.get("blog").expect("blog present");
        assert_eq!(blog.len(), 2);

        let page = blog.iter().find(|e| e.slug == "page").expect("page entry");
        let post = blog.iter().find(|e| e.slug == "post").expect("post entry");

        // Frontmatter survives the round-trip as JSON.
        assert_eq!(post.frontmatter["title"].as_str(), Some("Hello"));
        assert_eq!(page.frontmatter["title"].as_str(), Some("Tsx"));

        // Markdown bodies are preserved; TSX bodies are empty.
        assert!(post.body.contains("body for Hello"));
        assert!(page.body.is_empty(), "tsx entries have empty body");

        // Module specifiers reflect the file kind.
        assert!(
            post.module_specifier.starts_with("mdx://"),
            "got {}",
            post.module_specifier,
        );
        assert!(
            page.module_specifier.starts_with("tsx://"),
            "got {}",
            page.module_specifier,
        );

        // rel_path is forward-slash normalised even on Windows.
        assert_eq!(post.rel_path, "post.md");
        assert_eq!(page.rel_path, "page.tsx");
    }

    #[test]
    fn snapshot_serde_roundtrip_preserves_shape() {
        let tmp = TmpDir::new("roundtrip");
        tmp.write("blog/x.md", &md("X"));

        let snap = build_snapshot(&[CollectionConfig::new("blog", tmp.path.join("blog"))])
            .expect("snapshot ok");

        let json = serde_json::to_string(&snap).expect("serialize");
        let back: ContentSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(hash_canonical(&snap), hash_canonical(&back));
    }

    #[test]
    fn malformed_collection_surfaces_bridge_error_with_collection_name() {
        let tmp = TmpDir::new("error");
        tmp.write("blog/bad.md", "---\ntitle: [unclosed\n---\nbody\n");
        let cfg = CollectionConfig::new("blog", tmp.path.join("blog"));
        let err = build_snapshot(&[cfg]).expect_err("malformed YAML must fail");
        match err {
            BridgeError::Walk { collection, .. } => assert_eq!(collection, "blog"),
        }
    }

    #[test]
    fn nested_directories_use_slash_separator_in_rel_path() {
        let tmp = TmpDir::new("nested");
        tmp.write("docs/guides/intro.md", &md("Intro"));

        let snap = build_snapshot(&[CollectionConfig::new("docs", tmp.path.join("docs"))])
            .expect("snapshot ok");
        let docs = snap.collections.get("docs").expect("docs present");
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].rel_path, "guides/intro.md");
    }
}
