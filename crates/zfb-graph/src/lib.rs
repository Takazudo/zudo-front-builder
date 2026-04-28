//! zfb-graph: page dependency graph.
//!
//! Tracks which pages depend on which sources (content collections, layouts,
//! components, styles, data modules) and answers: "given this file changed,
//! which pages need to be re-rendered?"
//!
//! Consumes the resolved-dep output of Epic 3's module resolver
//! ([`zfb_render::loader::ModuleLoader`]). The exact resolver output type is
//! not yet shared as a public struct, so this crate is intentionally
//! structurally-typed: it accepts any iterable of `(page, deps)` tuples whose
//! page is identified by its source path and whose deps are absolute paths.
//!
//! ## Public surface
//!
//! - [`PageId`] — newtype around a page's source `PathBuf` (matches
//!   [`zfb_router::Route::source_path`] semantically without a hard dep).
//! - [`DepKind`] — informational tag for a dependency edge.
//! - [`PageDeps`] — input record: a page plus its resolved dependencies.
//! - [`DependencyGraph`] — the graph itself.
//! - [`DirtySet`] — return shape of [`DependencyGraph::dirty_pages`]; either a
//!   specific set of pages or the [`DirtySet::All`] sentinel meaning "rebuild
//!   every page" (used for global files like `zfb.config.ts`).
//! - [`AssetDeps`] — per-page asset-dependency record: the islands components
//!   the page hydrates and the CSS modules it pulls in. Tracked alongside the
//!   page-source graph so dev rebuilds can scope asset-bundle invalidation
//!   precisely (re-bundle islands only when a page that uses them changed,
//!   re-emit CSS modules only when a page that imports them changed).
//!
//! ## Coarse-rebuild policy
//!
//! Some files affect every page (the framework config, top-level `_app.tsx`,
//! etc.). The graph treats these as **global**. A change to a global path
//! returns [`DirtySet::All`], not the union of all per-page edges. Callers
//! should treat this as a "rebuild everything" sentinel rather than iterating
//! over a giant `Vec<PageId>`.
//!
//! `zfb.config.ts` is registered as global by default. Add more via
//! [`DependencyGraph::mark_global`].

pub mod error;
pub mod persist;

pub use error::GraphError;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Identifier for a page in the graph. Wraps the source `.tsx` path so it can
/// be compared cheaply and printed in diagnostics.
///
/// We use a newtype rather than a bare `PathBuf` so future migrations (e.g.
/// switching to a content-hash key) do not ripple through callers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PageId(PathBuf);

impl PageId {
    /// Build a page id from its source path. The path is stored as-is (no
    /// canonicalisation) — callers are expected to feed in absolute paths
    /// matching what the resolver emits.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    /// Borrow the underlying path.
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Consume self and return the inner `PathBuf`.
    pub fn into_path(self) -> PathBuf {
        self.0
    }
}

impl From<PathBuf> for PageId {
    fn from(p: PathBuf) -> Self {
        Self(p)
    }
}

impl From<&Path> for PageId {
    fn from(p: &Path) -> Self {
        Self(p.to_path_buf())
    }
}

/// Informational tag for a dependency edge. The graph itself does not branch
/// on kind — every edge invalidates its owning page — but downstream tooling
/// (build orchestrator, diagnostics) can use the kind to decide which sub-
/// pipeline to re-run (CSS only, islands only, etc.).
///
/// Kept deliberately small: the resolver exposes raw paths today, so
/// classification is the consumer's job. Add variants conservatively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DepKind {
    /// A `.tsx` / `.ts` / `.jsx` / `.js` module imported from the page graph.
    Module,
    /// A markdown / MDX entry consumed via a content collection.
    Content,
    /// A CSS / global stylesheet.
    Style,
    /// A static data module (JSON, TS-as-data, etc.).
    Data,
    /// A static asset under `public/` referenced by the page.
    Asset,
    /// Catch-all for edges whose flavour is not yet classified.
    Other,
}

/// One page plus its resolved dependencies. Input record consumed by
/// [`DependencyGraph::from_pages`] and [`DependencyGraph::upsert`].
#[derive(Debug, Clone)]
pub struct PageDeps {
    /// The page itself.
    pub page: PageId,
    /// Absolute paths the page depends on, paired with their kind.
    pub deps: Vec<(PathBuf, DepKind)>,
}

impl PageDeps {
    /// Convenience constructor.
    pub fn new(page: impl Into<PageId>, deps: Vec<(PathBuf, DepKind)>) -> Self {
        Self {
            page: page.into(),
            deps,
        }
    }
}

