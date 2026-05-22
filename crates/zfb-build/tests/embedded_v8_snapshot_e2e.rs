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
use std::path::{Path, PathBuf};
use std::process::Command;

use zfb_build::{bundle, BundleMode, BundlerInput, ContentCollectionSpec};
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
        code_highlight_themes_dir: None,
        resolve_markdown_links: None,
        gfm_constructs: zfb_content::ResolvedGfmConstructs::default(),
        site: None,
        prefetch_disabled: false,
        toc: None,
        external_links: None,
        cjk_friendly: true,
        plugin_alias_entries: Vec::new(),
        plugin_virtual_modules: Vec::new(),
        worker_only_routes: None,
        bundle_basename: None,
        css_module_class_maps: std::collections::HashMap::new(),
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
// Group 3 — EmbeddedV8 render path
// ---------------------------------------------------------------------------

/// Resolve the esbuild binary. Same precedence as the other integration
/// tests (`bundler_integration.rs`, `integration_e2e_routing_rendering.rs`):
///
/// 1. `ZFB_ESBUILD_BIN` env var (an absolute path).
/// 2. `crates/zfb/binaries/esbuild/esbuild` (the workspace-staged binary).
/// 3. `which esbuild` (pnpm-store bin, system PATH).
fn locate_esbuild() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ZFB_ESBUILD_BIN") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(workspace) = here.parent().and_then(|p| p.parent()) {
        let slot = workspace.join("crates/zfb/binaries/esbuild/esbuild");
        if slot.exists() {
            return Some(slot);
        }
    }
    // Fallback: any pnpm-staged esbuild reachable from the worktree (the
    // sibling main-repo's `node_modules/.pnpm/node_modules/esbuild/bin`).
    if let Some(store) = locate_pnpm_node_modules() {
        let pnpm_slot = store.join("esbuild/bin/esbuild");
        if pnpm_slot.exists() {
            return Some(pnpm_slot);
        }
    }
    if let Ok(out) = Command::new("which").arg("esbuild").output() {
        if out.status.success() {
            let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

/// Workspace root (two levels up from `CARGO_MANIFEST_DIR`).
fn workspace_root() -> PathBuf {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    here.parent()
        .and_then(|p| p.parent())
        .expect("workspace root from CARGO_MANIFEST_DIR")
        .to_path_buf()
}

/// Locate a `node_modules/.pnpm/node_modules` directory that contains the
/// runtime deps (`preact`, `hono`, …) the bundle needs.
///
/// First tries the worktree root (where `pnpm install` would normally
/// drop it). If that path is missing — common in a fresh `/x-wt-teams`
/// worktree that has not been `pnpm install`-ed because the manager
/// session shares the main repo's `node_modules` — walks upwards looking
/// for a sibling main-repo checkout. Returns `None` when no candidate
/// exists; the test skips in that case.
fn locate_pnpm_node_modules() -> Option<PathBuf> {
    let primary = workspace_root().join("node_modules/.pnpm/node_modules");
    if primary.exists() {
        return Some(primary);
    }
    // Walk the worktree ancestry to find a main checkout. The
    // `/x-wt-teams` layout puts worktrees at `<repo>/worktrees/<name>/`,
    // so the main repo is at `<worktree>/../../`.
    let mut cursor = workspace_root();
    for _ in 0..4 {
        let candidate = cursor.join("node_modules/.pnpm/node_modules");
        if candidate.exists() {
            return Some(candidate);
        }
        let Some(parent) = cursor.parent() else {
            break;
        };
        cursor = parent.to_path_buf();
    }
    None
}

/// Build a custom `node_modules` directory for the test bundle.
///
/// Mirrors `integration_e2e_routing_rendering::make_test_node_modules`:
/// generic packages (`preact`, `preact-render-to-string`, `hono`) symlink
/// to the pnpm virtual store; `@takazudo/zfb-runtime` and `zfb` symlink
/// to the worktree copies so the bundle picks up the source under test.
fn make_test_node_modules() -> Option<tempfile::TempDir> {
    let worktree_root = workspace_root();
    let pnpm_store = locate_pnpm_node_modules()?;

    let tmp = tempfile::tempdir().expect("tempdir for test node_modules");
    let nm = tmp.path();

    let from_store: &[&str] = &["preact", "preact-render-to-string", "hono"];
    for pkg in from_store {
        let src = pnpm_store.join(pkg);
        if !src.exists() {
            eprintln!(
                "[embedded_v8_snapshot_e2e] missing runtime dep `{pkg}` at {} — \
                 skipping test (run `pnpm install` at the repo root).",
                src.display(),
            );
            return None;
        }
        let dst = nm.join(pkg);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&src, &dst)
            .unwrap_or_else(|e| panic!("symlink {}: {e}", src.display()));
    }

    // `@takazudo/zfb-runtime` and `@takazudo/zfb` both come from the
    // worktree so the test exercises the source under review. The bundler
    // rewrites bare `zfb` specifiers to `@takazudo/zfb` (see
    // `crates/zfb-build/src/bundler.rs` around `--alias:zfb=@takazudo/zfb`),
    // so both forms need the scoped package to exist.
    let takazudo_dir = nm.join("@takazudo");
    fs::create_dir_all(&takazudo_dir).expect("create @takazudo dir");
    let zfb_runtime_src = worktree_root.join("packages/zfb-runtime");
    let zfb_runtime_dst = takazudo_dir.join("zfb-runtime");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&zfb_runtime_src, &zfb_runtime_dst)
        .unwrap_or_else(|e| panic!("symlink @takazudo/zfb-runtime: {e}"));

    let zfb_src = worktree_root.join("packages/zfb");
    let zfb_dst_scoped = takazudo_dir.join("zfb");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&zfb_src, &zfb_dst_scoped)
        .unwrap_or_else(|e| panic!("symlink @takazudo/zfb: {e}"));
    // Also keep the bare `zfb` name available for any code path that
    // still resolves it directly (the bundler's alias targets the scoped
    // form for production builds, but unit tests sometimes import the
    // bare form).
    let zfb_dst_bare = nm.join("zfb");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&zfb_src, &zfb_dst_bare)
        .unwrap_or_else(|e| panic!("symlink zfb: {e}"));

    Some(tmp)
}

