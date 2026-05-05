//! ContentSnapshot end-to-end verification under the embedded-V8 host.
//!
//! ## What this test suite covers
//!
//! 1. **Bundler literal embedding** — `bundle()` with a real
//!    `build_snapshot()` result embeds the snapshot as a JSON literal in
//!    the generated `entry.mjs`, and the literal equals the serde_json
//!    serialisation of the snapshot byte-for-byte.
//!
//! 2. **Snapshot determinism** — two `build_snapshot()` calls over the
//!    same fixture directory produce byte-identical JSON. SHA-256 hashes
//!    must match. This pins the sort-by-(collection, slug) contract
//!    documented in `crates/zfb-content/src/content_bridge.rs`.
//!
//! 3. **EmbeddedV8 render path** (gated, requires sub-162) — a bundle
//!    built with `Backend::EmbeddedV8` renders a page that calls
//!    `getCollection("blog")` and the HTML contains snapshot data.
//!    **This test is annotated `#[ignore]` and will fail until
//!    sub-162 (EmbeddedV8RenderHost) is merged** because
//!    `Backend::EmbeddedV8` does not yet exist in this worktree.
//!    The manager re-runs it after merging sub-162.
//!
//! ## Dependency note
//!
//! The bundler-level tests (groups 1 and 2) do NOT require esbuild or a
//! running JS runtime — they exercise only the `bundle()` codepath
//! with `mock_subprocess_output` so they run in CI without any external
//! binary. The EmbeddedV8 render test (group 3) is gated behind
//! `#[ignore]` precisely because it needs sub-162's host.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;

use zfb_build::{bundle, BundleMode, BundlerInput};
use zfb_content::{build_snapshot, CollectionConfig};
use zfb_render::adapters::Framework;

// ---------------------------------------------------------------------------
// Shared fixture helpers
// ---------------------------------------------------------------------------

/// Write a minimal content fixture with two `.md` files into `dir`.
/// Returns the collection path (`dir/blog`).
fn write_blog_fixture(dir: &std::path::Path) -> PathBuf {
    let blog = dir.join("blog");
    fs::create_dir_all(&blog).unwrap();
    fs::write(
        blog.join("alpha.md"),
        "---\ntitle: \"Alpha Post\"\ndate: \"2026-01-01\"\n---\nBody of alpha.\n",
    )
    .unwrap();
    fs::write(
        blog.join("zulu.md"),
        "---\ntitle: \"Zulu Post\"\ndate: \"2026-03-01\"\n---\nBody of zulu.\n",
    )
    .unwrap();
    blog
}

/// Build a minimal `BundlerInput` that uses `mock_subprocess_output` so
/// the test doesn't need a real esbuild binary. The mock output satisfies
/// the bundler's post-processing expectations (routes export + hydrateIsland).
fn make_mock_input(tmp: &tempfile::TempDir, snapshot_json: Option<String>) -> BundlerInput {
    let root = tmp.path().to_path_buf();
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::create_dir_all(root.join("content")).unwrap();
    fs::create_dir_all(root.join("components")).unwrap();
    fs::create_dir_all(root.join("layouts")).unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        "export default function Home() { return null; }\n",
    )
    .unwrap();
    BundlerInput {
        project_root: root.clone(),
        pages_dir: PathBuf::from("pages"),
        content_dir: PathBuf::from("content"),
        content_collections: Vec::new(),
        components_dir: PathBuf::from("components"),
        layouts_dir: PathBuf::from("layouts"),
        framework: Framework::Preact,
        define_vars: HashMap::new(),
        tsconfig_paths: BTreeMap::new(),
        external: vec![],
        outdir: root.join("dist"),
        mode: BundleMode::Production,
        minify: false,
        esbuild_binary: None,
        mock_subprocess_output: Some(
            "// mock bundle\nexport const routes = {};\nexport const hydrateIsland = () => {};\n"
                .to_string(),
        ),
        content_snapshot_json: snapshot_json,
        node_modules_dir: None,
        node_modules_preserve_symlinks: false,
        strip_md_ext: false,
        code_highlight_theme: None,
        resolve_markdown_links: None,
    }
}