/// The result of [`DependencyGraph::dirty_pages`].
///
/// A change to a globally-scoped file (see [`DependencyGraph::mark_global`])
/// returns [`DirtySet::All`]. Every other change returns
/// [`DirtySet::Specific`] with the (possibly empty) set of affected pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirtySet {
    /// Rebuild every page. Triggered by changes to global files (e.g.
    /// `zfb.config.ts`) or by file deletions whose former consumers cannot
    /// be enumerated reliably.
    All,
    /// Rebuild this specific set of pages. May be empty when the changed file
    /// is unknown to the graph (no consumer recorded).
    Specific(BTreeSet<PageId>),
}

impl DirtySet {
    /// Convenience: collect the specific pages into a sorted `Vec` if this is
    /// a [`DirtySet::Specific`] — returns `None` for [`DirtySet::All`].
    pub fn as_pages(&self) -> Option<Vec<PageId>> {
        match self {
            DirtySet::All => None,
            DirtySet::Specific(set) => Some(set.iter().cloned().collect()),
        }
    }

    /// `true` iff this is the [`DirtySet::All`] sentinel.
    pub fn is_all(&self) -> bool {
        matches!(self, DirtySet::All)
    }
}

/// Per-page asset-dependency record.
///
/// Sits alongside the source-level [`DependencyGraph`] edges. The source graph
/// answers "which pages must re-render?" — the asset layer answers "which
/// asset bundles must re-emit?". A `"use client"` component edit dirties the
/// islands bundles owned by every page that hydrates it, but does not by
/// itself dirty the page's HTML. Likewise, a CSS-modules file edit dirties
/// the CSS asset for every consumer page.
///
/// Fields are deliberately small and structurally typed: islands are keyed by
/// stable component identifier (typically the scanner's
/// `component_name`), CSS modules by source path. Producers of this record
/// are not required to canonicalise paths, but they must hand in the same
/// representation they query against.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetDeps {
    /// Stable component identifiers (see `zfb_islands::scanner`'s notion of
    /// component-name identity) of every `"use client"` island the page
    /// hydrates.
    pub islands: BTreeSet<String>,
    /// CSS-Modules source paths the page pulls in. Plain stylesheets that
    /// land in the global Tailwind asset are not tracked here — those
    /// dirty every page through the regular `DepKind::Style` edge.
    pub css_modules: BTreeSet<PathBuf>,
}

impl AssetDeps {
    /// Convenience: construct from islands and css-modules iterables.
    pub fn new(
        islands: impl IntoIterator<Item = String>,
        css_modules: impl IntoIterator<Item = PathBuf>,
    ) -> Self {
        Self {
            islands: islands.into_iter().collect(),
            css_modules: css_modules.into_iter().collect(),
        }
    }

    /// `true` iff there are no recorded asset dependencies at all.
    pub fn is_empty(&self) -> bool {
        self.islands.is_empty() && self.css_modules.is_empty()
    }
}

/// Page dependency graph.
///
/// Internally maintains:
///
/// - `forward[page] -> {dep, kind}` — what each page depends on (kept so
///   `upsert` and `remove_node` can clean the reverse index).
/// - `reverse[dep] -> {pages}` — what pages depend on this file (the index
///   used by [`DependencyGraph::dirty_pages`]).
/// - `pages` — the canonical set of known pages.
/// - `globals` — paths whose change forces [`DirtySet::All`].
/// - `assets[page] -> AssetDeps` — per-page islands + css-modules sets
///   maintained alongside the source graph.
/// - `assets_islands_reverse[component] -> {pages}` and
///   `assets_css_reverse[module_path] -> {pages}` — reverse indexes that
///   power [`DependencyGraph::pages_using_island`] and
///   [`DependencyGraph::pages_using_css_module`] in O(1).
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DependencyGraph {
    forward: HashMap<PageId, Vec<(PathBuf, DepKind)>>,
    reverse: HashMap<PathBuf, HashSet<PageId>>,
    pages: HashSet<PageId>,
    globals: HashSet<PathBuf>,
    assets: HashMap<PageId, AssetDeps>,
    assets_islands_reverse: HashMap<String, HashSet<PageId>>,
    assets_css_reverse: HashMap<PathBuf, HashSet<PageId>>,
}

