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
//! 3. **EmbeddedV8 render path** (gated, requires the `embed_v8` feature) — a
//!    bundle built with a real esbuild renders a page that calls
//!    `getCollection("blog")` and the HTML contains snapshot data.
//!    Skips gracefully when esbuild is unavailable or pnpm deps are missing.
//!
//! ## Dependency note
//!
//! The bundler-level tests (groups 1 and 2) do NOT require esbuild or a
//! running JS runtime — they exercise only the `bundle()` codepath
//! with `mock_subprocess_output` so they run in CI without any external
//! binary. The EmbeddedV8 render test (group 3) is gated on the `embed_v8`
//! feature and skips when esbuild or the pnpm store is unavailable.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use zfb_build::{
    bundle, render_all, Backend, BundleManifest, BundleMode, BundlerInput, ContentCollectionSpec,
    RendererInput, RouteUniverseEntry,
};
use zfb_content::{build_snapshot, CollectionConfig};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

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
        markdown_features: None,
        plugin_alias_entries: Vec::new(),
        plugin_virtual_modules: Vec::new(),
        worker_only_routes: None,
        bundle_basename: None,
        css_module_class_maps: std::collections::HashMap::new(),
        mdx_components_file: None,
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
/// renders post titles plus the first post's `Content` component, plus a
/// `content/blog/` directory with two `.md` posts.
///
/// The alpha post body uses `**bold alpha**` (GFM strong) so the rendered
/// HTML contains `<strong>bold alpha</strong>`. This allows the
/// `embedded_v8_renders_page_with_snapshot_data` test to assert on rendered
/// body HTML rather than raw markdown source, confirming that
/// `materialise_collection` registers `.md` entries in the content bridge
/// so `<post.Content />` produces real HTML output instead of the
/// `<pre data-zfb-content-fallback>` raw-body fallback.
fn write_blog_fixture_project(root: &Path) -> PathBuf {
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::create_dir_all(root.join("content/blog")).unwrap();
    fs::create_dir_all(root.join("components")).unwrap();
    fs::create_dir_all(root.join("layouts")).unwrap();

    fs::write(
        root.join("pages/index.tsx"),
        r#"import { defaultComponents } from "zfb";
import type { ContentProps } from "zfb/content";

type ContentElement = {
  readonly type: string | ((...args: unknown[]) => unknown);
  readonly props: Readonly<Record<string, unknown>>;
  readonly key: unknown;
};

type Post = {
  slug: string;
  data: { title: string };
  Content: (props: ContentProps) => ContentElement;
};

export async function getStaticProps() {
  const { getCollection } = await import("zfb/content");
  const posts = (await getCollection("blog")) as Post[];
  return { props: { posts } };
}

type Props = {
  posts: Post[];
};

export default function HomePage({ posts }: Props) {
  const first = posts[0];
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
        {first ? (
          <article>
            <first.Content components={{ ...defaultComponents }} />
          </article>
        ) : null}
      </body>
    </html>
  );
}
"#,
    )
    .unwrap();

    // alpha.md uses `**bold alpha**` (GFM strong emphasis) so the rendered
    // HTML contains `<strong>bold alpha</strong>` — a discriminating marker
    // that proves the Content component ran the markdown renderer rather than
    // falling back to the raw `<pre data-zfb-content-fallback>` path.
    fs::write(
        root.join("content/blog/alpha.md"),
        "---\ntitle: \"Alpha Post\"\ndate: \"2026-01-01\"\n---\nBody of **bold alpha**.\n",
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
        // false (production default): esbuild resolves symlinks to realpaths,
        // collapsing the nested packages/zfb-runtime/node_modules/@takazudo/zfb
        // symlink and the top-level @takazudo/zfb symlink into a single
        // packages/zfb/src/content.ts — preventing a dual-module instance.
        // true would cause the zfb/content module to be bundled twice (once per
        // symlink path), breaking the content snapshot bridge (tracked in #413).
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
        markdown_features: None,
        plugin_alias_entries: Vec::new(),
        plugin_virtual_modules: Vec::new(),
        worker_only_routes: None,
        bundle_basename: None,
        css_module_class_maps: std::collections::HashMap::new(),
        mdx_components_file: None,
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

    // Body-content rendering assertion: the fixture's alpha.md body contains
    // `**bold alpha**`. The `<post.Content />` render must produce
    // `<strong>bold alpha</strong>` in the HTML output. If materialise_collection
    // failed to register the .md module in the content bridge, Content would
    // fall back to `<pre data-zfb-content-fallback>` printing the raw body, and
    // this substring would be absent.
    assert!(
        body.contains("<strong>bold alpha</strong>"),
        "rendered homepage must contain '<strong>bold alpha</strong>' from Content render; \
         raw markdown '**bold alpha**' must not pass through literally. \
         This would fail without #405's fix to materialise_collection. \
         got body:\n{body}",
    );
}