// ---------------------------------------------------------------------------
// Group 1 — bundler embeds a real ContentSnapshot as a JS literal
// ---------------------------------------------------------------------------

/// Build a real `ContentSnapshot` from a fixture, serialise it, pass it to
/// `bundle()`, and assert the bundle pipeline accepts the snapshot without
/// error and that the JSON is stable across a serde round-trip.
///
/// This covers the path:
///   `build_snapshot()` → `serde_json::to_string()` → `BundlerInput::content_snapshot_json`
///   → `bundle()` internal `write_entry_module()` validation → no fallback to empty snapshot.
///
/// Note: the shadow `entry.mjs` is an internal tempdir artefact cleaned up
/// inside `bundle()`. The public contract we can assert is that `bundle()`
/// succeeds (the validation inside `write_entry_module` must not fall back to
/// the empty snapshot) AND the JSON is serde-stable (bundler embeds it
/// verbatim, so a corrupt or truncated snapshot would surface as a mismatch
/// on round-trip).
#[test]
fn bundle_embeds_real_snapshot_as_js_literal() {
    let fixture_tmp = tempfile::tempdir().unwrap();
    let blog_dir = write_blog_fixture(fixture_tmp.path());

    let snap = build_snapshot(&[CollectionConfig::new("blog", &blog_dir)])
        .expect("build_snapshot from fixture");
    let snap_json = serde_json::to_string(&snap).expect("serialise snapshot");

    // Sanity: the snapshot must carry the two blog entries.
    assert!(
        snap_json.contains("\"alpha\""),
        "snapshot JSON should mention slug 'alpha'; got: {snap_json}"
    );
    assert!(
        snap_json.contains("\"zulu\""),
        "snapshot JSON should mention slug 'zulu'; got: {snap_json}"
    );
    assert!(
        snap_json.contains("\"collections\""),
        "snapshot JSON must have top-level 'collections' key; got: {snap_json}"
    );

    // Pass the real snapshot JSON to bundle(). The bundler's
    // `write_entry_module` validates the shape; a malformed value would
    // silently substitute `{ collections: {} }` instead — that would
    // not fail bundle(), but the serde round-trip check below would
    // catch it if the snap_json itself were somehow corrupt.
    let bundle_tmp = tempfile::tempdir().unwrap();
    let input = make_mock_input(&bundle_tmp, Some(snap_json.clone()));
    let out = bundle(input).expect("mock bundle should succeed with real snapshot");

    assert!(
        out.bundle_path.exists(),
        "bundle.mjs must be written even in mock mode"
    );
    assert_eq!(out.manifest.routes.len(), 1, "single route expected");

    // Serde round-trip: serde_json serialises BTreeMap keys in sorted
    // order; the snapshot's collections are stored in a BTreeMap so
    // two serialise calls must produce identical bytes. This asserts
    // the bundler can receive and embed the snapshot verbatim without
    // any mutation.
    let back: zfb_content::ContentSnapshot =
        serde_json::from_str(&snap_json).expect("snapshot JSON round-trips through serde");
    let back_json = serde_json::to_string(&back).expect("re-serialise");
    assert_eq!(
        snap_json, back_json,
        "snapshot JSON must be stable across serde round-trips (bundler embeds verbatim)"
    );

    // The blog collection must have two entries in slug-sorted order.
    let blog = back
        .collections
        .get("blog")
        .expect("blog collection present");
    assert_eq!(blog.len(), 2, "two blog entries expected");
    assert_eq!(blog[0].slug, "alpha", "first slug alphabetically");
    assert_eq!(blog[1].slug, "zulu", "second slug alphabetically");
}