impl Default for DependencyGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl DependencyGraph {
    /// Build an empty graph. The default global set is empty; the caller
    /// should register their `zfb.config.ts` (and any other always-invalidates
    /// paths) via [`Self::mark_global`].
    pub fn new() -> Self {
        Self {
            forward: HashMap::new(),
            reverse: HashMap::new(),
            pages: HashSet::new(),
            globals: HashSet::new(),
            assets: HashMap::new(),
            assets_islands_reverse: HashMap::new(),
            assets_css_reverse: HashMap::new(),
        }
    }

    /// Build a graph from an initial batch of `(page, deps)` records — this
    /// is the typical entry point right after the module resolver finishes.
    pub fn from_pages(records: impl IntoIterator<Item = PageDeps>) -> Self {
        let mut g = Self::new();
        for r in records {
            g.upsert(r);
        }
        g
    }

    /// Number of pages currently tracked.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// All currently-known pages, sorted for deterministic iteration.
    pub fn pages(&self) -> Vec<PageId> {
        let mut v: Vec<PageId> = self.pages.iter().cloned().collect();
        v.sort();
        v
    }

    /// Mark `path` as a global file: any change to it returns
    /// [`DirtySet::All`] regardless of recorded edges.
    ///
    /// Typical examples: `zfb.config.ts`, `pages/_app.tsx`,
    /// `pages/_document.tsx`. The graph does not auto-register any of these —
    /// the caller decides the policy.
    pub fn mark_global(&mut self, path: impl Into<PathBuf>) {
        self.globals.insert(path.into());
    }

    /// Stop treating `path` as a global file. No-op if it was not marked.
    pub fn unmark_global(&mut self, path: &Path) {
        self.globals.remove(path);
    }

    /// Whether `path` is currently registered as a global file.
    pub fn is_global(&self, path: &Path) -> bool {
        self.globals.contains(path)
    }

    /// Insert or replace the dep set for a single page. Cleans up stale
    /// reverse-index entries so callers can hand in the latest resolver
    /// output without bookkeeping.
    pub fn upsert(&mut self, record: PageDeps) {
        let page = record.page;

        // Remove any prior reverse edges from this page so a re-resolve does
        // not leave dangling entries.
        if let Some(prior) = self.forward.remove(&page) {
            for (dep, _) in prior {
                if let Some(consumers) = self.reverse.get_mut(&dep) {
                    consumers.remove(&page);
                    if consumers.is_empty() {
                        self.reverse.remove(&dep);
                    }
                }
            }
        }

        // Install fresh forward + reverse edges. A page is always a consumer
        // of itself — changing the page source obviously dirties the page —
        // so we add a self-edge unconditionally.
        let mut deps = record.deps;
        let self_edge = (page.path().to_path_buf(), DepKind::Module);
        if !deps.iter().any(|(p, _)| p == &self_edge.0) {
            deps.push(self_edge);
        }

        for (dep, _) in &deps {
            self.reverse
                .entry(dep.clone())
                .or_default()
                .insert(page.clone());
        }

        self.pages.insert(page.clone());
        self.forward.insert(page, deps);
    }

    /// Add a brand-new node to the graph.
    ///
    /// - If `path` is a page (a `pages/**/*.tsx` source), prefer
    ///   [`Self::upsert`] with an empty (or freshly-resolved) dep list —
    ///   `add_node` is for non-page sources discovered later (e.g., a new
    ///   markdown file in a content collection).
    /// - If `path` is already known as a dep or page, this is a no-op (we
    ///   keep existing edges intact).
    ///
    /// Returns `true` if the call introduced a new entry, `false` if the path
    /// was already present.
    ///
    /// Note: a freshly-added file has no recorded consumers yet, so the next
    /// [`Self::dirty_pages`] call for that path will return an empty
    /// [`DirtySet::Specific`]. Callers that need "consumers might appear
    /// imminently" semantics should re-resolve and call [`Self::upsert`] on
    /// affected pages.
    pub fn add_node(&mut self, path: impl Into<PathBuf>) -> bool {
        let path = path.into();
        if self.reverse.contains_key(&path) {
            return false;
        }
        // We only need an entry if something later adds an edge — but we
        // insert an empty consumer set so callers can distinguish "known but
        // unused" from "totally unknown" via reverse-index introspection.
        self.reverse.entry(path).or_default();
        true
    }

