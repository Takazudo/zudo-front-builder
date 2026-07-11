//! Issue #1551 — build/check/snapshot verification for collections whose
//! configured root lives OUTSIDE the project directory (`allowOutsideRoot`,
//! landed for config validation in #1549).
//!
//! Every consumer this test drives resolves a collection path by joining it
//! onto `project_root` (or the equivalent shadow root) and then stripping
//! that exact same prefix back off during the walk — `PathResolver::resolve`
//! / `materialise_collection`'s `strip_prefix` in
//! `crates/zfb-build/src/bundler.rs`, and `parse_entry`'s `strip_prefix` in
//! `crates/zfb-content/src/collection.rs`. A `..`-relative root never gets
//! normalised, but it doesn't need to: the joined root and every entry
//! discovered under it share the identical literal prefix, so slug
//! derivation and shadow placement are unaffected by the collection living
//! above `project_root`. This test proves that structural claim end-to-end
//! instead of taking it on faith.
//!
//! ## Fixture
//!
//! `tests/fixtures/collections-outside-root/`:
//! - `shared-content/posts/hello.mdx` — the out-of-root entry, above
//!   `project/`.
//! - `shared-content/posts/notes.md` — a same-directory sibling with a
//!   *recognised* collection extension (`.md`) that the collection's
//!   `include: ["**/*.mdx"]` glob does not match. Proves include-filtering
//!   is still applied to out-of-root roots, not just extension sniffing.
//! - `project/zfb.config.json` — declares the `posts` collection with
//!   `path: "../shared-content/posts"` and `allowOutsideRoot: true`.
//! - `project/pages/index.tsx`, `project/layouts/default.tsx` — minimal
//!   scaffolding so `zfb_build::bundle_with_session` has a valid pages_dir.
//!
//! ## What this test does NOT cover
//!
//! No V8 / SSR render path is exercised (this container cannot build the
//! V8-embedding `zfb` binary — see the worktree env constraints). The
//! bundler is driven with `mock_subprocess_output` so no esbuild binary is
//! required either; the assertions below only need the shadow-tree
//! materialisation step, which runs before esbuild is ever invoked.

use std::env;
use std::path::{Path, PathBuf};

use zfb::cli::CheckArgs;
use zfb::commands::check;
use zfb::config;
use zfb_build::{
    bundle_with_session, BundleMode, BundlerInput, ContentCollectionSpec, ShadowSession,
};
use zfb_content::{build_snapshot_with_config, CollectionConfig, PipelineSpec};
use zfb_render::adapters::Framework;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("collections-outside-root")
}

/// Restores the process cwd on drop. `zfb::commands::check::run` reads
/// `env::current_dir()` internally (it has no project-root parameter), so
/// driving it in-process means temporarily chdir-ing into the fixture. Safe
/// here because this file declares exactly one `#[tokio::test]`, so there is
/// no other test in this binary that could observe the changed cwd.
struct CwdGuard {
    previous: PathBuf,
}

impl CwdGuard {
    fn enter(dir: &Path) -> Self {
        let previous = env::current_dir().expect("read current dir");
        env::set_current_dir(dir).expect("chdir into fixture project root");
        Self { previous }
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = env::set_current_dir(&self.previous);
    }
}

#[tokio::test]
async fn out_of_root_collection_passes_check_snapshot_and_build_materialisation() {
    let fixture_root = fixture_dir();
    let project_root = fixture_root.join("project");
    assert!(
        project_root.is_dir(),
        "fixture project root missing: {}",
        project_root.display()
    );

    // ---------------------------------------------------------------
    // 1. `zfb check` — the in-process check entrypoint.
    // ---------------------------------------------------------------
    {
        let _cwd = CwdGuard::enter(&project_root);
        let result = check::run(&CheckArgs { skip_tsc: true }).await;
        assert!(
            result.is_ok(),
            "zfb check must pass for an allowOutsideRoot collection: {:?}",
            result.err()
        );
    }

    // ---------------------------------------------------------------
    // 2. Content snapshot — correct slugs, include-filtering applied.
    // ---------------------------------------------------------------
    let cfg = config::load_from_dir(&project_root)
        .await
        .expect("load zfb.config.json");
    assert_eq!(cfg.collections.len(), 1);
    assert!(
        cfg.collections[0].allow_outside_root,
        "fixture config must set allowOutsideRoot: true"
    );

    // Mirrors `build_content_snapshot_json`'s `CollectionConfig` assembly in
    // crates/zfb/src/commands/build.rs (that function is `pub(crate)`, so
    // this test drives the `zfb_content` entrypoint it wraps directly with
    // the same construction).
    let snapshot_collections: Vec<CollectionConfig> = cfg
        .collections
        .iter()
        .map(|c| CollectionConfig {
            name: c.name.clone(),
            root: project_root.join(&c.path),
            include: c.include.clone(),
            exclude: c.exclude.clone(),
            id_strip_suffix: c.id_strip_suffix.clone(),
        })
        .collect();
    let snapshot = build_snapshot_with_config(&snapshot_collections, &PipelineSpec::default())
        .expect("content snapshot build must succeed for an out-of-root collection root");

    let posts = snapshot
        .collections
        .get("posts")
        .expect("posts collection must be present in the snapshot");
    assert_eq!(
        posts.len(),
        1,
        "include: [\"**/*.mdx\"] must keep only hello.mdx, got: {posts:?}"
    );
    assert_eq!(
        posts[0].slug, "hello",
        "slug must derive from the out-of-root rel_path"
    );
    assert_eq!(posts[0].rel_path, "hello.mdx");
    assert_eq!(
        posts[0].frontmatter.get("title").and_then(|v| v.as_str()),
        Some("Hello From Outside Root"),
        "frontmatter must be read from the out-of-root source file: {:?}",
        posts[0].frontmatter
    );
    assert!(
        !posts.iter().any(|e| e.slug == "notes"),
        "include: [\"**/*.mdx\"] must exclude notes.md even though .md is a recognised extension"
    );

    // ---------------------------------------------------------------
    // 3. Build materialisation — shadow/content/<collection>/... placement.
    // ---------------------------------------------------------------
    let outdir_tmp = tempfile::tempdir().expect("tempdir for build outdir");
    let mut session = ShadowSession::new().expect("create shadow session");
    let input = BundlerInput {
        content_collections: vec![ContentCollectionSpec {
            include: Some(vec!["**/*.mdx".to_string()]),
            ..ContentCollectionSpec::new("posts", PathBuf::from("../shared-content/posts"))
        }],
        // `mock_subprocess_output` skips the esbuild subprocess entirely —
        // every shadow-tree materialisation pass (including the
        // content-collection walk under test) still runs in full before
        // esbuild would have been invoked, so this is a genuine exercise of
        // `materialise_collection`, not a stub of it.
        mock_subprocess_output: Some("export default {};".to_string()),
        ..BundlerInput::for_project(
            project_root.clone(),
            Framework::Preact,
            BundleMode::Production,
            outdir_tmp.path().join("dist"),
            None,
        )
    };
    bundle_with_session(input, Some(&mut session))
        .expect("bundle must succeed materialising an out-of-root collection root");

    let shadow_posts = session.shadow_root().join("content").join("posts");
    assert!(
        shadow_posts.join("hello.mdx").exists(),
        "materialise_collection must place the out-of-root entry under shadow/content/posts/"
    );
    assert!(
        !shadow_posts.join("notes.md").exists(),
        "materialise_collection must honour the include filter and skip notes.md"
    );
}
