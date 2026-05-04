//! [`BuildOrchestrator`] — the long-running glue that drives the dev loop.
//!
//! The orchestrator owns:
//!
//! - the [`zfb_watcher::Watcher`] handle (started on `run`),
//! - the [`zfb_graph::DependencyGraph`] (shared via `Arc<Mutex<…>>` so
//!   the resolver can update it from outside the orchestrator if needed),
//! - the [`crate::AssetPipeline`] implementation, and
//! - the [`crate::policy::GranularityPolicy`].
//!
//! ## Lifecycle
//!
//! 1. The bin crate (Epic 7) constructs an orchestrator with a
//!    [`OrchestratorConfig`], the graph, the asset pipeline, and a
//!    [`crate::pipeline::BuildContext`].
//! 2. It calls [`BuildOrchestrator::run`] on a tokio runtime.
//! 3. The orchestrator spawns a [`zfb_watcher::Watcher`] over the
//!    project's source roots, then loops on its `Change` receiver.
//! 4. For each batch of changes (drained from the channel within a tick
//!    window), the orchestrator folds the changes through
//!    [`crate::policy::classify_change`] + the dep graph into a single
//!    [`crate::RebuildPlan`], hands it to the pipeline, and reports the
//!    [`crate::pipeline::BuildOutcome`] back to the caller.
//!
//! ## Single-tick API
//!
//! [`BuildOrchestrator::plan_for_changes`] is exposed publicly because
//! integration tests (and the eventual one-shot `zfb build` command)
//! want to call "given this list of changed paths, what would the plan
//! look like?" without spinning up a watcher. The dev `run` loop uses
//! the same function internally.
//!
//! ## Why not a fixed tick window?
//!
//! The watcher already debounces (default 50ms). On top of that the
//! orchestrator drains *every* `Change` already in the channel before
//! invoking the pipeline — so a fast burst of saves still produces one
//! pipeline run per natural pause. There's no extra `sleep`-based
//! coalescing inside the orchestrator; if the watcher decides "this is
//! one logical save", we treat it as one.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use tracing::{debug, info, warn};
use zfb_graph::DependencyGraph;
use zfb_watcher::{Change, Watcher};

use crate::pipeline::{AssetPipeline, BuildContext, BuildOutcome};
use crate::plan::{PageSelection, RebuildPlan};
use crate::policy::{classify_change, GranularityPolicy, PathClass};

/// Construction-time configuration for [`BuildOrchestrator`].
#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    /// Project root (handed to [`zfb_watcher::Watcher::start`]).
    pub project_root: PathBuf,

    /// Relative source roots to watch. Typical: `["content", "pages",
    /// "components", "layouts", "styles", "data", "public",
    /// "zfb.config.ts"]`.
    pub watch_roots: Vec<PathBuf>,

    /// Granularity policy. Defaults to [`GranularityPolicy::default`].
    pub policy: GranularityPolicy,

    /// Optional override for the watcher debounce window. `None` =
    /// `zfb_watcher::DEFAULT_DEBOUNCE` (50ms).
    pub debounce: Option<Duration>,
}

impl OrchestratorConfig {
    /// Convenience: build a config from `(project_root, watch_roots)`
    /// with the default policy and debounce.
    pub fn new(project_root: impl Into<PathBuf>, watch_roots: Vec<PathBuf>) -> Self {
        Self {
            project_root: project_root.into(),
            watch_roots,
            policy: GranularityPolicy::default(),
            debounce: None,
        }
    }

    /// Override the policy (chainable).
    pub fn with_policy(mut self, policy: GranularityPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Override the debounce window (chainable).
    pub fn with_debounce(mut self, debounce: Duration) -> Self {
        self.debounce = Some(debounce);
        self
    }
}

/// The dev-loop orchestrator.
///
/// `P` is the [`AssetPipeline`] implementation (typically
/// [`crate::DevAssetPipeline`]).
pub struct BuildOrchestrator<P: AssetPipeline> {
    config: OrchestratorConfig,
    graph: Arc<Mutex<DependencyGraph>>,
    pipeline: P,
}

impl<P: AssetPipeline> BuildOrchestrator<P> {
    /// Construct a new orchestrator. The graph is shared via
    /// `Arc<Mutex<…>>` so the resolver/loader can mutate it (via
    /// `DependencyGraph::upsert`) while the orchestrator reads from it.
    pub fn new(
        config: OrchestratorConfig,
        graph: Arc<Mutex<DependencyGraph>>,
        pipeline: P,
    ) -> Self {
        Self {
            config,
            graph,
            pipeline,
        }
    }