    /// Remove a node from the graph and return its former consumers.
    ///
    /// - If `path` was a page, the page is dropped and its reverse-index
    ///   entries are cleaned up. The returned set contains just that page id.
    /// - If `path` was a non-page dep, every page that depended on it loses
    ///   that edge. The returned set is the former consumer list — these
    ///   pages should be rebuilt.
    /// - If `path` was unknown, returns an empty set.
    ///
    /// A removed file invalidates all of its former consumers (they imported
    /// something that no longer exists), so the returned set is exactly the
    /// pages that need rebuilding.
    pub fn remove_node(&mut self, path: &Path) -> BTreeSet<PageId> {
        let mut affected: BTreeSet<PageId> = BTreeSet::new();

        // Case 1: removing a page.
        let page_id = PageId::new(path.to_path_buf());
        if self.pages.remove(&page_id) {
            if let Some(deps) = self.forward.remove(&page_id) {
                for (dep, _) in deps {
                    if let Some(consumers) = self.reverse.get_mut(&dep) {
                        consumers.remove(&page_id);
                        if consumers.is_empty() {
                            self.reverse.remove(&dep);
                        }
                    }
                }
            }
            // Tear down the asset-layer record too — leaving stale reverse
            // entries would surface this dropped page as an island/css
            // consumer on subsequent queries.
            self.clear_assets_for_page(&page_id);
            affected.insert(page_id);
            return affected;
        }

        // Case 2: removing a non-page dep. Collect consumers, clear edges.
        if let Some(consumers) = self.reverse.remove(path) {
            for page in &consumers {
                if let Some(forward_deps) = self.forward.get_mut(page) {
                    forward_deps.retain(|(p, _)| p != path);
                }
            }
            affected.extend(consumers);
        }

        affected
    }

    // -------------------------------------------------------------------------
    // Asset-dependency layer
    // -------------------------------------------------------------------------

    /// Replace the asset-dependency record for `page`.
    ///
    /// Cleans up the reverse indexes from the previous record so callers can
    /// hand in fresh resolver/scanner output without bookkeeping. Pages are
    /// not implicitly added to [`Self::pages`]: the asset layer is observable
    /// even on pages whose source-level edges have not been registered (e.g.,
    /// a freshly-scanned page whose module dep set is still being resolved).
    /// Producers that want a registered page should call [`Self::upsert`] in
    /// addition.
    pub fn set_assets_for_page(&mut self, page: PageId, deps: AssetDeps) {
        // Drop prior reverse entries for this page so we never leave stale
        // pointers behind.
        if let Some(prior) = self.assets.remove(&page) {
            for c in prior.islands {
                if let Some(consumers) = self.assets_islands_reverse.get_mut(&c) {
                    consumers.remove(&page);
                    if consumers.is_empty() {
                        self.assets_islands_reverse.remove(&c);
                    }
                }
            }
            for m in prior.css_modules {
                if let Some(consumers) = self.assets_css_reverse.get_mut(&m) {
                    consumers.remove(&page);
                    if consumers.is_empty() {
                        self.assets_css_reverse.remove(&m);
                    }
                }
            }
        }

        // Install fresh reverse entries.
        for c in &deps.islands {
            self.assets_islands_reverse
                .entry(c.clone())
                .or_default()
                .insert(page.clone());
        }
        for m in &deps.css_modules {
            self.assets_css_reverse
                .entry(m.clone())
                .or_default()
                .insert(page.clone());
        }

        self.assets.insert(page, deps);
    }

    /// Forget the asset-layer record for `page` (no-op if not present).
    /// Used by [`Self::remove_node`] when a page itself is removed.
    pub fn clear_assets_for_page(&mut self, page: &PageId) {
        if let Some(prior) = self.assets.remove(page) {
            for c in prior.islands {
                if let Some(consumers) = self.assets_islands_reverse.get_mut(&c) {
                    consumers.remove(page);
                    if consumers.is_empty() {
                        self.assets_islands_reverse.remove(&c);
                    }
                }
            }
            for m in prior.css_modules {
                if let Some(consumers) = self.assets_css_reverse.get_mut(&m) {
                    consumers.remove(page);
                    if consumers.is_empty() {
                        self.assets_css_reverse.remove(&m);
                    }
                }
            }
        }
    }

    /// Borrow the asset-dependency record for `page`. Returns `None` if no
    /// record has been registered (versus `Some(empty)` for a page that has
    /// been registered with no islands or CSS modules).
    pub fn assets_for_page(&self, page: &PageId) -> Option<&AssetDeps> {
        self.assets.get(page)
    }