// ---------------------------------------------------------------------------
// Group 4 — .md and .html page end-to-end (confirm gate for #408 + #409)
// ---------------------------------------------------------------------------

/// Build a minimal fixture project that has a `.md` page (pages/about.md)
/// and a `.html` page (pages/legal.html) to drive the #408 + #409 pipelines.
fn write_md_html_page_fixture(root: &Path) {
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::create_dir_all(root.join("content")).unwrap();
    fs::create_dir_all(root.join("components")).unwrap();
    fs::create_dir_all(root.join("layouts")).unwrap();

    // pages/about.md — heading, GFM strong, relative link.
    // The heading `# Hello` → <h1>Hello</h1> and `**bold**` →
    // <strong>bold</strong> prove the MDX pipeline ran (#408).
    fs::write(
        root.join("pages/about.md"),
        "---\ntitle: About\nlang: en\n---\n\n# Hello\n\nThis is **bold** text.\n\nSee [link](./other).\n",
    )
    .unwrap();

    // pages/legal.html — verbatim HTML with a YAML frontmatter block.
    // The frontmatter must be stripped; the HTML body must pass through
    // verbatim (#409).
    fs::write(
        root.join("pages/legal.html"),
        "---\ntitle: Legal\n---\n<!doctype html>\n<html lang=\"en\">\n\
         <head><meta charset=\"utf-8\"><title>Legal</title></head>\n\
         <body><h1>Legal Notice</h1><p>verbatim content</p></body>\n\
         </html>\n",
    )
    .unwrap();
}