    /// Borrow the underlying graph (for the resolver to mutate before /
    /// during the run).
    pub fn graph(&self) -> &Arc<Mutex<DependencyGraph>> {
        &self.graph
    }

    /// Borrow the policy.
    pub fn policy(&self) -> &GranularityPolicy {
        &self.config.policy
    }

    /// Build a plan from a list of changed paths. Pure: does not call
    /// the pipeline.
    ///
    /// This is the heart of the orchestrator — split out so tests can
    /// exercise the policy + graph fold without spinning up a watcher.
    pub fn plan_for_changes<I, P2>(&self, changes: I) -> RebuildPlan
    where
        I: IntoIterator<Item = P2>,
        P2: Into<PathBuf>,
    {
        let mut plan = RebuildPlan::empty();
        // Recover from poisoning rather than crashing the long-running
        // dev loop: a panicked previous holder leaves the mutex
        // poisoned, but the graph data itself is still consistent for
        // our purposes. Surface the recovery via warn so the operator
        // notices the upstream panic instead of silently absorbing it.
        let graph = self.graph.lock().unwrap_or_else(|p| {
            warn!(
                site = "plan_for_changes",
                "graph mutex poisoned, recovering"
            );
            p.into_inner()
        });

        let mut changes_iter = changes.into_iter();
        loop {
            let Some(change) = changes_iter.next() else {
                break;
            };
            let path: PathBuf = change.into();
            plan.record_trigger(path.clone());

            let class = classify_change(&path, |p| graph.is_global(p));

            match class {
                PathClass::Global => {
                    // Nuke and pave: every page, every sub-pipeline.
                    // Drain the rest of the iterator into `triggers`
                    // first so we don't lose the paths from later
                    // changes in the same tick — the caller still
                    // wants to know about them for logging.
                    //
                    // `path` is already in `plan.triggers` (recorded by
                    // `plan.record_trigger(path.clone())` above), so the
                    // `mem::take` covers it. Pushing `path` again here
                    // would duplicate it — leave it to the take.
                    let mut full = RebuildPlan::full_rebuild();
                    full.triggers = std::mem::take(&mut plan.triggers);
                    let _ = path;
                    for remaining in changes_iter {
                        full.triggers.push(remaining.into());
                    }
                    return full;
                }
                PathClass::Page | PathClass::Module | PathClass::Content | PathClass::Data => {
                    let dirty: PageSelection = graph.dirty_pages(&path).into();
                    // If the graph returned no dirty pages (empty specific set),
                    // fall back to rebuilding all pages. This handles two cases:
                    //   1. Cold start: graph seeded with page nodes but has no
                    //      reverse edges yet (content→page deps not resolved).
                    //   2. Untracked file: file genuinely isn't in the graph;
                    //      rebuild everything conservatively.
                    // `PageSelection::All` is safe here — `resolve_all` will
                    // expand it to the known page list before the pipeline runs.
                    let effective = if dirty.is_empty() {
                        PageSelection::All
                    } else {
                        dirty
                    };
                    plan.mark_pages(effective);

                    // Modules under an islands root re-bundle islands.
                    if matches!(class, PathClass::Module)
                        && self.config.policy.is_islands_candidate(&path)
                    {
                        plan.mark_islands();
                    }
                }
                PathClass::Style => {
                    plan.mark_css();
                    // CSS sources may also be referenced as deps of pages
                    // (CSS Modules imported from a .tsx). Honour the
                    // graph for those.
                    let dirty: PageSelection = graph.dirty_pages(&path).into();
                    plan.mark_pages(dirty);
                }
                PathClass::Asset => {
                    // No page re-render. The orchestrator does not yet
                    // copy `public/**` into dist/ — that's the
                    // responsibility of the bin crate's setup step.
                    debug!(path = %path.display(), "asset change, no rebuild needed");
                }
                PathClass::Unclassified => {
                    // Defensive: maybe the graph knows about this path
                    // via an explicit dep. If so, dirty those pages; if
                    // not, do nothing.
                    let dirty: PageSelection = graph.dirty_pages(&path).into();
                    plan.mark_pages(dirty);
                }
            }
        }

        plan
    }