    /// Pages that hydrate the island whose stable component identifier is
    /// `component`. Sorted for deterministic iteration. Empty when no page
    /// has registered the component.
    pub fn pages_using_island(&self, component: &str) -> Vec<PageId> {
        let mut v: Vec<PageId> = self
            .assets_islands_reverse
            .get(component)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    /// Pages that pull in the given CSS-Modules source path. Sorted.
    pub fn pages_using_css_module(&self, module_path: &Path) -> Vec<PageId> {
        let mut v: Vec<PageId> = self
            .assets_css_reverse
            .get(module_path)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    /// All distinct islands currently registered across every page, sorted.
    /// Useful for the dev-mode islands bundler which wants the union as its
    /// scan input.
    pub fn all_islands(&self) -> Vec<String> {
        let mut v: Vec<String> = self.assets_islands_reverse.keys().cloned().collect();
        v.sort();
        v
    }

    /// All distinct CSS-Modules paths currently registered across every page.
    /// Sorted.
    pub fn all_css_modules(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.assets_css_reverse.keys().cloned().collect();
        v.sort();
        v
    }

    // -------------------------------------------------------------------------
    // Dirty-set queries
    // -------------------------------------------------------------------------

    /// The minimal set of pages whose output is affected by changing `path`.
    ///
    /// - For a global file (see [`Self::mark_global`]) → [`DirtySet::All`].
    /// - For a page or dep with recorded consumers → [`DirtySet::Specific`]
    ///   with that consumer set.
    /// - For an unknown path → empty [`DirtySet::Specific`].
    ///
    /// This method is read-only; it does not mutate the graph. For deletions
    /// use [`Self::remove_node`], which both returns the affected set and
    /// updates the index.
    pub fn dirty_pages(&self, path: &Path) -> DirtySet {
        if self.globals.contains(path) {
            return DirtySet::All;
        }

        let mut out: BTreeSet<PageId> = BTreeSet::new();

        // The file is itself a page → it is dirty.
        let page_id = PageId::new(path.to_path_buf());
        if self.pages.contains(&page_id) {
            out.insert(page_id);
        }

        // Anything that imported this path is dirty.
        if let Some(consumers) = self.reverse.get(path) {
            for c in consumers {
                out.insert(c.clone());
            }
        }

        DirtySet::Specific(out)
    }

    /// Lookup helper: every recorded dep of a page (kind included), sorted by
    /// path. Returns an empty `Vec` for unknown pages.
    pub fn deps_of(&self, page: &PageId) -> Vec<(PathBuf, DepKind)> {
        let mut v = self
            .forward
            .get(page)
            .cloned()
            .unwrap_or_default();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// Lookup helper: every page that depends on `path`, sorted. Returns
    /// `None` if the path is not known to the graph at all (versus
    /// `Some(empty)` for a known-but-unused dep registered via
    /// [`Self::add_node`]).
    pub fn consumers_of(&self, path: &Path) -> Option<Vec<PageId>> {
        self.reverse.get(path).map(|set| {
            let mut v: Vec<PageId> = set.iter().cloned().collect();
            v.sort();
            v
        })
    }

    /// Snapshot for diagnostics: every (dep -> sorted consumers) pair, keyed
    /// for deterministic ordering. Mostly useful in tests.
    pub fn snapshot(&self) -> BTreeMap<PathBuf, Vec<PageId>> {
        let mut out: BTreeMap<PathBuf, Vec<PageId>> = BTreeMap::new();
        for (dep, consumers) in &self.reverse {
            let mut v: Vec<PageId> = consumers.iter().cloned().collect();
            v.sort();
            out.insert(dep.clone(), v);
        }
        out
    }
}

// -----------------------------------------------------------------------------
// tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn pid(s: &str) -> PageId {
        PageId::new(p(s))
    }

    #[test]
    fn empty_graph_returns_empty_specific_set() {
        let g = DependencyGraph::new();
        let d = g.dirty_pages(&p("/x"));
        assert_eq!(d, DirtySet::Specific(BTreeSet::new()));
        assert!(!d.is_all());
    }

    #[test]
    fn page_self_dirties_on_change_to_its_own_source() {
        let mut g = DependencyGraph::new();
        g.upsert(PageDeps::new(pid("/pages/index.tsx"), vec![]));
        let d = g.dirty_pages(&p("/pages/index.tsx"));
        match d {
            DirtySet::Specific(set) => {
                assert_eq!(set.len(), 1);
                assert!(set.contains(&pid("/pages/index.tsx")));
            }
            DirtySet::All => panic!("expected specific set, got All"),
        }
    }

    #[test]
    fn shared_layout_dirties_all_consumers() {
        let mut g = DependencyGraph::new();
        g.upsert(PageDeps::new(
            pid("/pages/a.tsx"),
            vec![(p("/layouts/main.tsx"), DepKind::Module)],
        ));
        g.upsert(PageDeps::new(
            pid("/pages/b.tsx"),
            vec![(p("/layouts/main.tsx"), DepKind::Module)],
        ));
        g.upsert(PageDeps::new(
            pid("/pages/c.tsx"),
            vec![(p("/layouts/other.tsx"), DepKind::Module)],
        ));

        let d = g.dirty_pages(&p("/layouts/main.tsx"));
        let pages = d.as_pages().expect("specific");
        assert_eq!(pages, vec![pid("/pages/a.tsx"), pid("/pages/b.tsx")]);
    }

    #[test]
    fn unique_dep_dirties_only_one_page() {
        let mut g = DependencyGraph::new();
        g.upsert(PageDeps::new(
            pid("/pages/a.tsx"),
            vec![(p("/components/only-a.tsx"), DepKind::Module)],
        ));
        g.upsert(PageDeps::new(
            pid("/pages/b.tsx"),
            vec![(p("/components/only-b.tsx"), DepKind::Module)],
        ));

        let d = g.dirty_pages(&p("/components/only-a.tsx"));
        assert_eq!(d.as_pages().unwrap(), vec![pid("/pages/a.tsx")]);
    }

    #[test]
    fn unknown_path_returns_empty_specific() {
        let mut g = DependencyGraph::new();
        g.upsert(PageDeps::new(pid("/pages/a.tsx"), vec![]));
        let d = g.dirty_pages(&p("/totally/unrelated.css"));
        assert_eq!(d, DirtySet::Specific(BTreeSet::new()));
    }

    #[test]
    fn global_file_returns_all() {
        let mut g = DependencyGraph::new();
        g.upsert(PageDeps::new(pid("/pages/a.tsx"), vec![]));
        g.upsert(PageDeps::new(pid("/pages/b.tsx"), vec![]));
        g.mark_global("/zfb.config.ts");

        let d = g.dirty_pages(&p("/zfb.config.ts"));
        assert!(d.is_all());
        assert!(d.as_pages().is_none());
    }

    #[test]
    fn global_overrides_recorded_edges() {
        let mut g = DependencyGraph::new();
        g.upsert(PageDeps::new(
            pid("/pages/a.tsx"),
            vec![(p("/zfb.config.ts"), DepKind::Other)],
        ));
        g.mark_global("/zfb.config.ts");
        // Even though only page a recorded the edge, a global file change
        // means "rebuild everything" — All, not just {a}.
        let d = g.dirty_pages(&p("/zfb.config.ts"));
        assert!(d.is_all());
    }

    #[test]
    fn upsert_replaces_prior_deps_cleanly() {
        let mut g = DependencyGraph::new();
        g.upsert(PageDeps::new(
            pid("/pages/a.tsx"),
            vec![(p("/x.tsx"), DepKind::Module)],
        ));
        // Prior /x.tsx edge should disappear; new /y.tsx edge should appear.
        g.upsert(PageDeps::new(
            pid("/pages/a.tsx"),
            vec![(p("/y.tsx"), DepKind::Module)],
        ));

        let d_x = g.dirty_pages(&p("/x.tsx"));
        assert_eq!(d_x.as_pages().unwrap(), Vec::<PageId>::new());

        let d_y = g.dirty_pages(&p("/y.tsx"));
        assert_eq!(d_y.as_pages().unwrap(), vec![pid("/pages/a.tsx")]);
    }

    #[test]
    fn remove_node_for_a_page_drops_it() {
        let mut g = DependencyGraph::new();
        g.upsert(PageDeps::new(
            pid("/pages/a.tsx"),
            vec![(p("/lib/util.tsx"), DepKind::Module)],
        ));
        let removed = g.remove_node(&p("/pages/a.tsx"));
        assert!(removed.contains(&pid("/pages/a.tsx")));
        assert_eq!(g.page_count(), 0);
        // Reverse edge for /lib/util.tsx should also be cleaned up.
        let d = g.dirty_pages(&p("/lib/util.tsx"));
        assert_eq!(d.as_pages().unwrap(), Vec::<PageId>::new());
    }

    #[test]
    fn remove_node_for_a_dep_returns_consumers() {
        let mut g = DependencyGraph::new();
        g.upsert(PageDeps::new(
            pid("/pages/a.tsx"),
            vec![(p("/components/shared.tsx"), DepKind::Module)],
        ));
        g.upsert(PageDeps::new(
            pid("/pages/b.tsx"),
            vec![(p("/components/shared.tsx"), DepKind::Module)],
        ));
        let removed = g.remove_node(&p("/components/shared.tsx"));
        let removed_v: Vec<PageId> = removed.into_iter().collect();
        assert_eq!(removed_v, vec![pid("/pages/a.tsx"), pid("/pages/b.tsx")]);

        // Subsequent dirty_pages for the deleted file should be empty
        // (consumers no longer recorded under that path).
        let d = g.dirty_pages(&p("/components/shared.tsx"));
        assert_eq!(d.as_pages().unwrap(), Vec::<PageId>::new());
    }

    #[test]
    fn remove_node_for_unknown_path_returns_empty() {
        let mut g = DependencyGraph::new();
        let removed = g.remove_node(&p("/nope"));
        assert!(removed.is_empty());
    }

    #[test]
    fn add_node_is_idempotent_for_known_paths() {
        let mut g = DependencyGraph::new();
        g.upsert(PageDeps::new(
            pid("/pages/a.tsx"),
            vec![(p("/x.tsx"), DepKind::Module)],
        ));
        // /x.tsx is already in the reverse index.
        assert!(!g.add_node("/x.tsx"));
        // Brand-new path → true.
        assert!(g.add_node("/new-thing.tsx"));
        // Second time → false.
        assert!(!g.add_node("/new-thing.tsx"));
    }

    #[test]
    fn pages_returns_sorted_list() {
        let mut g = DependencyGraph::new();
        g.upsert(PageDeps::new(pid("/pages/c.tsx"), vec![]));
        g.upsert(PageDeps::new(pid("/pages/a.tsx"), vec![]));
        g.upsert(PageDeps::new(pid("/pages/b.tsx"), vec![]));
        assert_eq!(
            g.pages(),
            vec![pid("/pages/a.tsx"), pid("/pages/b.tsx"), pid("/pages/c.tsx")]
        );
    }

    #[test]
    fn deps_of_returns_sorted_with_self_edge() {
        let mut g = DependencyGraph::new();
        g.upsert(PageDeps::new(
            pid("/pages/a.tsx"),
            vec![
                (p("/c.tsx"), DepKind::Module),
                (p("/a.css"), DepKind::Style),
            ],
        ));
        let deps = g.deps_of(&pid("/pages/a.tsx"));
        let paths: Vec<PathBuf> = deps.iter().map(|(p, _)| p.clone()).collect();
        // a.css, /c.tsx, and the implicit self-edge (/pages/a.tsx).
        assert_eq!(paths.len(), 3);
        // Sorted.
        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted);
        assert!(paths.contains(&p("/pages/a.tsx")));
    }