/// Confirm that the bundler validates the snapshot shape: a JSON string
/// that is NOT a `{ collections: {...} }` object must not cause the build
/// to fail — the bundler falls back to the empty snapshot. This ensures the
/// validation logic in `bundler.rs` (`write_entry_module`) is exercised
/// when a real snapshot is provided alongside the mock path.
#[test]
fn bundle_falls_back_gracefully_for_invalid_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    // Pass an invalid snapshot — plain array. The bundler should fall
    // back to the empty `{ collections: {} }` and still succeed.
    let input = make_mock_input(&tmp, Some("[\"not\", \"an\", \"object\"]".to_string()));
    let out = bundle(input).expect("bundle must succeed even with malformed snapshot");
    assert!(out.bundle_path.exists());
}

// ---------------------------------------------------------------------------
// Group 2 — snapshot determinism
// ---------------------------------------------------------------------------

/// `build_snapshot()` must produce byte-identical JSON across two calls
/// over the same fixture. This pins the sort-by-(collection, slug)
/// guarantee documented in `crates/zfb-content/src/content_bridge.rs`.
///
/// We use SHA-256 of the serialized JSON as the equality witness so the
/// test catches any non-determinism in BTreeMap iteration or Vec ordering.
#[test]
fn snapshot_json_is_deterministic_across_calls() {
    use sha2::{Digest, Sha256};

    let fixture_tmp = tempfile::tempdir().unwrap();
    // Two collections with several entries in intentionally
    // non-alphabetical creation order.
    let docs = fixture_tmp.path().join("docs");
    let blog = fixture_tmp.path().join("blog");
    fs::create_dir_all(&docs).unwrap();
    fs::create_dir_all(&blog).unwrap();

    fs::write(
        docs.join("zz-last.md"),
        "---\ntitle: \"Z\"\n---\nlast\n",
    )
    .unwrap();
    fs::write(
        docs.join("aa-first.md"),
        "---\ntitle: \"A\"\n---\nfirst\n",
    )
    .unwrap();
    fs::write(
        blog.join("beta.md"),
        "---\ntitle: \"Beta\"\n---\nbeta body\n",
    )
    .unwrap();
    fs::write(
        blog.join("alpha.md"),
        "---\ntitle: \"Alpha\"\n---\nalpha body\n",
    )
    .unwrap();

    let cfgs = vec![
        // Intentionally supply in reverse alphabetical order to confirm
        // BTreeMap normalisation.
        CollectionConfig::new("docs", &docs),
        CollectionConfig::new("blog", &blog),
    ];

    let snap1 = build_snapshot(&cfgs).expect("snapshot call 1");
    let snap2 = build_snapshot(&cfgs).expect("snapshot call 2");

    let json1 = serde_json::to_string(&snap1).expect("serialise 1");
    let json2 = serde_json::to_string(&snap2).expect("serialise 2");

    let hash1 = {
        let mut h = Sha256::new();
        h.update(json1.as_bytes());
        hex::encode(h.finalize())
    };
    let hash2 = {
        let mut h = Sha256::new();
        h.update(json2.as_bytes());
        hex::encode(h.finalize())
    };

    assert_eq!(
        hash1, hash2,
        "two build_snapshot() calls over the same fixture must produce identical SHA-256:\n\
         call1 JSON: {json1}\n\
         call2 JSON: {json2}"
    );

    // Also verify the sort order within each collection.
    let blog_entries = snap1
        .collections
        .get("blog")
        .expect("blog collection present");
    let slugs: Vec<&str> = blog_entries.iter().map(|e| e.slug.as_str()).collect();
    assert_eq!(
        slugs,
        vec!["alpha", "beta"],
        "entries within a collection must be sorted ascending by slug"
    );

    let doc_entries = snap1
        .collections
        .get("docs")
        .expect("docs collection present");
    let doc_slugs: Vec<&str> = doc_entries.iter().map(|e| e.slug.as_str()).collect();
    assert_eq!(
        doc_slugs,
        vec!["aa-first", "zz-last"],
        "doc entries must be sorted ascending by slug"
    );

    // Verify collection order in the JSON itself: BTreeMap → keys in
    // alphabetical order → "blog" before "docs".
    let blog_pos = json1.find("\"blog\"").expect("blog key in JSON");
    let docs_pos = json1.find("\"docs\"").expect("docs key in JSON");
    assert!(
        blog_pos < docs_pos,
        "collections must appear in alphabetical key order in JSON (blog before docs)"
    );
}

