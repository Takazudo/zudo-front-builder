//! Regression tests for #1293: persisted-graph load must not clobber
//! `DepKind::Module` edges that were populated before the cache load.
//!
//! # Background
//!
//! The dev boot sequence has two paths:
//!
//! - **Eager path**: `seed_boot_module_edges` populates Module edges into the
//!   live graph synchronously at boot, BEFORE the deferred boot task loads the
//!   persisted graph.
//! - **Deferred path**: `refresh_bundle_and_routes` (inside the deferred boot
//!   task, step 0) populates Module edges, THEN the cache load (step 2+3) runs.
//!
//! In both cases the cache load used to do `*g = persisted`, wholesale-replacing
//! the graph and dropping any Module edges that had just been seeded.  The fix
//! (#1293) switches to `merge_from_persisted`, which preserves live Module edges
//! while blending in persisted non-Module edges.
//!
//! The `assemble_boot_graph` function in `dev.rs` wraps both the merge and the
//! empty-seed step for unknown pages; these tests exercise `merge_from_persisted`
//! directly because the types it operates on live in `zfb-graph`.

use std::path::PathBuf;

use zfb_graph::{AssetDeps, DepKind, DependencyGraph, DirtySet, PageDeps, PageId};

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

fn pid(s: &str) -> PageId {
    PageId::new(p(s))
}

/// Regression: on a digest cache hit the Module edges seeded before the load
/// must survive. The old `*g = persisted` wiped them; `merge_from_persisted`
/// must not.
#[test]
fn module_edges_survive_persisted_graph_load() {
    // Simulate the live graph after seed_boot_module_edges / refresh_bundle_and_routes.
    let mut live = DependencyGraph::new();
    live.upsert(PageDeps::new(
        pid("/proj/pages/index.tsx"),
        vec![(p("/proj/components/Header.tsx"), DepKind::Module)],
    ));
    live.upsert(PageDeps::new(
        pid("/proj/pages/about.tsx"),
        vec![(p("/proj/components/Nav.tsx"), DepKind::Module)],
    ));

    // Simulate a persisted graph from a previous session: same pages but with
    // Content and Style edges that the current metafile doesn't know yet.
    let mut persisted = DependencyGraph::new();
    persisted.upsert(PageDeps::new(
        pid("/proj/pages/index.tsx"),
        vec![
            (p("/proj/content/post.md"), DepKind::Content),
            (p("/proj/styles/global.css"), DepKind::Style),
        ],
    ));
    persisted.upsert(PageDeps::new(
        pid("/proj/pages/about.tsx"),
        vec![(p("/proj/content/team.md"), DepKind::Content)],
    ));

    // This is the fix: merge rather than replace.
    live.merge_from_persisted(persisted);

    // Module edges from the live graph must still be present.
    assert_eq!(
        live.dirty_pages(&p("/proj/components/Header.tsx"))
            .as_pages()
            .unwrap(),
        vec![pid("/proj/pages/index.tsx")],
        "Module edge from live graph must survive merge_from_persisted"
    );
    assert_eq!(
        live.dirty_pages(&p("/proj/components/Nav.tsx"))
            .as_pages()
            .unwrap(),
        vec![pid("/proj/pages/about.tsx")],
        "Module edge for second page must survive"
    );

    // Non-Module edges from persisted must be present too.
    assert_eq!(
        live.dirty_pages(&p("/proj/content/post.md"))
            .as_pages()
            .unwrap(),
        vec![pid("/proj/pages/index.tsx")],
        "Content edge from persisted must be present after merge"
    );
    assert_eq!(
        live.dirty_pages(&p("/proj/styles/global.css"))
            .as_pages()
            .unwrap(),
        vec![pid("/proj/pages/index.tsx")],
        "Style edge from persisted must be present after merge"
    );
    assert_eq!(
        live.dirty_pages(&p("/proj/content/team.md"))
            .as_pages()
            .unwrap(),
        vec![pid("/proj/pages/about.tsx")],
        "Content edge for second page must be present after merge"
    );
}

/// Regression: the old `*g = persisted` path. Demonstrates what broke before
/// the fix: Module edges are lost when the whole graph is replaced.
/// This test ASSERTS THE BUG to lock the regression boundary so a revert is
/// immediately visible.
#[test]
fn replace_not_merge_loses_module_edges_regression_anchor() {
    let mut live = DependencyGraph::new();
    live.upsert(PageDeps::new(
        pid("/proj/pages/index.tsx"),
        vec![(p("/proj/components/Header.tsx"), DepKind::Module)],
    ));

    let mut persisted = DependencyGraph::new();
    persisted.upsert(PageDeps::new(
        pid("/proj/pages/index.tsx"),
        vec![(p("/proj/content/post.md"), DepKind::Content)],
    ));

    // Simulate the OLD bug: wholesale replace drops Module edges.
    live = persisted;

    let dirty = live.dirty_pages(&p("/proj/components/Header.tsx"));
    assert_eq!(
        dirty,
        DirtySet::Specific(Default::default()),
        "BUG ANCHOR: wholesale replace loses Module edges — if this assertion \
         fails the regression has been introduced again"
    );
}