    #[test]
    fn consumers_of_distinguishes_unknown_from_unused() {
        let mut g = DependencyGraph::new();
        g.upsert(PageDeps::new(
            pid("/pages/a.tsx"),
            vec![(p("/x.tsx"), DepKind::Module)],
        ));
        assert_eq!(
            g.consumers_of(&p("/x.tsx")),
            Some(vec![pid("/pages/a.tsx")])
        );
        assert_eq!(g.consumers_of(&p("/never-seen.tsx")), None);

        g.add_node("/known-but-unused.tsx");
        assert_eq!(g.consumers_of(&p("/known-but-unused.tsx")), Some(vec![]));
    }

    // -------------------------------------------------------------------------
    // asset-dependency layer
    // -------------------------------------------------------------------------

    #[test]
    fn asset_layer_records_islands_per_page() {
        let mut g = DependencyGraph::new();
        g.set_assets_for_page(
            pid("/pages/a.tsx"),
            AssetDeps::new(["Counter".to_string()], []),
        );
        g.set_assets_for_page(
            pid("/pages/b.tsx"),
            AssetDeps::new(["Counter".to_string(), "Search".to_string()], []),
        );
        g.set_assets_for_page(
            pid("/pages/c.tsx"),
            AssetDeps::new(["Search".to_string()], []),
        );

        let a_assets = g.assets_for_page(&pid("/pages/a.tsx")).unwrap();
        assert!(a_assets.islands.contains("Counter"));
        assert!(!a_assets.islands.contains("Search"));

        // Reverse lookup: editing the Counter component dirties exactly
        // the pages that hydrate it.
        assert_eq!(
            g.pages_using_island("Counter"),
            vec![pid("/pages/a.tsx"), pid("/pages/b.tsx")],
        );
        assert_eq!(
            g.pages_using_island("Search"),
            vec![pid("/pages/b.tsx"), pid("/pages/c.tsx")],
        );
        assert!(g.pages_using_island("DoesNotExist").is_empty());
    }