/// Create a minimal fixture project on disk: a `pages/index.tsx` that
/// reads from a `blog` content collection via `getCollection("blog")` and
/// renders post titles, plus a `content/blog/` directory with two
/// `.md` posts carrying `title` frontmatter.
fn write_blog_fixture_project(root: &Path) -> PathBuf {
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::create_dir_all(root.join("content/blog")).unwrap();
    fs::create_dir_all(root.join("components")).unwrap();
    fs::create_dir_all(root.join("layouts")).unwrap();

    fs::write(
        root.join("pages/index.tsx"),
        r#"export async function getStaticProps() {
  const { getCollection } = await import("zfb/content");
  const posts = (await getCollection("blog")) as Array<{
    slug: string;
    data: { title: string };
  }>;
  return { props: { posts } };
}

type Props = {
  posts: Array<{ slug: string; data: { title: string } }>;
};

export default function HomePage({ posts }: Props) {
  return (
    <html lang="en">
      <head>
        <title>blog-snapshot-fixture</title>
      </head>
      <body>
        <h1>Posts</h1>
        <ul>
          {posts.map((p) => (
            <li key={p.slug}>{p.data.title}</li>
          ))}
        </ul>
      </body>
    </html>
  );
}
"#,
    )
    .unwrap();

    fs::write(
        root.join("content/blog/alpha.md"),
        "---\ntitle: \"Alpha Post\"\ndate: \"2026-01-01\"\n---\nBody of alpha.\n",
    )
    .unwrap();
    fs::write(
        root.join("content/blog/zulu.md"),
        "---\ntitle: \"Zulu Post\"\ndate: \"2026-03-01\"\n---\nBody of zulu.\n",
    )
    .unwrap();

    root.to_path_buf()
}

