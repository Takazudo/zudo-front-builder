//! [`RebuildPlan`] — what the orchestrator decided needs to happen for a
//! given burst of filesystem changes.
//!
//! The plan is the only thing the orchestrator hands to an
//! [`crate::AssetPipeline`]. Pipelines never see raw filesystem events;
//! they consume a plan and act on it. This keeps the granularity policy
//! in one place ([`crate::policy`]) and lets the pipeline focus on
//! "given this plan, run the right sub-pipelines and write output
//! atomically".

use std::collections::BTreeSet;
use std::path::PathBuf;

use zfb_graph::{DirtySet, PageId};

/// The tick's directly-edited content-collection files
/// (`PathClass::Content`), carried so the dev render callback can act on
/// the changed entries' own routes. Two consumers, two strictness levels:
///
/// - The #958 **eager** fan-out narrowing requires the STRICT gate
///   (`fan_out_safe`): narrow each dynamic source's fan-out to the routes
///   derived from these entries ONLY when the whole tick was in-place
///   `Modified` content edits — a co-changed module/component can affect
///   every page, so narrowing a mixed tick would under-render.
/// - The #1025 **lazy** eager-set derivation does NOT require it: it only
///   ADDS eager renders for the edited entries' own routes and leaves the
///   rest stale (never under-rendered), so it consumes `changed_content`
///   even when `fan_out_safe` is `false`.
///
/// `None` = nothing was directly content-edited this tick. Purely
/// advisory: must not affect `is_noop()`/merge logic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentNarrowing {
    /// Paths of the edited content files exactly as the watcher delivered
    /// them (absolute; same representation recorded in
    /// [`RebuildPlan::triggers`]). Populated permissively for the lazy
    /// eager basis (issue #1058): a file edited as `Modified` OR `Created`
    /// (an in-place edit of an existing file can arrive as `Created` under
    /// FSEvents coalescing). Genuinely-new files carry no routes yet, so
    /// the downstream route-table match renders nothing eager for them.
    pub changed_content: Vec<PathBuf>,
    /// `true` only when the ENTIRE tick was in-place `Modified` content
    /// edits — the strict #958 gate. The eager fan-out narrowing keys on
    /// this; the lazy eager-set derivation ignores it (issue #1058).
    pub fan_out_safe: bool,
}

/// What the orchestrator wants the asset pipeline to do for the next
/// rebuild tick.
///
/// Construction is via [`RebuildPlan::empty`] + the `mark_*` helpers, or
/// via [`RebuildPlan::full_rebuild`] which sets every flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebuildPlan {
    /// Pages whose HTML must be re-rendered.
    pub pages: PageSelection,

    /// Whether the CSS pipeline should run again.
    pub rerun_css: bool,

    /// Whether the islands bundler should run again.
    pub rerun_islands: bool,

    /// Whether the client-scripts bundler should run again.
    ///
    /// Set whenever a `*.client.{ts,tsx,js,jsx}` file changes under any of
    /// the conventional discovery roots (`pages/`, `components/`, `src/`).
    /// The dev pipeline calls `ctx.run_client_scripts` when this is true
    /// and emits a `ReloadEvent::Page` on a successful re-bundle.
    pub rerun_client_scripts: bool,

    /// True when the renderer's bundle was already refreshed for this
    /// tick — by the boot bundle (initial render) or by the watch-ADD
    /// discovery hook's re-bundle + host reload. The pipeline consults
    /// this to skip its own `reload_renderer` call so a single tick
    /// never bundles twice.
    pub renderer_fresh: bool,

    /// True when the tick was caused by a Content, Page, Module, or Data
    /// change — i.e. something that could affect SSR output. Used to
    /// trigger `reload_renderer` on SSR-only projects where `pages` is
    /// always empty (no SSG pages exist) but the V8 host still needs a
    /// fresh bundle after each edit (issue #807).
    pub ssr_reload_needed: bool,

    /// Absolute dist-root paths that the pipeline must delete from disk
    /// and evict from any in-memory caches (issue #804).
    ///
    /// Populated by the orchestrator from [`crate::DiscoveryOutcome::vanished_output_paths`]
    /// when a discovery refresh already handled the bundle reload (so
    /// `reload_renderer` is skipped) but the route table still lost some
    /// output paths. The pipeline processes these in addition to any
    /// vanished paths returned by `reload_renderer` itself.
    pub prune_paths: Vec<PathBuf>,

    /// The raw paths that triggered this plan, kept around purely for
    /// diagnostics. Not consumed by the pipeline.
    pub triggers: Vec<PathBuf>,

    /// See [`ContentNarrowing`]. [`RebuildPlan::empty`] and
    /// [`RebuildPlan::full_rebuild`] set `None`.
    pub content_narrowing: Option<ContentNarrowing>,
}

/// Which pages to re-render this tick.
///
/// Mirrors [`zfb_graph::DirtySet`] but is owned by `zfb-build`; we don't
/// re-export the graph variant directly so future plans can carry richer
/// per-page info (e.g. force flag, props override) without rippling
/// through the graph crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageSelection {
    /// Re-render every page (e.g. after a global change).
    All,
    /// Re-render exactly this set. Empty means "no pages need re-render
    /// this tick" — common when only CSS or only assets changed.
    Specific(BTreeSet<PageId>),
}

impl PageSelection {
    /// Empty (no pages). Convenience constructor.
    pub fn none() -> Self {
        PageSelection::Specific(BTreeSet::new())
    }

    /// True iff this is the [`PageSelection::All`] sentinel.
    pub fn is_all(&self) -> bool {
        matches!(self, PageSelection::All)
    }

