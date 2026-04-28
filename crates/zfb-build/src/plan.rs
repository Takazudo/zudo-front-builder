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

    /// The raw paths that triggered this plan, kept around purely for
    /// diagnostics. Not consumed by the pipeline.
    pub triggers: Vec<PathBuf>,
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
            triggers: Vec::new(),
        }
    }

    /// A plan that does everything: every page, every sub-pipeline.
    pub fn full_rebuild() -> Self {
        Self {
            pages: PageSelection::All,
            rerun_css: true,
            rerun_islands: true,
            triggers: Vec::new(),
        }
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

    /// True iff the plan would do nothing — no pages, no CSS, no
    /// islands. The orchestrator skips empty plans.
    pub fn is_noop(&self) -> bool {
        !self.rerun_css && !self.rerun_islands && self.pages.is_empty()
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
