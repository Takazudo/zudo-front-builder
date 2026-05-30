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

    /// Extra absolute paths to watch in addition to `watch_roots`.
    ///
    /// Sourced from `Config::extra_watch_paths` and handed through
    /// verbatim — the caller (the `zfb dev` command layer) is
    /// responsible for canonicalisation + missing-at-boot policy
    /// before populating this. See [`zfb_watcher::Watcher::start_with_extras`].
    ///
    /// Events from these paths fall outside the dependency graph's
    /// coverage and conservatively trigger broader rebuilds — that is
    /// the documented contract for the public `extraWatchPaths`
    /// config field.
    pub extra_watch_paths: Vec<PathBuf>,

    /// Granularity policy. Defaults to [`GranularityPolicy::default`].
    pub policy: GranularityPolicy,

    /// Optional override for the watcher debounce window. `None` =
    /// `zfb_watcher::DEFAULT_DEBOUNCE` (50ms).
    pub debounce: Option<Duration>,
}

impl OrchestratorConfig {
    /// Convenience: build a config from `(project_root, watch_roots)`
    /// with the default policy and debounce, and no extra watch paths.
    pub fn new(project_root: impl Into<PathBuf>, watch_roots: Vec<PathBuf>) -> Self {
        Self {
            project_root: project_root.into(),
            watch_roots,
            extra_watch_paths: Vec::new(),
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

    /// Set the extra (absolute) watch paths (chainable). Each entry is
    /// expected to be absolute and already canonicalised by the caller.
    pub fn with_extra_watch_paths(mut self, paths: Vec<PathBuf>) -> Self {
        self.extra_watch_paths = paths;
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

            let class = classify_change(&path, &self.config.project_root, |p| graph.is_global(p));

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
                PathClass::External => {
                    // An out-of-root file change from the
                    // `extraWatchPaths` channel (issue #368). The user
                    // opted in to watching that path; the graph has no
                    // edges for it so consulting `dirty_pages` is a
                    // no-op. Trigger a conservative `PageSelection::All`
                    // rebuild so edits to e.g. `logo.png` or
                    // `schema.graphql` under an extra watch root
                    // actually re-render. Deep-review fix (PR #376).
                    plan.mark_pages(PageSelection::All);
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

    /// Eager initial render of **every** page in the graph, run once at
    /// dev-server boot before the watcher loop and before the server
    /// starts accepting requests.
    ///
    /// `run()` is purely watcher-driven: it only renders a page after a
    /// file-change event. Without this call the dev pipeline never
    /// populates `.zfb-build/dev-pages/` until the user edits a file, so
    /// a fresh `zfb dev` 404s every route until the first edit (zfb#642 /
    /// #644). `zfb build` was unaffected because it never goes through
    /// the orchestrator at all.
    ///
    /// Pages-only by design: the dev command already bundles CSS and
    /// islands eagerly at boot (the #494 / #377 wiring) before
    /// constructing the orchestrator, so re-running those sub-pipelines
    /// here would be redundant work. We force `PageSelection::All` (not a
    /// change-derived plan) so the result does not depend on the graph's
    /// reverse-edge state — the seeded page nodes are enough.
    ///
    /// Returns `Ok(None)` only when the graph has zero pages (nothing to
    /// render); otherwise `Ok(Some(outcome))` whose `pages_rendered`
    /// count the caller checks to detect a silent zero-page render.
    pub fn initial_build(&self, ctx: &BuildContext) -> Result<Option<BuildOutcome>> {
        let mut plan = RebuildPlan::empty();
        plan.mark_pages(PageSelection::All);
        self.resolve_all(&mut plan);
        if plan.pages.is_empty() {
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
        let debounce = self
            .config
            .debounce
            .unwrap_or(zfb_watcher::DEFAULT_DEBOUNCE);
        let (watcher, mut rx) = Watcher::start_with_extras(
            &self.config.project_root,
            self.config.watch_roots.iter().map(|p| p.as_path()),
            self.config.extra_watch_paths.iter().map(|p| p.as_path()),
            debounce,
        )?;

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

    /// Deep-review regression (PR #376): a file change OUTSIDE the
    /// project root with a non-whitelisted extension (e.g. `logo.png`,
    /// `schema.graphql`) used to silently no-op because the
    /// `Unclassified` branch in `plan_for_changes` doesn't fall back to
    /// `PageSelection::All`. The `External` variant added in the same
    /// fix triggers a conservative full rebuild so the
    /// `extraWatchPaths` feature actually fires when the watcher does.
    #[test]
    fn external_change_with_non_whitelisted_extension_triggers_full_rebuild() {
        let orch = make_orch(CountingPipeline::default());
        let plan = orch.plan_for_changes(vec![PathBuf::from("/srv/shared/assets/logo.png")]);
        assert!(
            plan.pages.is_all(),
            "external file with non-whitelisted extension must trigger PageSelection::All; got {:?}",
            plan.pages,
        );
        // The orchestrator's policy doesn't decide CSS/islands re-runs
        // for External — only pages. CSS / islands stay false unless
        // the user's other changes set them.
    }

    /// Sister regression: a file with NO extension at all under an
    /// extra watch root (e.g. `Makefile`, `Dockerfile`) also re-routes
    /// through `External` and triggers a full rebuild.
    #[test]
    fn external_change_with_no_extension_triggers_full_rebuild() {
        let orch = make_orch(CountingPipeline::default());
        let plan = orch.plan_for_changes(vec![PathBuf::from("/srv/shared/Makefile")]);
        assert!(plan.pages.is_all());
    }

    /// Regression for zfb#642 / #644 — `zfb dev` 404'd every route on a
    /// fresh boot because nothing rendered pages into the dev cache until
    /// the user edited a file: `run()` is watcher-driven and there was no
    /// eager initial render. `initial_build` closes that gap.
    ///
    /// This drives the REAL [`DevAssetPipeline`] (not a stub) so the test
    /// exercises the actual render → atomic-write path the dev server
    /// reads back from disk. A stub `render_pages` returns one
    /// `RenderedPage` per requested page id (no V8 needed). The assertion
    /// is the capability the pre-fix code lacked: with ZERO file-change
    /// events, `initial_build` resolves `PageSelection::All` against the
    /// seeded graph, renders every page, and writes its HTML to
    /// `dist_root`.
    ///
    /// Falsifiability: the pre-fix orchestrator had no `initial_build`;
    /// the only render entry point was `tick`/`run`, both gated on a file
    /// change. A no-op initial build (or one that resolved to zero pages)
    /// makes `pages_rendered == 0` and leaves the dist dir empty, failing
    /// every assertion below.
    #[test]
    fn initial_build_renders_all_seeded_pages_with_no_file_events() {
        use crate::pipeline::{BuildContext, RelDistPath, RenderedPage};
        use crate::pipeline::dev::DevAssetPipeline;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tempfile::tempdir;

        // Seed the graph exactly as `dev.rs` does: one zero-dep page node
        // per route the router scan discovered, with NO reverse edges yet
        // (the cold-start state).
        let mut g = DependencyGraph::new();
        let seeded = [
            "pages/index.tsx",
            "pages/about.md",
            "pages/posts/[slug].tsx",
        ];
        for p in seeded {
            g.upsert(PageDeps::new(pid(p), vec![]));
        }
        let graph = Arc::new(Mutex::new(g));

        let dist = tempdir().expect("tempdir");
        let orch = BuildOrchestrator::new(
            OrchestratorConfig::new("/proj", vec![PathBuf::from("pages")]),
            graph,
            DevAssetPipeline::new(),
        );

        // Stub renderer: emit one RenderedPage per requested page id,
        // writing each to a deterministic `<id>.html`. Records how many
        // pages it was asked to render so we can prove the initial build
        // drove a non-empty render (and didn't just no-op).
        let render_calls = Arc::new(AtomicUsize::new(0));
        let render_calls_cb = render_calls.clone();
        let ctx = BuildContext {
            dist_root: dist.path().to_path_buf(),
            render_pages: Arc::new(move |pages: &[PageId]| {
                render_calls_cb.fetch_add(pages.len(), Ordering::SeqCst);
                Ok(pages
                    .iter()
                    .enumerate()
                    .map(|(i, page)| RenderedPage {
                        page: page.clone(),
                        output_path: RelDistPath::new(format!("p{i}.html")).unwrap(),
                        html: format!("<html>{}</html>", page.path().display()),
                        content_type: None,
                    })
                    .collect())
            }),
            run_css: None,
            run_islands: None,
            reload_renderer: None,
        };

        // The whole point: no file-change events are passed. Pre-fix this
        // capability did not exist; the dev cache stayed empty until an
        // edit. `initial_build` renders everything up front.
        let outcome = orch
            .initial_build(&ctx)
            .expect("initial_build must succeed")
            .expect("a graph with pages must render at least one page");

        assert_eq!(
            outcome.pages_rendered,
            seeded.len(),
            "initial build must render every seeded page; got {}",
            outcome.pages_rendered,
        );
        assert_eq!(
            render_calls.load(Ordering::SeqCst),
            seeded.len(),
            "the render callback must be asked for every seeded page",
        );

        // The pipeline must have written the HTML to disk — this is what
        // the dev server's `read_from_dist` fallback serves. An empty
        // dist dir is the exact pre-fix 404 symptom.
        let written: Vec<_> = std::fs::read_dir(dist.path())
            .expect("dist dir readable")
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "html").unwrap_or(false))
            .collect();
        assert_eq!(
            written.len(),
            seeded.len(),
            "every rendered page must be written to dist for the dev server to serve",
        );
    }

    /// `initial_build` on an empty graph (no pages) is a clean no-op,
    /// not an error — the dev server still boots (e.g. an SSR-only
    /// project) so the user can poke at it.
    #[test]
    fn initial_build_on_empty_graph_is_noop() {
        use crate::pipeline::BuildContext;
        use tempfile::tempdir;

        let graph = Arc::new(Mutex::new(DependencyGraph::new()));
        let dist = tempdir().expect("tempdir");
        let orch = BuildOrchestrator::new(
            OrchestratorConfig::new("/proj", vec![PathBuf::from("pages")]),
            graph,
            CountingPipeline::default(),
        );
        let ctx = BuildContext {
            dist_root: dist.path().to_path_buf(),
            render_pages: Arc::new(|_| Ok(vec![])),
            run_css: None,
            run_islands: None,
            reload_renderer: None,
        };
        assert!(
            orch.initial_build(&ctx).expect("must not error").is_none(),
            "empty graph must yield Ok(None)",
        );
    }
}