    /// True iff no pages are selected. Returns false for `All` since
    /// `All` covers every page.
    pub fn is_empty(&self) -> bool {
        matches!(self, PageSelection::Specific(s) if s.is_empty())
    }

    /// Merge `other` into self. `All` absorbs everything.
    pub fn merge(&mut self, other: PageSelection) {
        match (&mut *self, other) {
            (PageSelection::All, _) => {}
            (slot, PageSelection::All) => *slot = PageSelection::All,
            (PageSelection::Specific(a), PageSelection::Specific(b)) => a.extend(b),
        }
    }
}

impl From<DirtySet> for PageSelection {
    fn from(d: DirtySet) -> Self {
        match d {
            DirtySet::All => PageSelection::All,
            DirtySet::Specific(s) => PageSelection::Specific(s),
        }
    }
}

impl RebuildPlan {
    /// An empty plan — nothing to do.
    pub fn empty() -> Self {
        Self {
            pages: PageSelection::none(),
            rerun_css: false,
            rerun_islands: false,
            rerun_client_scripts: false,
            renderer_fresh: false,
            ssr_reload_needed: false,
            prune_paths: Vec::new(),
            triggers: Vec::new(),
            content_narrowing: None,
        }
    }

    /// A plan that does everything: every page, every sub-pipeline.
    pub fn full_rebuild() -> Self {
        Self {
            pages: PageSelection::All,
            rerun_css: true,
            rerun_islands: true,
            rerun_client_scripts: true,
            renderer_fresh: false,
            ssr_reload_needed: true,
            prune_paths: Vec::new(),
            triggers: Vec::new(),
            content_narrowing: None,
        }
    }

    /// Add absolute dist paths to prune during this tick's pipeline run.
    /// These paths come from route-table diffs in the discovery hook.
    pub fn add_prune_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.prune_paths.extend(paths);
    }

    /// Mark the renderer bundle as already refreshed for this tick (see
    /// [`RebuildPlan::renderer_fresh`]).
    pub fn mark_renderer_fresh(&mut self) {
        self.renderer_fresh = true;
    }

    /// Mark this tick as SSR-relevant — caused by a Content, Page, Module,
    /// or Data change that may affect SSR output (issue #807). The pipeline
    /// uses this to trigger `reload_renderer` on SSR-only projects where
    /// `pages` is always empty.
    pub fn mark_ssr_reload_needed(&mut self) {
        self.ssr_reload_needed = true;
    }

    /// Add `path` to the diagnostics trigger list.
    pub fn record_trigger(&mut self, path: PathBuf) {
        self.triggers.push(path);
    }

    /// Mark these pages dirty. Combines with what's already in `pages`.
    pub fn mark_pages(&mut self, pages: PageSelection) {
        self.pages.merge(pages);
    }

    /// Mark CSS as needing a rerun.
    pub fn mark_css(&mut self) {
        self.rerun_css = true;
    }

    /// Mark islands as needing a rerun.
    pub fn mark_islands(&mut self) {
        self.rerun_islands = true;
    }

    /// Mark client scripts as needing a rerun.
    pub fn mark_client_scripts(&mut self) {
        self.rerun_client_scripts = true;
    }

    /// True iff the plan would do nothing — no pages, no CSS, no
    /// islands, no client scripts, no paths to prune, and no SSR-only
    /// host reload needed. The orchestrator skips empty plans.
    pub fn is_noop(&self) -> bool {
        !self.rerun_css
            && !self.rerun_islands
            && !self.rerun_client_scripts
            && !self.ssr_reload_needed
            && self.pages.is_empty()
            && self.prune_paths.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn pid(s: &str) -> PageId {
        PageId::new(PathBuf::from(s))
    }

    #[test]
    fn empty_plan_is_noop() {
        assert!(RebuildPlan::empty().is_noop());
    }

    /// The content-narrowing hint is purely advisory (issue #958): a plan
    /// that carries one but selects nothing must stay a no-op.
    #[test]
    fn content_narrowing_hint_does_not_affect_is_noop() {
        let mut plan = RebuildPlan::empty();
        plan.content_narrowing = Some(ContentNarrowing {
            changed_content: vec![PathBuf::from("/proj/content/post.md")],
            fan_out_safe: true,
        });
        assert!(plan.is_noop());
    }

    #[test]
    fn full_rebuild_is_not_noop() {
        let p = RebuildPlan::full_rebuild();
        assert!(!p.is_noop());
        assert!(p.pages.is_all());
        assert!(p.rerun_css);
        assert!(p.rerun_islands);
    }

    #[test]
    fn merging_specific_into_all_stays_all() {
        let mut sel = PageSelection::All;
        let mut s = BTreeSet::new();
        s.insert(pid("/p/a.tsx"));
        sel.merge(PageSelection::Specific(s));
        assert!(sel.is_all());
    }

    #[test]
    fn merging_all_into_specific_promotes() {
        let mut sel = PageSelection::Specific(BTreeSet::new());
        sel.merge(PageSelection::All);
        assert!(sel.is_all());
    }

    #[test]
    fn merging_specifics_unions() {
        let mut a = BTreeSet::new();
        a.insert(pid("/p/a.tsx"));
        let mut b = BTreeSet::new();
        b.insert(pid("/p/b.tsx"));
        let mut sel = PageSelection::Specific(a);
        sel.merge(PageSelection::Specific(b));
        match sel {
            PageSelection::Specific(s) => {
                assert!(s.contains(&pid("/p/a.tsx")));
                assert!(s.contains(&pid("/p/b.tsx")));
            }
            other => unreachable!("expected PageSelection::Specific, got {other:?}"),
        }
    }
}