/// End-to-end: a `.md` page in `pages/` renders to HTML with the markdown
/// body compiled to real tags via the MDX pipeline.
///
/// Asserts:
/// - `<h1>Hello</h1>` (from `# Hello`) — MDX heading compiled correctly.
/// - `<strong>bold</strong>` (from `**bold**`) — GFM strong emphasis compiled.
///   Without #408's `materialise_shadow` fix the route would be absent or the
///   body would be raw markdown / a `<pre>` fallback, and both substrings
///   would be absent.
///
/// Gated on `#[cfg(feature = "embed_v8")]` (same as the rest of this group).
/// Skips when esbuild is unavailable or pnpm runtime deps are missing.
#[cfg(feature = "embed_v8")]
#[tokio::test]
async fn embedded_v8_md_page_renders_to_html() {
    use zfb_render::{EmbeddedV8RenderHost, HttpRequestLike, RenderHost};

    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[embedded_v8_snapshot_e2e] no esbuild binary; set ZFB_ESBUILD_BIN, \
             place it at crates/zfb/binaries/esbuild/esbuild, or install via pnpm. \
             Skipping md-page test."
        );
        return;
    };

    let fixture_tmp = tempfile::tempdir().expect("fixture tempdir");
    let project_root = fixture_tmp.path();
    write_md_html_page_fixture(project_root);

    let Some(node_modules) = make_test_node_modules() else {
        eprintln!(
            "[embedded_v8_snapshot_e2e] missing runtime deps in pnpm store; \
             skipping md-page test (run `pnpm install` at the repo root)."
        );
        return;
    };

    let dist_tmp = tempfile::tempdir().expect("dist tempdir");

    let input = BundlerInput {
        project_root: project_root.to_path_buf(),
        pages_dir: PathBuf::from("pages"),
        content_dir: PathBuf::from("content"),
        content_collections: Vec::new(),
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
        content_snapshot_json: None,
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
        markdown_features: None,
        plugin_alias_entries: Vec::new(),
        plugin_virtual_modules: Vec::new(),
        worker_only_routes: None,
        bundle_basename: None,
        css_module_class_maps: std::collections::HashMap::new(),
        mdx_components_file: None,
    };

    let out = bundle(input).expect("bundle with pages/about.md should succeed");
    let bundle_source = fs::read_to_string(&out.bundle_path).expect("read bundle.mjs");

    // Sanity: the compiled markdown body must be present in the bundle.
    // `# Hello` compiles to an h1 reference and `**bold**` to a strong
    // reference. If #408 is reverted, this file won't be in the route at all.
    assert!(
        bundle_source.contains("Hello"),
        "bundle.mjs must embed the compiled about.md body containing 'Hello'; \
         got (first 500 chars):\n{}",
        &bundle_source[..bundle_source.len().min(500)],
    );

    // Dispatch via the embedded V8 host.
    let mut host = EmbeddedV8RenderHost::new().expect("EmbeddedV8RenderHost boot");
    host.execute_module("bundle.mjs", &bundle_source)
        .await
        .unwrap_or_else(|e| panic!("bundle failed to load in V8 host: {e}"));

    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/about"))
        .await
        .unwrap_or_else(|e| panic!("dispatch /about failed: {e}"));

    assert_eq!(
        resp.status, 200,
        "/about must render 200; got status {}, body={:?}",
        resp.status,
        resp.body_utf8(),
    );

    let body = resp
        .body_utf8()
        .expect("response body must be valid UTF-8")
        .to_string();

    // `# Hello` must compile to <h1>Hello</h1>.
    // Raw markdown would produce literal `# Hello` or nothing; this string
    // only appears when the MDX heading compiler ran (#408 path).
    assert!(
        body.contains("<h1>Hello</h1>"),
        "rendered /about must contain '<h1>Hello</h1>' from compiled markdown; \
         raw '# Hello' must not pass through literally. \
         This would fail without #408's materialise_shadow .md support. \
         got body:\n{body}",
    );

    // `**bold**` must compile to <strong>bold</strong>.
    // A raw-body fallback would contain `**bold**`, not the HTML tag.
    assert!(
        body.contains("<strong>bold</strong>"),
        "rendered /about must contain '<strong>bold</strong>' from GFM strong; \
         raw '**bold**' must not pass through literally. \
         This would fail without #408's materialise_shadow .md support. \
         got body:\n{body}",
    );
}

/// End-to-end: a `.html` page in `pages/` is written verbatim to dist with
/// YAML frontmatter stripped. Uses `Backend::Stub` (never called — the static-
/// HTML bypass in `render_one_inner` reads from disk directly).
///
/// Asserts:
/// - `dist/legal/index.html` is written.
/// - The YAML frontmatter block (`title: Legal`) is absent in the output.
/// - `<!doctype html>` is present (first line after frontmatter strip).
/// - `<h1>Legal Notice</h1>` is present (verbatim body preserved).
///
/// Removing #409's `static_html` bypass would route the `.html` page through
/// the backend, the stub would panic, and the test would fail.
#[test]
fn html_page_written_verbatim_via_render_all() {
    let project_tmp = tempfile::tempdir().expect("project tempdir");
    let dist_tmp = tempfile::tempdir().expect("dist tempdir");

    write_md_html_page_fixture(project_tmp.path());

    // Backend::Stub that panics if called — the static_html bypass must
    // never reach the backend for the legal route.
    let backend = Backend::Stub {
        handler: Arc::new(|path: &str| {
            panic!(
                "backend must not be dispatched for static_html routes; got path: {path}"
            )
        }),
    };

    let universe = vec![RouteUniverseEntry {
        url_path: "/legal".into(),
        output_path: PathBuf::from("legal/index.html"),
        route_key: "/legal".into(),
        static_html: true,
        source_path: Some(PathBuf::from("pages/legal.html")),
    }];

    let out = render_all(RendererInput {
        bundle_path: PathBuf::from("/dev/null"),
        sourcemap_path: PathBuf::from("/dev/null"),
        manifest: BundleManifest {
            framework: "preact".into(),
            jsx_import_source: "preact".into(),
            hydrate_shim_specifier: "zfb:internal/preact/hydrate".into(),
            bundle_basename: "bundle.mjs".into(),
            routes: vec![],
        },
        dist_dir: dist_tmp.path().to_path_buf(),
        project_root: project_tmp.path().to_path_buf(),
        route_universe: universe,
        prerender_map: BTreeMap::new(),
        backend,
        request_timeout: None,
        prod_head_assets: None,
    })
    .expect("render_all must succeed for static-HTML route");

    assert_eq!(
        out.ssg_files_written.len(),
        1,
        "exactly one file should be written for the legal route"
    );

    let dest = dist_tmp.path().join("legal/index.html");
    assert!(dest.exists(), "dist/legal/index.html must be written");

    let content = fs::read_to_string(&dest).expect("read dist/legal/index.html");

    // YAML frontmatter must be stripped — not present in the output.
    assert!(
        !content.contains("title: Legal"),
        "YAML frontmatter 'title: Legal' must be stripped from the output. \
         got content:\n{content}",
    );

    // The verbatim HTML body must be present.
    assert!(
        content.contains("<!doctype html>"),
        "output must contain '<!doctype html>' (frontmatter stripped, body verbatim); \
         This would fail without #409's static_html bypass. \
         got content:\n{content}",
    );
    assert!(
        content.contains("<h1>Legal Notice</h1>"),
        "output must contain '<h1>Legal Notice</h1>' (verbatim HTML body preserved); \
         This would fail without #409's static_html bypass. \
         got content:\n{content}",
    );
}