    /// Resolve [`PageSelection::All`] to an explicit page list using the
    /// graph's known pages. Required before handing the plan to a
    /// pipeline.
    fn resolve_all(&self, plan: &mut RebuildPlan) {
        if plan.pages.is_all() {
            let graph = self.graph.lock().unwrap_or_else(|p| {
                warn!(site = "resolve_all", "graph mutex poisoned, recovering");
                p.into_inner()
            });
            let pages: std::collections::BTreeSet<_> = graph.pages().into_iter().collect();
            plan.pages = PageSelection::Specific(pages);
        }
    }

    /// One-shot tick: take an iterator of changes, build a plan,
    /// resolve "All" against the graph, hand to the pipeline, return
    /// the outcome.
    ///
    /// Returns `Ok(None)` if the plan was a no-op.
    pub fn tick<I, P2>(&self, changes: I, ctx: &BuildContext) -> Result<Option<BuildOutcome>>
    where
        I: IntoIterator<Item = P2>,
        P2: Into<PathBuf>,
    {
        let mut plan = self.plan_for_changes(changes);
        if plan.is_noop() {
            return Ok(None);
        }
        self.resolve_all(&mut plan);
        // After resolution an `All` plan can become an empty plan (the
        // graph has zero pages). Treat that as a no-op rather than
        // erroring out of the pipeline.
        if plan.pages.is_empty() && !plan.rerun_css && !plan.rerun_islands {
            return Ok(None);
        }
        let outcome = self.pipeline.apply(&plan, ctx)?;
        Ok(Some(outcome))
    }