    #[test]
    fn asset_layer_records_css_modules_per_page() {
        let mut g = DependencyGraph::new();
        g.set_assets_for_page(
            pid("/pages/a.tsx"),
            AssetDeps::new([], [p("/styles/header.module.css")]),
        );
        g.set_assets_for_page(
            pid("/pages/b.tsx"),
            AssetDeps::new(
                [],
                [p("/styles/header.module.css"), p("/styles/footer.module.css")],
            ),
        );
        assert_eq!(
            g.pages_using_css_module(&p("/styles/header.module.css")),
            vec![pid("/pages/a.tsx"), pid("/pages/b.tsx")],
        );
        assert_eq!(
            g.pages_using_css_module(&p("/styles/footer.module.css")),
            vec![pid("/pages/b.tsx")],
        );
        assert!(g.pages_using_css_module(&p("/nope.css")).is_empty());
    }

    #[test]
    fn set_assets_replaces_prior_record_cleanly() {
        let mut g = DependencyGraph::new();
        g.set_assets_for_page(
            pid("/pages/a.tsx"),
            AssetDeps::new(["Old".to_string()], [p("/styles/old.module.css")]),
        );
        // Replace: Old should disappear from reverse indexes.
        g.set_assets_for_page(
            pid("/pages/a.tsx"),
            AssetDeps::new(["New".to_string()], [p("/styles/new.module.css")]),
        );
        assert!(g.pages_using_island("Old").is_empty());
        assert_eq!(
            g.pages_using_island("New"),
            vec![pid("/pages/a.tsx")],
        );
        assert!(g
            .pages_using_css_module(&p("/styles/old.module.css"))
            .is_empty());
        assert_eq!(
            g.pages_using_css_module(&p("/styles/new.module.css")),
            vec![pid("/pages/a.tsx")],
        );
    }

