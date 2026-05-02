//! Content query Rust contract for the Rust → JS bridge.
//!
//! This module defines the serializable shape that flows from the Rust
//! build into the JS runtime so TSX page modules can call
//! `getCollection("docs")` and `getEntry("docs", "slug")` synchronously
//! during SSR. The actual JS-side wiring (embedding the snapshot into
//! `globalThis.__zfb.content` via the miniflare runtime per ADR-005)
//! lands in a follow-up epic — this sub delivers ONLY the Rust contract
//! and the TypeScript surface.
//!
//! See `docs/architecture/adr-004-content-bridge.md` for the JS-side
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
use crate::pipeline::Pipeline;

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
/// Walks each collection in `collections` via [`walk_collection`],
/// driving the walker through a fresh-per-collection
/// [`Pipeline::with_defaults()`] so the resulting JSX (and the
/// [`module_specifier`](EntrySnapshot::module_specifier) hash baked from
/// it) is **byte-identical** to what the bundler emits in
/// `crates/zfb-build/src/bundler.rs`. Snapshot specifiers must agree
/// with the bundler's bridge map keys byte-for-byte; otherwise every
/// `globalThis.__zfb.content.get(specifier)` lookup misses and the page
/// renders the raw-markdown `<pre data-zfb-content-fallback>` fallback.
/// Driving both code paths through the same default pipeline shape —
/// one pipeline per collection, reused across files within that
/// collection — is what keeps the two hashes in lock-step
/// (zfb #131 / #132).
///
/// Each entry is converted into an [`EntrySnapshot`] and the resulting
/// `Vec` is inserted into a [`BTreeMap`] keyed by collection name.
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
        // Construct a fresh `Pipeline::with_defaults()` per collection,
        // mirroring the bundler's two call sites at
        // `crates/zfb-build/src/bundler.rs:734` and `:964` — each
        // `materialise_*` call hoists its own pipeline outside the
        // walk loop, so within one collection a single pipeline serves
        // every entry sequentially while no state leaks across
        // collection boundaries. Per-collection instantiation matters
        // because some default plugins (notably `HeadingLinksPlugin`)
        // carry per-document state (the slug-dedupe `seen` map):
        // reusing one pipeline across collections would mean a heading
        // like "Intro" in collection B would slugify to `intro-1` if
        // collection A already used `intro`, leaking unrelated
        // collections' headings into B's compiled JSX (and into the
        // hash that drives `module_specifier`). The snapshot must
        // agree byte-for-byte with the bundler's bridge map keys (see
        // the doc comment on `build_snapshot`); the fastest way to
        // hold that contract is to drive `walk_collection` with the
        // same pipeline shape the bundler uses.
        let mut pipeline = Pipeline::with_defaults();

        // The bridge ships frontmatter as raw JSON, so we walk with the
        // no-op `UntypedFrontmatter` schema. See module-level docs for
        // why the parallel walker is implemented as a `T` substitution
        // rather than a duplicate FS traversal.
        let entries: Vec<Entry<UntypedFrontmatter>> =
            walk_collection(&cfg.root, Some(&mut pipeline)).map_err(|source| {
                BridgeError::Walk {
                    collection: cfg.name.clone(),
                    source,
                }
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

    let snapshot = ContentSnapshot { collections: out };
    log_snapshot_size_if_requested(&snapshot);
    Ok(snapshot)
}

/// If `ZFB_DEBUG_SNAPSHOT` is set to a truthy value (`1` or `true`,
/// case-insensitive), print a one-line summary of the serialized
/// snapshot size to stderr. Anything else (unset, empty, `0`, `false`,
/// or unrecognized) is a no-op.
///
/// Used to monitor V8 RAM pressure: zfb embeds the full content
/// snapshot in the miniflare worker at build time, so the byte size
/// reported here is roughly what gets held in V8 during render. See
/// the "Limits" section of the project README.
///
/// Failure is silent on purpose — debug telemetry must never panic the
/// build. If `serde_json::to_string` ever returns `Err` (it cannot for
/// our shape, which contains no NaN/non-string keys), we just skip the
/// log line.
fn log_snapshot_size_if_requested(snap: &ContentSnapshot) {
    if !debug_snapshot_enabled() {
        return;
    }
    let entries: usize = snap.collections.values().map(|v| v.len()).sum();
    let bytes = match serde_json::to_string(snap) {
        Ok(s) => s.len(),
        Err(_) => return,
    };
    // Round up so a 1-byte snapshot still reports as 1 KB rather than 0.
    let kb = bytes.div_ceil(1024);
    eprintln!("content snapshot: {entries} entries / {kb} KB");
}

/// Read `ZFB_DEBUG_SNAPSHOT` and decide whether to emit the debug line.
/// Truthy values: `1`, `true` (case-insensitive). Everything else —
/// including unset, empty string, and unrecognized values like `yes` —
/// is treated as off so a stray export does not change build output.
///
/// Public so the CLI can avoid building a snapshot purely for telemetry
/// when the flag is off.
pub fn debug_snapshot_enabled() -> bool {
    std::env::var("ZFB_DEBUG_SNAPSHOT")
        .ok()
        .as_deref()
        .map(parse_debug_truthy)
        .unwrap_or(false)
}

/// Pure parser for the `ZFB_DEBUG_SNAPSHOT` value. Split out from
/// [`debug_snapshot_enabled`] so the truthiness rule is testable
/// without mutating process-global env state.
fn parse_debug_truthy(raw: &str) -> bool {
    let t = raw.trim();
    t.eq_ignore_ascii_case("1") || t.eq_ignore_ascii_case("true")
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
    fn debug_truthy_parser_accepts_only_documented_truthy_values() {
        // Documented truthy: `1`, `true` (case-insensitive, trim whitespace).
        for raw in ["1", " 1 ", "true", "TRUE", "True", "  true\n"] {
            assert!(parse_debug_truthy(raw), "{raw:?} should be truthy");
        }
        // Everything else is off — guards against accidental enabling
        // when a script sets `ZFB_DEBUG_SNAPSHOT=yes` or similar.
        for raw in ["", "0", "false", "no", "yes", "on", "off", "TRUE!"] {
            assert!(!parse_debug_truthy(raw), "{raw:?} should be falsy");
        }
    }

    #[test]
    fn snapshot_specifier_matches_bridge_hash_for_pipeline_transformed_entry() {
        // Regression guard for zfb #132: `build_snapshot` must drive
        // `walk_collection` through the same default `Pipeline` the
        // bundler uses, so the `module_specifier` baked into the
        // snapshot agrees byte-for-byte with the bridge map key the
        // bundler emits. Without the fix, `walk_collection` ran with
        // `pipeline = None` (pre-pipeline JSX, pre-pipeline hash) while
        // the bundler ran with `Some(&mut Pipeline::with_defaults())`
        // (post-pipeline JSX, post-pipeline hash). Any page whose body
        // the pipeline actually transformed — admonitions, mermaid,
        // syntect-highlighted fences, image-enlarge, heading-links,
        // CJK-friendly emphasis — produced two divergent specifiers,
        // and `globalThis.__zfb.content.get(snapshot.module_specifier)`
        // missed at render time → silent fallback render.
        //
        // We reproduce that hazard with the cheapest signal we have:
        // a `:::note` admonition. `AdmonitionsPlugin` rewrites the
        // mdast subtree into an `<MdxJsxFlowElement name="Note">`, so
        // the post-pipeline JSX (and therefore its `content_hash`)
        // differs from the pre-pipeline JSX. Asserting that the
        // snapshot's hash component matches the hash an independent
        // `compile_mdx_to_jsx_module_cached(... Some(&mut Pipeline::with_defaults()))`
        // call produces is the explicit "snapshot hash matches bridge
        // hash" guarantee.
        use crate::mdx_jsx_emit::{compile_mdx_to_jsx_module_cached, parse_mdx_specifier};

        let tmp = TmpDir::new("snapshot-pipeline-hash");
        // The `:::note` directive is the canonical pipeline-transformable
        // signal — without the default pipeline it survives as raw text;
        // with it, it becomes an `<MdxJsxFlowElement name="Note">`.
        let mdx = "---\ntitle: \"Admon\"\n---\n\n:::note\nhi\n:::\n";
        let path = tmp.write("docs/admon.mdx", mdx);

        let snap = build_snapshot(&[CollectionConfig::new("docs", tmp.path.join("docs"))])
            .expect("build_snapshot must succeed");
        let docs = snap
            .collections
            .get("docs")
            .expect("docs collection present");
        let entry = docs
            .iter()
            .find(|e| e.slug == "admon")
            .expect("admon entry");

        // Independently compile the same body through the same default
        // pipeline the bundler uses, then compare hash components.
        let body = "\n:::note\nhi\n:::\n"; // post-frontmatter body, mirrors `walk_collection`'s split.
        let mut pipeline = Pipeline::with_defaults();
        let compiled = compile_mdx_to_jsx_module_cached(body, &path, None, Some(&mut pipeline))
            .expect("independent compile must succeed");

        let snap_spec = parse_mdx_specifier(&entry.module_specifier)
            .expect("snapshot specifier parses as `mdx://...#hash`");
        let bridge_spec = parse_mdx_specifier(&compiled.specifier)
            .expect("bundler-style specifier parses as `mdx://...#hash`");

        assert_eq!(
            snap_spec.content_hash, bridge_spec.content_hash,
            "snapshot module_specifier hash ({snap}) must equal the bundler's bridge key hash ({bridge}); a divergence here is the zfb #132 regression — the snapshot path is walking with `pipeline = None` while the bundler walks with `Some(&mut Pipeline::with_defaults())`, so transformed pages render the raw-markdown fallback",
            snap = snap_spec.content_hash,
            bridge = bridge_spec.content_hash,
        );
        // Sanity: collection + slug also agree.
        assert_eq!(snap_spec.collection, bridge_spec.collection);
        assert_eq!(snap_spec.slug, bridge_spec.slug);
    }

    #[test]
    fn pipeline_state_does_not_leak_across_collections() {
        // Companion guard for the per-collection pipeline shape:
        // `HeadingLinksPlugin` carries a `seen: HashMap<String, usize>`
        // slug-dedupe counter on the pipeline instance. If
        // `build_snapshot` reused one `Pipeline::with_defaults()` across
        // every collection in the for-loop, a heading like `## Intro`
        // in collection B would emit `id="intro-1"` after collection A
        // already consumed `intro` — and the snapshot bytes for B would
        // depend on which other collections were configured alongside
        // it. The bundler dodges this by hoisting one pipeline per
        // `materialise_*` call (one per collection), and `build_snapshot`
        // mirrors that shape for the same reason.
        //
        // We assert the contract by building the snapshot for two
        // single-collection configs in isolation, then for the same two
        // collections together, and proving the per-collection slice of
        // the combined snapshot matches the isolated build byte-for-byte.
        let tmp = TmpDir::new("collection-isolation");
        // Both files have the same heading text — without per-collection
        // pipelines the second one walked would slug to `intro-1`.
        let mdx_with_h2 = "---\ntitle: \"X\"\n---\n\n## Intro\n\nbody\n";
        tmp.write("a/page.mdx", mdx_with_h2);
        tmp.write("b/page.mdx", mdx_with_h2);

        let cfg_a = CollectionConfig::new("a", tmp.path.join("a"));
        let cfg_b = CollectionConfig::new("b", tmp.path.join("b"));

        let solo_a = build_snapshot(&[cfg_a.clone()]).expect("solo a");
        let solo_b = build_snapshot(&[cfg_b.clone()]).expect("solo b");
        let combined = build_snapshot(&[cfg_a, cfg_b]).expect("combined");

        let combined_a = combined.collections.get("a").expect("combined a");
        let combined_b = combined.collections.get("b").expect("combined b");
        let solo_a_entries = solo_a.collections.get("a").expect("solo a entries");
        let solo_b_entries = solo_b.collections.get("b").expect("solo b entries");

        assert_eq!(
            combined_a[0].module_specifier, solo_a_entries[0].module_specifier,
            "collection a's specifier must not depend on whether collection b is also configured (heading-link state leak)",
        );
        assert_eq!(
            combined_b[0].module_specifier, solo_b_entries[0].module_specifier,
            "collection b's specifier must not depend on whether collection a is also configured (heading-link state leak)",
        );
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