/// Globals from the persisted graph are merged into live.
#[test]
fn globals_are_merged_from_persisted() {
    let mut live = DependencyGraph::new();
    live.mark_global("/proj/zfb.config.ts");

    let mut persisted = DependencyGraph::new();
    persisted.mark_global("/proj/extra.config.ts");

    live.merge_from_persisted(persisted);

    // Live global unchanged.
    assert!(
        live.is_global(&p("/proj/zfb.config.ts")),
        "pre-existing global must survive merge"
    );
    // Persisted global added.
    assert!(
        live.is_global(&p("/proj/extra.config.ts")),
        "persisted global must be merged in"
    );

    assert!(
        live.dirty_pages(&p("/proj/zfb.config.ts")).is_all(),
        "pre-existing global still triggers DirtySet::All"
    );
    assert!(
        live.dirty_pages(&p("/proj/extra.config.ts")).is_all(),
        "merged-in global triggers DirtySet::All"
    );
}

/// Asset deps from the persisted graph are copied for pages not already
/// tracked in the live asset layer.
#[test]
fn asset_deps_are_merged_for_untracked_pages() {
    let mut live = DependencyGraph::new();
    live.upsert(PageDeps::new(
        pid("/proj/pages/index.tsx"),
        vec![(p("/proj/components/Counter.tsx"), DepKind::Module)],
    ));

    let mut persisted = DependencyGraph::new();
    persisted.upsert(PageDeps::new(pid("/proj/pages/index.tsx"), vec![]));
    persisted.set_assets_for_page(
        pid("/proj/pages/index.tsx"),
        AssetDeps::new(["Counter".to_string()], [p("/styles/counter.module.css")]),
    );

    live.merge_from_persisted(persisted);

    // Live graph has no prior asset record for index.tsx; persisted record is
    // adopted.
    let assets = live
        .assets_for_page(&pid("/proj/pages/index.tsx"))
        .expect("asset record from persisted should be present");
    assert!(
        assets.islands.contains("Counter"),
        "island from persisted asset record must be present"
    );
    assert!(
        assets
            .css_modules
            .contains(&p("/styles/counter.module.css")),
        "CSS module from persisted asset record must be present"
    );
}

/// When the live graph already has an asset record for a page, the persisted
/// record does NOT overwrite it (live record is more current).
#[test]
fn live_asset_record_wins_over_persisted() {
    let mut live = DependencyGraph::new();
    live.upsert(PageDeps::new(pid("/proj/pages/index.tsx"), vec![]));
    live.set_assets_for_page(
        pid("/proj/pages/index.tsx"),
        AssetDeps::new(["LiveIsland".to_string()], []),
    );

    let mut persisted = DependencyGraph::new();
    persisted.upsert(PageDeps::new(pid("/proj/pages/index.tsx"), vec![]));
    persisted.set_assets_for_page(
        pid("/proj/pages/index.tsx"),
        AssetDeps::new(["StaleIsland".to_string()], []),
    );

    live.merge_from_persisted(persisted);

    let assets = live
        .assets_for_page(&pid("/proj/pages/index.tsx"))
        .expect("asset record should exist");
    assert!(
        assets.islands.contains("LiveIsland"),
        "live island must be kept"
    );
    assert!(
        !assets.islands.contains("StaleIsland"),
        "stale persisted island must not overwrite the live record"
    );
}

/// A page that exists ONLY in `persisted` (not yet seeded in live) has all its
/// edges (including non-Module) imported into live. Live has no Module edges
/// for it to preserve, so the persisted edges — including any Module ones —
/// are all taken.
#[test]
fn page_only_in_persisted_gets_all_its_edges() {
    let mut live = DependencyGraph::new();
    // index is already live; about is only in persisted.
    live.upsert(PageDeps::new(
        pid("/proj/pages/index.tsx"),
        vec![(p("/proj/components/Header.tsx"), DepKind::Module)],
    ));

    let mut persisted = DependencyGraph::new();
    persisted.upsert(PageDeps::new(
        pid("/proj/pages/about.tsx"),
        vec![
            (p("/proj/components/Nav.tsx"), DepKind::Module),
            (p("/proj/content/team.md"), DepKind::Content),
        ],
    ));

    live.merge_from_persisted(persisted);

    // The about page's Module and Content edges must appear in live.
    assert_eq!(
        live.dirty_pages(&p("/proj/components/Nav.tsx"))
            .as_pages()
            .unwrap(),
        vec![pid("/proj/pages/about.tsx")],
        "Module edge for persisted-only page must be imported"
    );
    assert_eq!(
        live.dirty_pages(&p("/proj/content/team.md"))
            .as_pages()
            .unwrap(),
        vec![pid("/proj/pages/about.tsx")],
        "Content edge for persisted-only page must be imported"
    );

    // The live index page must be unchanged.
    assert_eq!(
        live.dirty_pages(&p("/proj/components/Header.tsx"))
            .as_pages()
            .unwrap(),
        vec![pid("/proj/pages/index.tsx")],
        "live page's Module edge must be untouched"
    );
}