    #[test]
    fn remove_node_clears_asset_layer_for_page() {
        let mut g = DependencyGraph::new();
        g.upsert(PageDeps::new(pid("/pages/a.tsx"), vec![]));
        g.set_assets_for_page(
            pid("/pages/a.tsx"),
            AssetDeps::new(["Counter".to_string()], [p("/m.module.css")]),
        );
        // Removing the page also forgets its asset record so reverse
        // queries don't surface a dangling page.
        let removed = g.remove_node(&p("/pages/a.tsx"));
        assert!(removed.contains(&pid("/pages/a.tsx")));
        assert!(g.pages_using_island("Counter").is_empty());
        assert!(g.pages_using_css_module(&p("/m.module.css")).is_empty());
        assert!(g.assets_for_page(&pid("/pages/a.tsx")).is_none());
    }

    #[test]
    fn all_islands_and_all_css_modules_are_sorted_unions() {
        let mut g = DependencyGraph::new();
        g.set_assets_for_page(
            pid("/pages/a.tsx"),
            AssetDeps::new(["Z".to_string(), "A".to_string()], [p("/y.css"), p("/x.css")]),
        );
        g.set_assets_for_page(
            pid("/pages/b.tsx"),
            AssetDeps::new(["M".to_string(), "A".to_string()], [p("/x.css")]),
        );
        assert_eq!(
            g.all_islands(),
            vec!["A".to_string(), "M".to_string(), "Z".to_string()],
        );
        assert_eq!(g.all_css_modules(), vec![p("/x.css"), p("/y.css")]);
    }

    #[test]
    fn assets_for_page_distinguishes_unregistered_from_empty() {
        let mut g = DependencyGraph::new();
        assert!(g.assets_for_page(&pid("/pages/none.tsx")).is_none());
        g.set_assets_for_page(pid("/pages/empty.tsx"), AssetDeps::default());
        let rec = g.assets_for_page(&pid("/pages/empty.tsx")).unwrap();
        assert!(rec.is_empty());
    }

    #[test]
    fn from_pages_constructor_is_equivalent_to_repeated_upsert() {
        let recs = vec![
            PageDeps::new(pid("/pages/a.tsx"), vec![(p("/x"), DepKind::Module)]),
            PageDeps::new(pid("/pages/b.tsx"), vec![(p("/x"), DepKind::Module)]),
        ];
        let g1 = DependencyGraph::from_pages(recs.clone());

        let mut g2 = DependencyGraph::new();
        for r in recs {
            g2.upsert(r);
        }
        assert_eq!(g1.snapshot(), g2.snapshot());
    }
}