    /// Long-running dev loop: spawn a watcher, drain change events, and
    /// invoke the pipeline once per debounced burst.
    ///
    /// `on_outcome` is called after every non-noop tick — typically the
    /// dev preview server uses this to push a websocket reload signal.
    pub async fn run<F>(self, ctx: BuildContext, mut on_outcome: F) -> Result<()>
    where
        F: FnMut(&BuildOutcome) + Send + 'static,
    {
        let (watcher, mut rx) = match self.config.debounce {
            Some(d) => Watcher::start_with_debounce(
                &self.config.project_root,
                self.config.watch_roots.iter().map(|p| p.as_path()),
                d,
            )?,
            None => Watcher::start(
                &self.config.project_root,
                self.config.watch_roots.iter().map(|p| p.as_path()),
            )?,
        };

        info!(
            project_root = %self.config.project_root.display(),
            "build orchestrator running"
        );

        // Drain loop: wait for the first event, then drain everything
        // currently in the channel so we coalesce concurrent bursts
        // into one tick.
        while let Some(first) = rx.recv().await {
            let mut batch: Vec<Change> = vec![first];
            while let Ok(c) = rx.try_recv() {
                batch.push(c);
            }

            let paths: Vec<PathBuf> = batch.iter().map(|c| c.path.clone()).collect();
            match self.tick(paths, &ctx) {
                Ok(Some(outcome)) => on_outcome(&outcome),
                Ok(None) => {
                    debug!("rebuild tick was a no-op");
                }
                Err(e) => {
                    warn!(error = %e, "rebuild tick failed; watcher staying alive");
                }
            }
        }

        // Channel closed = watcher dropped from elsewhere. Returning Ok
        // lets the caller decide whether to re-spawn or exit.
        drop(watcher);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::BuildOutcome;
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use zfb_graph::{DepKind, PageDeps, PageId};

    /// Test pipeline that records what it was asked to do.
    #[derive(Debug, Default, Clone)]
    struct CountingPipeline {
        applies: Arc<Mutex<Vec<RebuildPlan>>>,
    }

    impl AssetPipeline for CountingPipeline {
        fn apply(&self, plan: &RebuildPlan, _ctx: &BuildContext) -> Result<BuildOutcome> {
            self.applies.lock().unwrap().push(plan.clone());
            Ok(BuildOutcome::default())
        }
    }

    fn pid(s: &str) -> PageId {
        PageId::new(PathBuf::from(s))
    }

    fn make_graph() -> Arc<Mutex<DependencyGraph>> {
        let mut g = DependencyGraph::new();
        g.upsert(PageDeps::new(
            pid("/proj/pages/a.tsx"),
            vec![(
                PathBuf::from("/proj/components/Header.tsx"),
                DepKind::Module,
            )],
        ));
        g.upsert(PageDeps::new(
            pid("/proj/pages/b.tsx"),
            vec![(
                PathBuf::from("/proj/components/Header.tsx"),
                DepKind::Module,
            )],
        ));
        g.upsert(PageDeps::new(
            pid("/proj/pages/c.tsx"),
            vec![(PathBuf::from("/proj/content/post.md"), DepKind::Content)],
        ));
        g.mark_global(PathBuf::from("/proj/zfb.config.ts"));
        Arc::new(Mutex::new(g))
    }

    fn make_orch<P: AssetPipeline>(pipeline: P) -> BuildOrchestrator<P> {
        BuildOrchestrator::new(
            OrchestratorConfig::new(
                "/proj",
                vec![PathBuf::from("pages"), PathBuf::from("content")],
            ),
            make_graph(),
            pipeline,
        )
    }

    #[test]
    fn page_change_dirties_only_that_page() {
        let orch = make_orch(CountingPipeline::default());
        let plan = orch.plan_for_changes(vec![PathBuf::from("/proj/pages/a.tsx")]);
        match plan.pages {
            PageSelection::Specific(s) => {
                assert!(s.contains(&pid("/proj/pages/a.tsx")));
                assert!(!s.contains(&pid("/proj/pages/b.tsx")));
            }
            other => unreachable!("expected PageSelection::Specific, got {other:?}"),
        }
        assert!(!plan.rerun_css);
        assert!(!plan.rerun_islands);
    }

    #[test]
    fn shared_component_dirties_all_consumers() {
        let orch = make_orch(CountingPipeline::default());
        let plan = orch.plan_for_changes(vec![PathBuf::from("/proj/components/Header.tsx")]);
        match plan.pages {
            PageSelection::Specific(s) => {
                assert!(s.contains(&pid("/proj/pages/a.tsx")));
                assert!(s.contains(&pid("/proj/pages/b.tsx")));
                assert!(!s.contains(&pid("/proj/pages/c.tsx")));
            }
            other => unreachable!("expected PageSelection::Specific, got {other:?}"),
        }
        // components/ is in default islands roots -> islands rerun.
        assert!(plan.rerun_islands);
        assert!(!plan.rerun_css);
    }

    #[test]
    fn css_change_triggers_css_pipeline_only() {
        let orch = make_orch(CountingPipeline::default());
        let plan = orch.plan_for_changes(vec![PathBuf::from("/proj/styles/main.css")]);
        assert!(plan.rerun_css);
        assert!(!plan.rerun_islands);
        // No pages depend on this CSS file directly.
        assert!(plan.pages.is_empty());
    }

    #[test]
    fn global_change_promotes_to_full_rebuild() {
        let orch = make_orch(CountingPipeline::default());
        let plan = orch.plan_for_changes(vec![PathBuf::from("/proj/zfb.config.ts")]);
        assert!(plan.pages.is_all());
        assert!(plan.rerun_css);
        assert!(plan.rerun_islands);
    }

    /// Round 2 regression guard: when a Global change fires, each
    /// trigger path must appear exactly once in `plan.triggers`. The
    /// previous code recorded the change once via `record_trigger` and
    /// then re-pushed the same path into `full.triggers`, doubling it.
    #[test]
    fn global_change_does_not_duplicate_trigger_paths() {
        let orch = make_orch(CountingPipeline::default());
        let cfg = PathBuf::from("/proj/zfb.config.ts");
        let other = PathBuf::from("/proj/pages/a.tsx");
        let plan = orch.plan_for_changes(vec![cfg.clone(), other.clone()]);
        assert_eq!(plan.triggers, vec![cfg, other]);
    }

    #[test]
    fn content_change_dirties_consumer_pages() {
        let orch = make_orch(CountingPipeline::default());
        let plan = orch.plan_for_changes(vec![PathBuf::from("/proj/content/post.md")]);
        match plan.pages {
            PageSelection::Specific(s) => {
                assert_eq!(s, BTreeSet::from([pid("/proj/pages/c.tsx")]));
            }
            other => unreachable!("expected PageSelection::Specific, got {other:?}"),
        }
    }

    #[test]
    fn unclassified_change_with_no_consumers_is_noop() {
        let orch = make_orch(CountingPipeline::default());
        let plan = orch.plan_for_changes(vec![PathBuf::from("/proj/some-random-thing")]);
        assert!(plan.is_noop());
    }
}