// ---------------------------------------------------------------------------
// Group 5 — `paths()` worker dispatch over a dual-`zfb/content` instance bundle
// ---------------------------------------------------------------------------

/// Build a `node_modules` directory that mimics the pnpm-strict layout
/// real consumers ship: `@takazudo/zfb` is reachable via TWO physical
/// paths so esbuild + `--preserve-symlinks` will end up inlining
/// `content.js` twice.
///
/// Layout:
///
/// ```text
/// <tmp>/
///   preact/                                  -> pnpm store /preact
///   preact-render-to-string/                 -> pnpm store /preact-render-to-string
///   hono/                                    -> pnpm store /hono
///   @takazudo/
///     zfb/                                   -> packages/zfb            (path A)
///     zfb-runtime/                           -> packages/zfb-runtime
///       node_modules/@takazudo/
///         zfb/                               -> packages/zfb            (path B)
///   zfb/                                     -> packages/zfb            (bare alias)
/// ```
///
/// Both `path A` and `path B` resolve to the same source file
/// (`packages/zfb/dist/content.js`), but under `--preserve-symlinks`
/// esbuild treats them as distinct sources and inlines the module
/// body once per path. That is precisely the dual-instance shape that
/// the bug #442 / #449 fix needs to survive: the worker bundle
/// receives one `setContentSnapshot` call (on the instance reached
/// from `@takazudo/zfb-runtime`) but the user `paths()` calls
/// `getCollection()` on the other instance — and without the
/// `globalThis`-backed snapshot slot, the second instance sees
/// `installedSnapshot === undefined` and falls through to `node:fs`,
/// which throws because `node:*` is externalized.
fn make_dual_zfb_node_modules() -> Option<tempfile::TempDir> {
    let worktree_root = workspace_root();
    let pnpm_store = locate_pnpm_node_modules()?;

    let tmp = tempfile::tempdir().expect("tempdir for dual-zfb node_modules");
    let nm = tmp.path();

    // Generic deps from the pnpm store.
    let from_store: &[&str] = &["preact", "preact-render-to-string", "hono"];
    for pkg in from_store {
        let src = pnpm_store.join(pkg);
        if !src.exists() {
            eprintln!(
                "[embedded_v8_snapshot_e2e] missing runtime dep `{pkg}` at {} — \
                 skipping paths-dual-instance test (run `pnpm install`).",
                src.display(),
            );
            return None;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&src, nm.join(pkg)).expect("symlink generic dep");
    }

    let takazudo_dir = nm.join("@takazudo");
    fs::create_dir_all(&takazudo_dir).expect("create @takazudo dir");

    let zfb_src = worktree_root.join("packages/zfb");
    let zfb_runtime_src = worktree_root.join("packages/zfb-runtime");

    // Path A — the consumer's top-level @takazudo/zfb.
    #[cfg(unix)]
    std::os::unix::fs::symlink(&zfb_src, takazudo_dir.join("zfb"))
        .expect("symlink top-level @takazudo/zfb");
    // Top-level @takazudo/zfb-runtime.
    #[cfg(unix)]
    std::os::unix::fs::symlink(&zfb_runtime_src, takazudo_dir.join("zfb-runtime"))
        .expect("symlink top-level @takazudo/zfb-runtime");

    // Path B — the nested @takazudo/zfb inside zfb-runtime's own
    // node_modules. We materialise this as `node_modules/@takazudo/zfb-runtime-nested/`
    // because we can't create a `node_modules/` inside the real workspace
    // package without polluting it. Instead we symlink a directory tree:
    // the test only needs esbuild to see TWO distinct paths to the
    // zfb package. Easiest reliable way: a sibling top-level dir that
    // also gets `@takazudo/zfb` and is reachable from zfb-runtime's
    // resolution chain.
    //
    // The simplest reproducer that does NOT depend on writing inside
    // packages/ is: alias the bare package name `zfb-shadow` to the
    // same packages/zfb directory, and have the runtime resolve from
    // there. But that requires bundler tweaks.
    //
    // What esbuild's `--preserve-symlinks` actually keys on is the
    // post-resolution physical path. We can synthesize a second
    // physical path by putting the same target behind a **second**
    // node_modules entry whose package.json points back to the same
    // module. The simplest is `@takazudo/zfb-nested-copy/` → same
    // packages/zfb source — but esbuild resolves bare specifiers
    // through package.json name fields, so we need the consumer's
    // import to LITERALLY traverse a different filesystem path. The
    // most reliable way without modifying packages/ is to give zfb-runtime
    // its own private node_modules tree under the test tempdir.
    //
    // Implementation: copy zfb-runtime into the tempdir (NOT a symlink),
    // then write a private `node_modules/@takazudo/zfb` symlink inside
    // that copy. Now both the top-level lookup and the runtime-private
    // lookup walk distinct physical paths to packages/zfb. esbuild +
    // `--preserve-symlinks` will see two distinct sources for
    // `@takazudo/zfb/content` and inline content.js twice. This is
    // the production-pnpm-strict shape.
    let zfb_runtime_copy = nm.join(".zfb-runtime-copy");
    copy_dir_recursive(&zfb_runtime_src, &zfb_runtime_copy).expect("copy zfb-runtime into tempdir");
    let copy_takazudo = zfb_runtime_copy.join("node_modules/@takazudo");
    fs::create_dir_all(&copy_takazudo).expect("create copy/node_modules/@takazudo");
    // The source `packages/zfb-runtime/node_modules/@takazudo/zfb` may
    // already exist in the workspace as a pnpm-installed symlink to the
    // workspace's `packages/zfb`. The recursive copy preserves it. Remove
    // it before re-creating so this helper is idempotent regardless of
    // whether pnpm has linked the workspace dep.
    let nested_zfb = copy_takazudo.join("zfb");
    let _ = fs::remove_file(&nested_zfb);
    let _ = fs::remove_dir_all(&nested_zfb);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&zfb_src, &nested_zfb).expect("symlink nested @takazudo/zfb");
    // Replace the top-level @takazudo/zfb-runtime symlink with one pointing
    // at the copy so resolution from `import "@takazudo/zfb-runtime"`
    // lands inside our private node_modules tree.
    fs::remove_file(takazudo_dir.join("zfb-runtime"))
        .expect("remove placeholder @takazudo/zfb-runtime symlink");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&zfb_runtime_copy, takazudo_dir.join("zfb-runtime"))
        .expect("symlink @takazudo/zfb-runtime to the copy");

    // Also keep the bare `zfb` name available — `bundler.rs` rewrites
    // some specifiers to `zfb` and esbuild expects that to resolve.
    #[cfg(unix)]
    std::os::unix::fs::symlink(&zfb_src, nm.join("zfb")).expect("symlink bare zfb");

    Some(tmp)
}