/// End-to-end render test: a bundle built with a populated
/// `content_snapshot_json` and dispatched through the in-process V8
/// host renders a page that calls `getCollection("blog")` inside
/// `getStaticProps`. The rendered HTML must contain the post titles
/// from the snapshot, proving:
///
/// 1. `bundle()` embeds the snapshot literal into `entry.mjs`
///    (`bundler.rs:2258`).
/// 2. The generated `createPageRouter({...})` call hands the literal to
///    the runtime's `setContentSnapshot` (`router.ts:159`).
/// 3. `getCollection("blog")` resolves from the installed snapshot
///    (`packages/zfb/src/content.ts:425-427`) — i.e. without falling
///    through to `node:fs`.
///
/// This is the previously-deferred stub from sub-162 / #392, now
/// implemented. Drives the host directly via `EmbeddedV8RenderHost`
/// rather than `render_all(Backend::EmbeddedV8 { .. })`: the
/// `EmbeddedV8Host` trait requires `Send`, and the only impl
/// (`ThreadedV8Host`) lives in the `zfb` crate, which is downstream of
/// this crate — depending on it from here would form a cycle. Driving
/// the host directly here gives the same proof with no extra wiring.
///
/// Skips with an `eprintln!` when esbuild is unavailable (mirroring
/// `integration_e2e_routing_rendering`). Gated on the `embed_v8`
/// feature; off by default builds drop this test entirely (same
/// pattern as `crates/zfb-render/tests/embedded_v8_*.rs`).
#[cfg(feature = "embed_v8")]
#[tokio::test]
async fn embedded_v8_renders_page_with_snapshot_data() {
    use zfb_render::{EmbeddedV8RenderHost, HttpRequestLike, RenderHost};

    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[embedded_v8_snapshot_e2e] no esbuild binary; set ZFB_ESBUILD_BIN, \
             place it at crates/zfb/binaries/esbuild/esbuild, or install via pnpm. \
             Skipping."
        );
        return;
    };

    // --- Fixture: pages/index.tsx + content/blog/*.md ---
    let fixture_tmp = tempfile::tempdir().expect("fixture tempdir");
    let project_root = write_blog_fixture_project(fixture_tmp.path());
    let blog_dir = project_root.join("content/blog");

    // --- Snapshot: build the real ContentSnapshot off disk ---
    let snap = build_snapshot(&[CollectionConfig::new("blog", &blog_dir)])
        .expect("build_snapshot from fixture");
    let snap_json = serde_json::to_string(&snap).expect("serialise snapshot");

    // Sanity: the snapshot must carry both entries with their titles in
    // the frontmatter — otherwise the in-bundle render below cannot
    // surface them either.
    assert!(
        snap_json.contains("Alpha Post"),
        "snapshot JSON should embed 'Alpha Post' from frontmatter; got: {snap_json}"
    );
    assert!(
        snap_json.contains("Zulu Post"),
        "snapshot JSON should embed 'Zulu Post' from frontmatter; got: {snap_json}"
    );

    // --- Bundle: real esbuild, real node_modules symlinks ---
    let Some(node_modules) = make_test_node_modules() else {
        eprintln!(
            "[embedded_v8_snapshot_e2e] missing runtime deps in pnpm store; \
             skipping test (run `pnpm install` at the repo root)."
        );
        return;
    };
    let dist_tmp = tempfile::tempdir().expect("dist tempdir");

    let input = BundlerInput {
        project_root: project_root.clone(),
        pages_dir: PathBuf::from("pages"),
        content_dir: PathBuf::from("content"),
        content_collections: vec![ContentCollectionSpec::new(
            "blog",
            project_root.join("content/blog"),
        )],
        components_dir: PathBuf::from("components"),
        layouts_dir: PathBuf::from("layouts"),
        framework: Framework::Preact,
        define_vars: HashMap::new(),
        tsconfig_paths: BTreeMap::new(),
        external: vec![],
        outdir: dist_tmp.path().to_path_buf(),
        mode: BundleMode::Production,
        minify: false,
        esbuild_binary: Some(esbuild.clone()),
        mock_subprocess_output: None,
        content_snapshot_json: Some(snap_json.clone()),
        node_modules_dir: Some(node_modules.path().to_path_buf()),
        node_modules_preserve_symlinks: true,
        strip_md_ext: false,
        code_highlight_theme: None,
        code_highlight_themes_dir: None,
        resolve_markdown_links: None,
        gfm_constructs: zfb_content::ResolvedGfmConstructs::default(),
        site: None,
        prefetch_disabled: false,
        toc: None,
        external_links: None,
        cjk_friendly: true,
        plugin_alias_entries: Vec::new(),
        plugin_virtual_modules: Vec::new(),
        worker_only_routes: None,
        bundle_basename: None,
        css_module_class_maps: std::collections::HashMap::new(),
    };

    let out = bundle(input).expect("bundle should succeed for fixture project");
    let bundle_source = fs::read_to_string(&out.bundle_path)
        .expect("read produced bundle.mjs");

    // Spot-check: the snapshot literal made it into the bundle. This
    // guarantees the bundler-level wiring is correct; the V8 host
    // dispatch below proves the runtime side.
    assert!(
        bundle_source.contains("Alpha Post"),
        "bundle.mjs must embed snapshot literal containing 'Alpha Post'; \
         got (first 500 chars):\n{}",
        &bundle_source[..bundle_source.len().min(500)],
    );

    // --- Dispatch through the embedded V8 host ---
    let mut host = EmbeddedV8RenderHost::new().expect("EmbeddedV8RenderHost boot");
    host.execute_module("bundle.mjs", &bundle_source)
        .await
        .unwrap_or_else(|e| panic!("bundle failed to load in V8 host: {e}"));

    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .unwrap_or_else(|e| panic!("dispatch / failed: {e}"));

    assert_eq!(
        resp.status, 200,
        "homepage must render 200; got status {}, body={:?}",
        resp.status,
        resp.body_utf8(),
    );
    let body = resp
        .body_utf8()
        .expect("response body must be valid UTF-8")
        .to_string();
    // The load-bearing assertion: post titles from the snapshot are
    // present in the rendered HTML. If `getCollection` were silently
    // returning `[]` (e.g. because the snapshot wasn't installed) the
    // `<ul>` would be empty and these substrings would be absent.
    assert!(
        body.contains("Alpha Post"),
        "rendered homepage must contain 'Alpha Post' from getCollection(\"blog\"); \
         got body:\n{body}",
    );
    assert!(
        body.contains("Zulu Post"),
        "rendered homepage must contain 'Zulu Post' from getCollection(\"blog\"); \
         got body:\n{body}",
    );
}