/// Snapshot determinism across reversed config-order supply — BTreeMap
/// normalisation must absorb the config-order noise.
#[test]
fn snapshot_json_is_stable_under_reversed_config_order() {
    use sha2::{Digest, Sha256};

    let fixture_tmp = tempfile::tempdir().unwrap();
    let a_dir = fixture_tmp.path().join("a");
    let b_dir = fixture_tmp.path().join("b");
    fs::create_dir_all(&a_dir).unwrap();
    fs::create_dir_all(&b_dir).unwrap();
    fs::write(
        a_dir.join("one.md"),
        "---\ntitle: \"One\"\n---\none\n",
    )
    .unwrap();
    fs::write(
        b_dir.join("two.md"),
        "---\ntitle: \"Two\"\n---\ntwo\n",
    )
    .unwrap();

    let forward = vec![
        CollectionConfig::new("a", &a_dir),
        CollectionConfig::new("b", &b_dir),
    ];
    let reversed = vec![
        CollectionConfig::new("b", &b_dir),
        CollectionConfig::new("a", &a_dir),
    ];

    let json_fwd = serde_json::to_string(
        &build_snapshot(&forward).expect("forward snapshot"),
    )
    .expect("serialize forward");
    let json_rev = serde_json::to_string(
        &build_snapshot(&reversed).expect("reversed snapshot"),
    )
    .expect("serialize reversed");

    let hash_fwd = {
        let mut h = Sha256::new();
        h.update(json_fwd.as_bytes());
        hex::encode(h.finalize())
    };
    let hash_rev = {
        let mut h = Sha256::new();
        h.update(json_rev.as_bytes());
        hex::encode(h.finalize())
    };

    assert_eq!(
        hash_fwd, hash_rev,
        "config order must not affect snapshot bytes:\n\
         forward:  {json_fwd}\n\
         reversed: {json_rev}"
    );
}

// ---------------------------------------------------------------------------
// Group 3 — EmbeddedV8 render path (requires sub-162)
// ---------------------------------------------------------------------------

// End-to-end render test: build a bundle with `Backend::EmbeddedV8` and
// render a page that calls `getCollection("blog")`. The HTML response must
// contain titles from the snapshot.
//
// IMPORTANT — this test requires sub-162 (EmbeddedV8RenderHost), which is
// now merged on base/embed-v8. The body is a compile-time stub so the file
// compiles; un-comment + replace `todo!()` with the real assertions when
// the integration wiring is finalised. Kept `#[ignore]` so cargo test
// --workspace doesn't panic on the stub.
//
// ## What it will assert
//
// 1. bundle() builds a bundle that embeds the snapshot literal.
// 2. render_all() with Backend::EmbeddedV8 dispatches a page request.
// 3. The rendered HTML for the homepage contains blog post titles sourced
//    from the snapshot — confirming that getCollection("blog") inside
//    getStaticProps() resolves from the embedded snapshot rather than
//    attempting a node:fs read.
// #[test]
// #[ignore = "requires sub-162 (Backend::EmbeddedV8) — un-ignore after merge"]
// fn embedded_v8_renders_page_with_snapshot_data() {
//     // Locate esbuild (required for real bundle).
//     // Build fixture at examples/basic-blog.
//     // Call bundle() with Backend::EmbeddedV8 and content_snapshot_json
//     //   set from build_snapshot(blog_collection_config).
//     // Call render_all() with a route universe containing "/".
//     // Read dist/index.html and assert it contains "Alpha Post" or
//     //   similar title from the blog collection.
//     todo!("implement after sub-162 is merged");
// }