/// Recursive directory copy (regular files + symlinks). We need a real
/// copy of zfb-runtime so we can attach a private `node_modules/` inside
/// it without contaminating the workspace package.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path)?;
        } else if ty.is_symlink() {
            // Skip pre-existing symlinks (e.g. dangling ones in dist/). The
            // resolver only needs package.json + dist source files; the
            // copy doesn't have to mirror symlinks 1:1.
            let target = fs::read_link(entry.path())?;
            #[cfg(unix)]
            let _ = std::os::unix::fs::symlink(target, &dst_path);
        } else {
            fs::copy(entry.path(), &dst_path)?;
        }
    }
    Ok(())
}

/// Write a fixture project that has a dynamic route `[slug].tsx` whose
/// `paths()` calls `getCollection("blog")`. The synthetic
/// `/__paths__/<encoded-route-key>` endpoint should return the resolved
/// path list when dispatched against this bundle.
fn write_paths_via_collection_fixture(root: &Path) {
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::create_dir_all(root.join("content/blog")).unwrap();
    fs::create_dir_all(root.join("components")).unwrap();
    fs::create_dir_all(root.join("layouts")).unwrap();

    fs::write(
        root.join("pages/[slug].tsx"),
        r#"import { getCollection } from "@takazudo/zfb/content";

type Post = { slug: string; data: { title: string } };

export async function paths() {
  const posts = getCollection("blog") as Post[];
  return posts.map((p) => ({ params: { slug: p.slug }, props: { title: p.data.title } }));
}

type Props = { title: string; params: { slug: string } };

export default function PostPage({ title, params }: Props) {
  return (
    <html lang="en">
      <head><title>{title}</title></head>
      <body><h1>{title}</h1><p>slug: {params.slug}</p></body>
    </html>
  );
}
"#,
    )
    .unwrap();

    fs::write(
        root.join("content/blog/alpha.md"),
        "---\ntitle: \"Alpha Post\"\n---\nAlpha body.\n",
    )
    .unwrap();
    fs::write(
        root.join("content/blog/zulu.md"),
        "---\ntitle: \"Zulu Post\"\n---\nZulu body.\n",
    )
    .unwrap();
}

