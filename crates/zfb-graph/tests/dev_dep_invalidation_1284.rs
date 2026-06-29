//! Reproduction tests for bug #1284 (epic #1285), Wave-1 diagnosis #1286.
//!
//! These pin the *graph-level* root cause of symptoms A and B at Level 1
//! (pure logic, no V8, no browser):
//!
//! - **Symptom A** — editing an imported component (`components/**` or
//!   `src/components/**`), direct OR transitive, does not re-render the
//!   consuming route. Root cause proven here: in the DEV path the graph is
//!   seeded with page nodes that carry **no `DepKind::Module` edges**
//!   (`dev.rs` seeds `PageDeps::new(page_id, vec![])`; only `DepKind::Content`
//!   edges are upserted later). So `dirty_pages(component)` returns the EMPTY
//!   set — the graph cannot map a component back to its consumer route. The
//!   orchestrator's `Page|Module` arm then falls back to `PageSelection::All`
//!   (imprecise, and on `src/**` the watcher never even fires — that root is
//!   not watched). The #1287 fix makes `dirty_pages(component)` NON-EMPTY by
//!   populating per-route module-dep edges (esbuild `--metafile` `inputs`).
//!
//! - **Symptom B** — editing a transitively-imported CSS file (incl. a
//!   symlinked workspace dep reached via `@import`) does not map to any page.
//!   Same graph gap: no `DepKind::Style` edge for a transitively-imported CSS
//!   path, so `dirty_pages(css)` is empty.
//!
//! The CURRENT (buggy) behaviour is that these queries return empty. Each test
//! asserts the FIXED behaviour (non-empty) and is therefore `#[ignore]`d with
//! a pointer to #1284 so `cargo test` stays green until the fix waves land and
//! un-ignore them. See `research/1284-dev-dep-invalidation.md`.

use std::path::PathBuf;

use zfb_graph::{DependencyGraph, DirtySet, PageDeps, PageId};

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

fn pid(s: &str) -> PageId {
    PageId::new(p(s))
}

/// Mirror the DEV graph seed exactly: page nodes with NO dep edges
/// (`dev.rs` boot seed `PageDeps::new(page_id, vec![])`).
fn dev_seeded_graph() -> DependencyGraph {
    let mut g = DependencyGraph::new();
    g.upsert(PageDeps::new(pid("/proj/pages/index.tsx"), vec![]));
    g.upsert(PageDeps::new(pid("/proj/pages/about.tsx"), vec![]));
    g
}

/// SYMPTOM A — current bug: a component edit maps to NO consuming page,
/// because the dev graph has no Module edges. This test documents the
/// present (broken) state and MUST pass today so the regression boundary is
/// explicit. It is intentionally NOT ignored.
#[test]
fn current_bug_component_edit_maps_to_no_page() {
    let g = dev_seeded_graph();
    let dirty = g.dirty_pages(&p("/proj/components/Header.tsx"));
    assert_eq!(
        dirty,
        DirtySet::Specific(Default::default()),
        "dev graph has no Module edges, so a component maps to the empty set today"
    );
}

/// SYMPTOM A (fixed behaviour) — after #1287 populates per-route module-dep
/// edges, editing a directly-imported component dirties exactly its consumer
/// route. Ignored until the fix lands.
#[test]
#[ignore = "pending fix: #1284"]
fn component_edit_dirties_consuming_route() {
    // The fix populates Module edges; model the post-fix graph.
    let mut g = DependencyGraph::new();
    g.upsert(PageDeps::new(
        pid("/proj/pages/index.tsx"),
        vec![(p("/proj/components/Header.tsx"), zfb_graph::DepKind::Module)],
    ));
    g.upsert(PageDeps::new(pid("/proj/pages/about.tsx"), vec![]));

    let dirty = g.dirty_pages(&p("/proj/components/Header.tsx"));
    assert_eq!(
        dirty.as_pages().unwrap(),
        vec![pid("/proj/pages/index.tsx")],
        "after #1287 a component edit dirties exactly its consumer route, not All"
    );
}

/// SYMPTOM A (transitive) — a component imported only via another component
/// must still dirty the route. The metafile `inputs` map is transitive, so
/// the fix records the leaf→route edge directly. Ignored until the fix lands.
#[test]
#[ignore = "pending fix: #1284"]
fn transitively_imported_component_dirties_route() {
    // pages/index -> components/Header -> components/Logo
    // The metafile flattens transitive inputs, so Logo is recorded as a
    // Module dep of the route that ultimately pulls it in.
    let mut g = DependencyGraph::new();
    g.upsert(PageDeps::new(
        pid("/proj/pages/index.tsx"),
        vec![
            (p("/proj/components/Header.tsx"), zfb_graph::DepKind::Module),
            (p("/proj/components/Logo.tsx"), zfb_graph::DepKind::Module),
        ],
    ));

    let dirty = g.dirty_pages(&p("/proj/components/Logo.tsx"));
    assert_eq!(
        dirty.as_pages().unwrap(),
        vec![pid("/proj/pages/index.tsx")],
        "a transitively-imported component must dirty its ultimate consumer route"
    );
}

/// SYMPTOM B — current bug: a transitively-imported CSS file maps to NO page
/// because the dev graph has no Style edges. Present (broken) state; not
/// ignored.
#[test]
fn current_bug_transitive_css_maps_to_no_page() {
    let g = dev_seeded_graph();
    let dirty = g.dirty_pages(&p("/proj/styles/theme.css"));
    assert_eq!(
        dirty,
        DirtySet::Specific(Default::default()),
        "dev graph has no Style edges for transitively-imported CSS today"
    );
}

/// SYMPTOM B (fixed behaviour) — after #1288 resolves the `@import` graph and
/// registers Style edges (including canonicalised symlinked workspace deps),
/// the CSS file maps to the pages whose served stylesheet it feeds. Here we
/// model a global stylesheet that affects every page. Ignored until fix lands.
#[test]
#[ignore = "pending fix: #1284"]
fn transitively_imported_css_is_tracked() {
    // The css entry (styles/styles.css) @imports a workspace design-system.
    // After the fix, the resolved real path is registered; editing it must be
    // visible to the rebuild planner (here: known to the graph as a global so
    // every page's served /assets/styles.css refreshes).
    let mut g = DependencyGraph::new();
    g.upsert(PageDeps::new(pid("/proj/pages/index.tsx"), vec![]));
    g.mark_global("/real/design-system/dist/tokens.css");

    let dirty = g.dirty_pages(&p("/real/design-system/dist/tokens.css"));
    assert_eq!(
        dirty,
        DirtySet::All,
        "a resolved @import dep (canonicalised symlink target) must be tracked, not ignored"
    );
}