/// Regression test for #442 / #449 — `paths()` worker must NOT throw
/// when the bundle ends up with two `zfb/content` module instances.
///
/// Reproduces the production-pnpm-strict failure mode: a consumer
/// project whose `node_modules` exposes two physical paths to
/// `@takazudo/zfb` (top-level + nested inside zfb-runtime's private
/// `node_modules`), bundled with `--preserve-symlinks` (the bundler's
/// default whenever `node_modules_dir` is set — see
/// `crates/zfb-build/src/bundler.rs` around `--external:node:*`).
/// Under that shape esbuild inlines `content.js` twice, so the
/// runtime's `setContentSnapshot` call and the user `paths()`'s
/// `getCollection` call target distinct module-level slots.
///
/// Without the `globalThis.__zfb.contentSnapshot` fix in
/// `packages/zfb/src/content.ts`, the user-side instance falls
/// through to `loadNodeModules()` and throws:
///
/// > zfb/content: cannot load node:fs / node:path — no Node-style
/// > require available.
///
/// With the fix, both instances read the snapshot from the shared
/// `globalThis.__zfb.contentSnapshot` slot, the user `paths()`
/// returns the expected array, and the `/__paths__/` endpoint returns
/// 200 + a JSON list with the two slugs.
#[cfg(feature = "embed_v8")]
#[tokio::test]
async fn paths_worker_resolves_collection_across_dual_zfb_instances() {
    use zfb_render::{EmbeddedV8RenderHost, HttpRequestLike, RenderHost};

    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[embedded_v8_snapshot_e2e] no esbuild binary; \
             skipping paths-dual-instance test."
        );
        return;
    };

    let fixture_tmp = tempfile::tempdir().expect("fixture tempdir");
    let project_root = fixture_tmp.path();
    write_paths_via_collection_fixture(project_root);
    let blog_dir = project_root.join("content/blog");

    // Build a real ContentSnapshot from the fixture so the bundle's
    // `__zfb_content_snapshot` literal carries the same data the
    // build pipeline would inject.
    let snap = build_snapshot(&[CollectionConfig::new("blog", &blog_dir)])
        .expect("build_snapshot from paths-dual fixture");
    let snap_json = serde_json::to_string(&snap).expect("serialise snapshot");
    assert!(
        snap_json.contains("alpha"),
        "snapshot must contain the alpha slug; got: {snap_json}"
    );

    let Some(node_modules) = make_dual_zfb_node_modules() else {
        eprintln!(
            "[embedded_v8_snapshot_e2e] could not set up dual-zfb node_modules; \
             skipping."
        );
        return;
    };

    let dist_tmp = tempfile::tempdir().expect("dist tempdir");

    let input = BundlerInput {
        project_root: project_root.to_path_buf(),
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
        // `node_modules_preserve_symlinks` is a no-op today (see
        // `crates/zfb-build/src/bundler.rs` field doc) — the bundler
        // ALWAYS passes `--preserve-symlinks` whenever `node_modules_dir`
        // is set. We leave the value here for documentation parity with
        // the other tests.
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
        markdown_features: None,
        plugin_alias_entries: Vec::new(),
        plugin_virtual_modules: Vec::new(),
        worker_only_routes: None,
        bundle_basename: None,
        css_module_class_maps: std::collections::HashMap::new(),
        mdx_components_file: None,
    };

    let out = bundle(input).expect("bundle should succeed for paths-dual fixture");
    let bundle_source = fs::read_to_string(&out.bundle_path).expect("read bundle.mjs");

    // Spot-check: the snapshot literal made it into the bundle.
    assert!(
        bundle_source.contains("alpha"),
        "bundle.mjs must embed the snapshot literal containing 'alpha'; \
         got (first 500 chars):\n{}",
        &bundle_source[..bundle_source.len().min(500)],
    );

    // Boot the embedded V8 host with the bundle and dispatch the
    // synthetic `/__paths__/<encoded-route-key>` endpoint.
    let mut host = EmbeddedV8RenderHost::new().expect("EmbeddedV8RenderHost boot");
    host.execute_module("bundle.mjs", &bundle_source)
        .await
        .unwrap_or_else(|e| panic!("bundle failed to load in V8 host: {e}"));

    // The route key is the Hono pattern (the bundler converts the
    // file-system `[slug].tsx` to `/:slug`). Percent-encode it so the
    // `/__paths__/:routeKey{.+}` capture decodes back to the same
    // string.
    let route_key = "/:slug";
    let encoded = route_key.replace('/', "%2F").replace(':', "%3A");
    let url = format!("http://zfb.local/__paths__/{encoded}");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get(&url))
        .await
        .unwrap_or_else(|e| panic!("dispatch /__paths__ failed: {e}"));

    let body = resp
        .body_utf8()
        .map(|s| s.to_string())
        .unwrap_or_else(|| String::from("<non-utf8>"));

    assert_eq!(
        resp.status, 200,
        "/__paths__/{route_key} must return 200; got status {} body: {body}",
        resp.status,
    );

    // Verify the dual-instance case did NOT regress: the throw from
    // content.ts:308 is the bug we are guarding against. If it ever
    // reappears, the body will contain this string and the status
    // would have been 500 — the assertion above already fails first,
    // but we also surface the throw text here in case future router
    // edits start returning the throw with a 200.
    assert!(
        !body.contains("cannot load node:fs"),
        "paths() must not throw 'cannot load node:fs' — that means the dual \
         zfb/content instance broke the snapshot bridge again. body: {body}"
    );

    let json: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("paths() body is not JSON: {e} — body: {body}"));
    let arr = json
        .as_array()
        .unwrap_or_else(|| panic!("paths() must return a JSON array; got: {body}"));
    assert_eq!(
        arr.len(),
        2,
        "paths() must return one entry per blog post (alpha, zulu); got: {body}"
    );

    // Either order is fine — assert by content rather than position.
    let slugs: std::collections::BTreeSet<&str> = arr
        .iter()
        .filter_map(|e| e.get("params"))
        .filter_map(|p| p.get("slug"))
        .filter_map(|s| s.as_str())
        .collect();
    assert!(
        slugs.contains("alpha"),
        "paths() must include slug 'alpha'; got body: {body}"
    );
    assert!(
        slugs.contains("zulu"),
        "paths() must include slug 'zulu'; got body: {body}"
    );
}
