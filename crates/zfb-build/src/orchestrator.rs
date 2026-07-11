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

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use tracing::{debug, info, warn};
use zfb_graph::{DependencyGraph, PageId};
use zfb_watcher::{Change, ChangeKind, Watcher};

use crate::pipeline::{AssetPipeline, BuildContext, BuildOutcome};
use crate::plan::{PageSelection, RebuildPlan};
use crate::policy::{classify_change_with_content_roots, GranularityPolicy, PathClass};

/// Register non-recursive parent watches for browser dependency closures
/// discovered outside the configured recursive source roots.
fn register_dynamic_dependency_watches(watcher: &mut Watcher, policy: &GranularityPolicy) {
    watcher.watch_additional_files(policy.dynamic_dependency_paths());
}

/// `ZFB_DEV_TIMING` gate for the per-tick kind/narrowing trace (issue #1058).
/// Same env var and truthy parser as `bundler_timing_enabled` and
/// `crates/zfb/src/commands/dev.rs::dev_timing_enabled`, so one flag turns on
/// the whole timing story. Unset/empty/unrecognized → off (zero hot-path cost).
fn dev_timing_enabled() -> bool {
    std::env::var("ZFB_DEV_TIMING")
        .ok()
        .as_deref()
        .map(|raw| {
            let t = raw.trim();
            t.eq_ignore_ascii_case("1") || t.eq_ignore_ascii_case("true")
        })
        .unwrap_or(false)
}

/// Live watch-ADD discovery hook (issue #659).
///
/// Invoked from [`BuildOrchestrator::tick_with_kinds`] /
/// [`BuildOrchestrator::run`] with the subset of a tick's changed paths
/// whose [`ChangeKind`] is [`ChangeKind::Created`]. The implementation
/// (in the `zfb dev` command layer) is responsible for the side-effecting
/// "the running renderer cannot see a file created after boot" fix:
/// recompute the content snapshot, re-bundle the SSR worker, reload the
/// embedded V8 host **in place**, re-enumerate `paths()`, and rebuild the
/// dev session's source→route table. It returns the source [`PageId`]s
/// that newly became renderable so the orchestrator can fold them into
/// the tick's [`RebuildPlan`] and render them through the same
/// page-render path an edit tick uses.
///
/// Why a separate hook rather than [`BuildContext::reload_renderer`]:
/// the discovery hook additionally rediscovers routes and reports the
/// newly-renderable pages back into the tick's plan — `reload_renderer`
/// is a fire-and-forget refresh with no way to surface discovered pages.
/// The two cooperate via [`DiscoveryOutcome::renderer_reloaded`] /
/// [`crate::RebuildPlan::renderer_fresh`]: when the hook's re-bundle
/// already refreshed the renderer this tick, the pipeline skips its own
/// `reload_renderer` call instead of bundling twice.
///
/// Returning an empty page set (no created path mapped to a discoverable
/// page) folds into the tick as a no-op for discovery — the rest of the
/// tick's changes are still planned normally.
pub type DiscoveryHook =
    std::sync::Arc<dyn Fn(&[PathBuf]) -> Result<DiscoveryOutcome> + Send + Sync + 'static>;

/// Opt-in external-change narrowing hook (issue #1038).
///
/// Consulted from [`BuildOrchestrator::plan_for_changes`] for every
/// **out-of-root** change — a path that does not live under
/// `project_root`, which is exactly the `extraWatchPaths` channel. The
/// orchestrator has no dependency-graph edges for out-of-root paths, so by
/// default such a change conservatively triggers `PageSelection::All`.
/// This hook lets an *informed* consumer — one that knows which external
/// path backs which page (e.g. skill file `foo.mdx` → page `/skills/foo`)
/// — narrow that to a specific subset:
///
/// - `Some(pages)` → re-render exactly those pages (folded into
///   [`PageSelection::Specific`]). An empty `Some(vec![])` narrows to "no
///   pages" — the consumer asserts this external change affects nothing
///   page-renderable.
/// - `None` → the consumer cannot map this path; fall back to the
///   conservative `PageSelection::All` rebuild (the unchanged default).
///
/// Keyed on **out-of-root**, not on [`PathClass::External`]: a watched
/// external file with a whitelisted extension (e.g. `foo.mdx`) classifies
/// as [`PathClass::Content`], never reaching the `External` arm — yet it
/// is the issue's primary use case, so it must be narrowable too.
///
/// When no hook is configured ([`OrchestratorConfig::external_invalidation`]
/// is `None`) every external change keeps the `PageSelection::All` default
/// — there is no behavior change for existing consumers. The hook only
/// ever narrows; it cannot suppress the SSR host reload, which still fires
/// for every external change regardless of the hook's verdict.
///
/// Mirrors the [`DiscoveryHook`] pattern, but is a *pure* path→pages query
/// with no side effects — so it lives in [`OrchestratorConfig`]. It is
/// invoked in a pre-pass **before** the graph mutex is locked, so a hook
/// that captures the same `Arc<Mutex<DependencyGraph>>` (to derive or
/// validate page ids) cannot deadlock the dev loop.
pub type ExternalInvalidationHook =
    std::sync::Arc<dyn Fn(&Path) -> Option<Vec<PageId>> + Send + Sync + 'static>;

/// What a [`DiscoveryHook`] invocation did for this tick.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryOutcome {
    /// Source pages whose route set changed (newly renderable). Folded
    /// into the tick's plan so they render this tick.
    pub pages: Vec<PageId>,

    /// True when the hook re-bundled and reloaded the renderer in
    /// place. Propagated to [`crate::RebuildPlan::renderer_fresh`] so the
    /// pipeline's per-tick `reload_renderer` call is skipped — one
    /// bundle per tick.
    pub renderer_reloaded: bool,

    /// Absolute dist-root paths that vanished globally from the live
    /// route set during this hook's route-table rebuild (issue #804).
    ///
    /// A Create tick can cause routes to vanish — for example a dynamic
    /// `paths()` that keeps only the N most-recent posts, or an
    /// editor rename delivered as Removed+Created. Because the hook
    /// sets `renderer_reloaded = true`, the pipeline skips its own
    /// `reload_renderer` call and never learns about the vanished paths
    /// through that channel. Carrying them here lets the orchestrator
    /// fold them into [`crate::RebuildPlan::prune_paths`] so the pipeline
    /// still prunes the stale HTML files.
    ///
    /// Values are absolute paths (the hook is responsible for joining
    /// the relative output path with the appropriate dist root).
    pub vanished_output_paths: Vec<PathBuf>,
}

/// Construction-time configuration for [`BuildOrchestrator`].
///
/// `Debug` is hand-written rather than derived because
/// [`external_invalidation`](Self::external_invalidation) holds a boxed
/// closure ([`ExternalInvalidationHook`]) which is not `Debug`.
#[derive(Clone)]
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
    ///
    /// That conservative `PageSelection::All` rebuild remains the
    /// **default**. An informed consumer who knows which external path
    /// backs which page can opt in to narrowing via
    /// [`external_invalidation`](Self::external_invalidation) — the
    /// documented escape hatch (issue #1038).
    pub extra_watch_paths: Vec<PathBuf>,

    /// Granularity policy. Defaults to [`GranularityPolicy::default`].
    pub policy: GranularityPolicy,

    /// Optional override for the watcher debounce window. `None` =
    /// `zfb_watcher::DEFAULT_DEBOUNCE` (50ms).
    pub debounce: Option<Duration>,

    /// Opt-in external-change narrowing hook (issue #1038).
    ///
    /// `None` (the default) keeps the conservative contract: every
    /// `extraWatchPaths` change triggers a full `PageSelection::All`
    /// rebuild. `Some(hook)` lets an informed consumer narrow an external
    /// change to the specific pages it backs — see
    /// [`ExternalInvalidationHook`] for the `Some`/`None` semantics. The
    /// hook only ever narrows the page set; the SSR host reload still
    /// fires for every external change.
    pub external_invalidation: Option<ExternalInvalidationHook>,
}

impl std::fmt::Debug for OrchestratorConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrchestratorConfig")
            .field("project_root", &self.project_root)
            .field("watch_roots", &self.watch_roots)
            .field("extra_watch_paths", &self.extra_watch_paths)
            .field("policy", &self.policy)
            .field("debounce", &self.debounce)
            .field(
                "external_invalidation",
                &self.external_invalidation.as_ref().map(|_| "<hook>"),
            )
            .finish()
    }
}

impl OrchestratorConfig {
    /// Convenience: build a config from `(project_root, watch_roots)`
    /// with the default policy and debounce, no extra watch paths, and
    /// no external-invalidation hook.
    pub fn new(project_root: impl Into<PathBuf>, watch_roots: Vec<PathBuf>) -> Self {
        Self {
            project_root: project_root.into(),
            watch_roots,
            extra_watch_paths: Vec::new(),
            policy: GranularityPolicy::default(),
            debounce: None,
            external_invalidation: None,
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

    /// Set the opt-in external-change narrowing hook (chainable, issue
    /// #1038). See [`ExternalInvalidationHook`]. Without this, external
    /// changes keep the conservative `PageSelection::All` default.
    pub fn with_external_invalidation(mut self, hook: ExternalInvalidationHook) -> Self {
        self.external_invalidation = Some(hook);
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

        // Materialise the change set so we can run the external-invalidation
        // hook in a pre-pass (no lock held) before the graph-locked fold.
        let paths: Vec<PathBuf> = changes.into_iter().map(Into::into).collect();

        // Opt-in external-change narrowing pre-pass (issue #1038).
        //
        // Consult the consumer's `external_invalidation` hook for every
        // OUT-OF-ROOT change — i.e. a path that does not live under
        // `project_root`, which is exactly the `extraWatchPaths` channel.
        // Keying on out-of-root rather than on `PathClass::External`
        // matters: a watched external file with a whitelisted extension
        // (e.g. a skill `foo.mdx`) classifies as `Content`/`Module`/…,
        // never reaching the `External` arm — yet it is the issue's
        // primary `foo.mdx → /skills/foo` use case. Intercepting here
        // makes those narrowable too.
        //
        // CRITICAL: the hook is invoked here, BEFORE the graph mutex is
        // locked below. A consumer hook commonly captures the same
        // `Arc<Mutex<DependencyGraph>>` handed to `BuildOrchestrator::new`
        // (to derive or validate page ids); calling it under the graph
        // lock would deadlock the dev loop. Mirrors why `DiscoveryHook`
        // runs outside `plan_for_changes`.
        //
        // Only `Some(pages)` verdicts are recorded. `None` (or no hook)
        // leaves the path absent from the map so the locked fold below
        // applies the unchanged conservative default (`External` → `All`,
        // or the whitelisted-extension class arm's own `All` fallback).
        let external_overrides: std::collections::HashMap<&PathBuf, PageSelection> =
            match self.config.external_invalidation.as_ref() {
                Some(hook) => paths
                    .iter()
                    .filter(|p| p.strip_prefix(&self.config.project_root).is_err())
                    .filter_map(|p| {
                        hook(p)
                            .map(|pages| (p, PageSelection::Specific(pages.into_iter().collect())))
                    })
                    .collect(),
                None => std::collections::HashMap::new(),
            };

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

        let mut changes_iter = paths.iter();
        while let Some(path) = changes_iter.next() {
            let path: PathBuf = path.clone();
            plan.record_trigger(path.clone());

            // External-narrowing override (issue #1038): a configured hook
            // mapped this out-of-root path to a specific page set in the
            // pre-pass above. Apply that page-set verdict and skip the
            // graph-classified arms' OWN page selection — the verdict
            // supersedes the conservative default for this path. The SSR
            // host reload still fires (the hook narrows the page set, never
            // the reload), matching the unchanged `External` contract.
            //
            // The asset-rebuild flags, however, are NOT the hook's to
            // narrow: a hook narrowing an external CSS file or an islands
            // module still needs the corresponding asset rebuild, or the
            // consumer is left with a stale CSS / islands bundle. So we
            // classify the path and additively re-apply the SAME
            // asset-flag side effects (`rerun_css` / `rerun_islands` /
            // client-scripts) the matching class arm would have set — but
            // none of its page-selection side effects.
            if let Some(selection) = external_overrides.get(&path) {
                plan.mark_pages(selection.clone());
                plan.mark_ssr_reload_needed();

                let class = classify_change_with_content_roots(
                    &path,
                    &self.config.project_root,
                    &self.config.policy.content_roots,
                    |p| graph.is_global(p),
                );
                if class == PathClass::Style {
                    plan.mark_css();
                }
                if matches!(class, PathClass::Module)
                    && self.config.policy.is_islands_candidate(&path)
                {
                    plan.mark_islands();
                }
                if self.config.policy.is_islands_dependency(&path) {
                    plan.mark_islands();
                }
                if self.config.policy.is_client_script_candidate(&path)
                    || self.config.policy.is_client_script_raw_target(&path)
                    || self.config.policy.is_client_script_worker_target(&path)
                {
                    plan.mark_client_scripts();
                }
                continue;
            }

            let class = classify_change_with_content_roots(
                &path,
                &self.config.project_root,
                &self.config.policy.content_roots,
                |p| graph.is_global(p),
            );

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
                        full.triggers.push(remaining.clone());
                    }
                    return full;
                }
                PathClass::Page | PathClass::Module | PathClass::Content | PathClass::Data => {
                    let dirty: PageSelection = graph.dirty_pages(&path).into();
                    // Precise per-route selection backed by the dev `Module`
                    // edges populated from esbuild's metafile (#1284/#1287).
                    // Fall back to `PageSelection::All` ONLY when the path is
                    // genuinely UNKNOWN to the graph (no page / dep / global
                    // record) — then a whole-site rebuild is the conservative
                    // choice (cold start, or a file the graph never resolved).
                    // When the graph DOES know the path but maps it to an empty
                    // consumer set, that empty result is authoritative — the
                    // blunt All-fallback on every component edit was exactly the
                    // imprecise whole-site re-render the metafile edges remove.
                    // `PageSelection::All` is still expanded by `resolve_all` to
                    // the known page list before the pipeline runs.
                    let effective = if dirty.is_empty() && !graph.knows(&path) {
                        PageSelection::All
                    } else {
                        dirty
                    };
                    plan.mark_pages(effective);
                    // Content/Page/Module/Data changes may affect SSR output —
                    // flag the plan so the pipeline reloads the V8 host even
                    // on SSR-only projects where pages is always empty (issue #807).
                    plan.mark_ssr_reload_needed();

                    // Modules under an islands root re-bundle islands.
                    if matches!(class, PathClass::Module)
                        && self.config.policy.is_islands_candidate(&path)
                    {
                        plan.mark_islands();
                    }
                    // #1288 — a component (`.tsx` `Module`) edit may author a
                    // new Tailwind utility class (symptom C). The CSS content
                    // scan only re-runs on `rerun_css`, which a `.css` edit
                    // sets today; a `.tsx` edit did not. Re-run the content
                    // scan so a newly-introduced class is emitted into
                    // `/assets/styles.css` without touching the CSS entry.
                    if matches!(class, PathClass::Module) {
                        plan.mark_css();
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
                    //
                    // Opt-in narrowing (issue #1038) is applied EARLIER —
                    // see the `external_overrides` interception at the top
                    // of the loop, which catches every out-of-root path
                    // (not only the non-whitelisted-extension ones that
                    // reach this arm), so a whitelisted external file like
                    // `skills/foo.mdx` is also narrowable.
                    plan.mark_pages(PageSelection::All);
                    plan.mark_ssr_reload_needed();
                }
            }

            // Client-script rebuild trigger — evaluated after the class
            // switch so it fires for ALL three discovery roots:
            //
            // - `components/` and `src/` files classify as `Module`, which
            //   the class match above already handles for islands. Client
            //   scripts under these roots also need the client-scripts pass.
            // - `pages/` files classify as `Page`, NOT `Module`, so the
            //   `is_islands_candidate` gate in the Module branch never fires
            //   for them. We check `is_client_script_candidate` here, outside
            //   the match, so `pages/analytics.client.ts` triggers a rebuild
            //   even though it's classified as a Page change.
            //
            // `Global` already returns early with `full_rebuild()` which sets
            // `rerun_client_scripts = true`, so we never reach this point on a
            // Global change — the `is_client_script_candidate` guard here is
            // therefore only ever evaluated for non-Global changes.
            if self.config.policy.is_islands_dependency(&path) {
                plan.mark_islands();
            }
            if self.config.policy.is_client_script_candidate(&path)
                || self.config.policy.is_client_script_raw_target(&path)
                || self.config.policy.is_client_script_worker_target(&path)
            {
                plan.mark_client_scripts();
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
        // erroring out of the pipeline UNLESS an SSR-only reload is needed
        // (SSR-only projects have zero SSG pages but still require a
        // renderer refresh on every relevant tick — issue #807).
        if plan.pages.is_empty()
            && !plan.rerun_css
            && !plan.rerun_islands
            && !plan.rerun_client_scripts
            && !plan.ssr_reload_needed
        {
            return Ok(None);
        }
        let outcome = self.pipeline.apply(&plan, ctx)?;
        Ok(Some(outcome))
    }

    /// Kind-aware tick (issue #659 — live watch-ADD discovery).
    ///
    /// Additive sibling of [`tick`]: same plan-fold + resolve + apply
    /// pipeline, but it also threads the per-change [`ChangeKind`] so a
    /// file **created** after dev-server boot can be discovered and
    /// rendered without a restart. The path classification / dirty-page
    /// fold is unchanged — it reuses [`plan_for_changes`] verbatim, so
    /// the EDIT path ([`ChangeKind::Modified`]) behaves exactly as
    /// [`tick`] does (the discovery hook is never consulted for it).
    ///
    /// When `discover` is `Some`, the [`ChangeKind::Created`] subset of
    /// `changes` is handed to the hook; any source [`PageId`]s it returns
    /// are merged into the plan's page set so the new page renders through
    /// the same render→write boundary an edit traverses. The hook is
    /// responsible for the side effects (rebundle / reload-in-place /
    /// graph upsert / route-table rebuild) — see [`DiscoveryHook`].
    ///
    /// `tick`'s public signature is intentionally left untouched (it has
    /// many existing call sites); this is the new, explicit entry point
    /// the dev `run` loop uses.
    pub fn tick_with_kinds(
        &self,
        changes: Vec<(PathBuf, ChangeKind)>,
        ctx: &BuildContext,
        discover: Option<&DiscoveryHook>,
    ) -> Result<Option<BuildOutcome>> {
        // Issue #1058 — normalize a spurious `Created` for an already-known
        // content source back to `Modified`. On a loaded arm64 macOS host,
        // FSEvents coalescing can deliver an in-place edit of an EXISTING
        // file as `Created` (see `zfb_watcher::merge_kind` rule 2). Left as
        // `Created` the change routes through the discovery regime (watch-ADD)
        // instead of the in-place-edit regime, so the lazy path never
        // eager-renders the edited entry's own route — the dropped eager
        // render this issue tracks. A path the graph already knows as a
        // content dependency (`consumers_of` is non-empty) cannot be
        // genuinely new, so the `Created` flag is an artifact; a truly new
        // file has no reverse edge yet and stays `Created` for discovery.
        let changes: Vec<(PathBuf, ChangeKind)> = {
            let graph = self.graph.lock().unwrap_or_else(|p| {
                warn!(
                    site = "tick_with_kinds::created_normalize",
                    "graph mutex poisoned, recovering"
                );
                p.into_inner()
            });
            changes
                .into_iter()
                .map(|(path, kind)| {
                    let spurious_created = kind == ChangeKind::Created
                        && graph.consumers_of(&path).is_some_and(|c| !c.is_empty())
                        && classify_change_with_content_roots(
                            &path,
                            &self.config.project_root,
                            &self.config.policy.content_roots,
                            |p| graph.is_global(p),
                        ) == PathClass::Content;
                    if spurious_created {
                        (path, ChangeKind::Modified)
                    } else {
                        (path, kind)
                    }
                })
                .collect()
        };

        // Removal: drop graph edges for deleted files before planning and
        // collect the former consumers so they can be added to the plan.
        //
        // Why collect consumers? The deleted file's route must be pruned
        // from disk. That prune happens in DevAssetPipeline when
        // reload_renderer fires, which only runs when the plan has pages.
        // By folding the former consumers into the plan we guarantee a
        // non-noop tick — even when the deletion is the only change — so
        // reload_renderer runs, refreshes the route table, and returns the
        // vanished paths to the prune loop.
        //
        // Why NOT pass the removed path through plan_for_changes? After
        // remove_node the path has no consumers, so plan_for_changes would
        // fall back to the All-sentinel for an "unknown" path — re-rendering
        // every page rather than just the affected subset. Collecting the
        // affected set from remove_node is the precise, conservative choice.
        //
        // Excluding the removed path from plan_for_changes drops only the
        // page-fallback; its sub-pipeline side effects (CSS / islands / SSR
        // reload) are reinstated explicitly below by classifying each removed
        // path — see the `for path in &removed` loop after the plan is built.
        let removed: Vec<PathBuf> = changes
            .iter()
            .filter(|(_, kind)| *kind == ChangeKind::Removed)
            .map(|(p, _)| p.clone())
            .collect();
        let removed_consumers: std::collections::BTreeSet<PageId> = {
            if removed.is_empty() {
                std::collections::BTreeSet::new()
            } else {
                let mut graph = self.graph.lock().unwrap_or_else(|p| {
                    warn!(
                        site = "tick_with_kinds::remove_node",
                        "graph mutex poisoned, recovering"
                    );
                    p.into_inner()
                });
                let mut affected = std::collections::BTreeSet::new();
                for path in &removed {
                    affected.extend(graph.remove_node(path));
                }
                affected
            }
        };

        // Discovery runs first so a newly-created page is upserted into
        // the graph (by the hook) before `plan_for_changes` folds the
        // change set — and so the discovered page ids survive even when
        // the path-only fold would have produced an empty/no-op plan
        // (a brand-new content file has no reverse edge yet, exactly the
        // #659 symptom).
        let discovered: DiscoveryOutcome = match discover {
            Some(hook) => {
                let created: Vec<PathBuf> = changes
                    .iter()
                    .filter(|(_, kind)| *kind == ChangeKind::Created)
                    .map(|(p, _)| p.clone())
                    .collect();
                if created.is_empty() {
                    DiscoveryOutcome::default()
                } else {
                    hook(&created)?
                }
            }
            None => DiscoveryOutcome::default(),
        };

        // Exclude removed paths from plan_for_changes: after remove_node the
        // path has no consumers. Passing it through plan_for_changes would
        // trigger the cold-start All-fallback ("unknown file → rebuild all")
        // which is wrong for an intentional deletion. The former consumers
        // collected above are added directly to the plan instead.
        let plan_paths: Vec<PathBuf> = changes
            .iter()
            .filter(|(_, kind)| *kind != ChangeKind::Removed)
            .map(|(p, _)| p.clone())
            .collect();
        let mut plan = self.plan_for_changes(plan_paths);

        // Fold former consumers of removed files into the plan so the tick
        // is non-noop even when deletion is the only change. This lets
        // reload_renderer fire and prune the now-vanished HTML.
        if !removed_consumers.is_empty() {
            plan.mark_pages(PageSelection::Specific(removed_consumers));
        }

        // Removed paths are excluded from `plan_for_changes` above (the
        // All-fallback would be wrong for a deletion), but a removal still
        // has the SAME sub-pipeline side effects as a normal change: a
        // deleted stylesheet must rerun CSS, a deleted islands module must
        // rerun islands, a deleted global file must full-rebuild, and any
        // SSR-relevant source (page / module / content / data) must reload
        // the V8 host so SSR-only routes don't serve stale output (#807).
        // Classify each removed path and apply only those rerun/reload flags
        // — NOT the page fallback, which the `removed_consumers` fold already
        // handled precisely. Without this, a deletion-only tick leaves CSS /
        // islands / SSR stale until the next non-removed edit.
        for path in &removed {
            let class = {
                let graph = self.graph.lock().unwrap_or_else(|p| {
                    warn!(
                        site = "tick_with_kinds::classify_removed",
                        "graph mutex poisoned, recovering"
                    );
                    p.into_inner()
                });
                classify_change_with_content_roots(
                    path,
                    &self.config.project_root,
                    &self.config.policy.content_roots,
                    |p| graph.is_global(p),
                )
            };
            match class {
                PathClass::Global => {
                    // A deleted global file invalidates everything. Mirror
                    // `RebuildPlan::full_rebuild`'s sub-pipeline flags; pages
                    // come from `resolve_all` over the (post-removal) graph.
                    plan.mark_pages(PageSelection::All);
                    plan.mark_css();
                    plan.mark_islands();
                    plan.mark_ssr_reload_needed();
                }
                PathClass::Page | PathClass::Module | PathClass::Content | PathClass::Data => {
                    plan.mark_ssr_reload_needed();
                    if matches!(class, PathClass::Module)
                        && self.config.policy.is_islands_candidate(path)
                    {
                        plan.mark_islands();
                    }
                }
                PathClass::Style => {
                    plan.mark_css();
                }
                PathClass::External => {
                    plan.mark_ssr_reload_needed();
                }
                PathClass::Asset | PathClass::Unclassified => {}
            }
            if self.config.policy.is_islands_dependency(path) {
                plan.mark_islands();
            }
            if self.config.policy.is_client_script_raw_target(path)
                || self.config.policy.is_client_script_worker_target(path)
            {
                plan.mark_client_scripts();
            }
        }

        if discovered.renderer_reloaded {
            plan.mark_renderer_fresh();
        }
        if !discovered.pages.is_empty() {
            let set: std::collections::BTreeSet<PageId> = discovered.pages.into_iter().collect();
            plan.mark_pages(PageSelection::Specific(set));
        }
        // Thread vanished output paths from the discovery refresh into
        // the plan's prune list (issue #804 — P2). The discovery hook
        // sets `renderer_fresh`, so the pipeline skips `reload_renderer`
        // and would otherwise miss these vanished paths entirely.
        if !discovered.vanished_output_paths.is_empty() {
            plan.add_prune_paths(discovered.vanished_output_paths);
        }

        // Content-narrowing hint (issues #958 / #1058). Classify each
        // change once (content vs not) under a single graph lock; the
        // classifier runs with the same config `plan_for_changes` uses, so
        // the hint can never disagree with the plan fold.
        let content_flags: Vec<bool> = {
            let graph = self.graph.lock().unwrap_or_else(|p| {
                warn!(
                    site = "tick_with_kinds::content_narrowing",
                    "graph mutex poisoned, recovering"
                );
                p.into_inner()
            });
            changes
                .iter()
                .map(|(path, _)| {
                    classify_change_with_content_roots(
                        path,
                        &self.config.project_root,
                        &self.config.policy.content_roots,
                        |p| graph.is_global(p),
                    ) == PathClass::Content
                })
                .collect()
        };

        // Strict #958 gate (`fan_out_safe`): the tick consists EXCLUSIVELY
        // of in-place `Modified` edits to content files. The eager fan-out
        // narrowing requires this — a mixed tick (Created/Removed/Global/
        // module change) can affect every page, so narrowing it would
        // under-render (fallback G1).
        let modified_only_content = !changes.is_empty()
            && changes
                .iter()
                .zip(&content_flags)
                .all(|((_, kind), is_content)| *kind == ChangeKind::Modified && *is_content);

        // Permissive lazy eager basis (issue #1058): content files edited as
        // `Modified` OR `Created`. `Created` is included because under
        // FSEvents coalescing on a loaded arm64 macOS host an in-place edit
        // of an EXISTING file can arrive as `Created` (see
        // `zfb_watcher::merge_kind` rule 2), which would otherwise defeat
        // the all-`Modified` gate and drop the eager render. Brand-new files
        // are also `Created`+content but carry no routes yet, so the
        // downstream route-table match renders nothing eager for them
        // (discovery owns new routes). `Removed` content is excluded (prune
        // regime).
        let edited_content: Vec<PathBuf> = changes
            .iter()
            .zip(&content_flags)
            .filter(|((_, kind), is_content)| {
                **is_content && matches!(kind, ChangeKind::Modified | ChangeKind::Created)
            })
            .map(|((path, _), _)| path.clone())
            .collect();

        if !edited_content.is_empty() && !plan.pages.is_empty() {
            plan.content_narrowing = Some(crate::plan::ContentNarrowing {
                changed_content: edited_content,
                fan_out_safe: modified_only_content,
            });
        }

        // `ZFB_DEV_TIMING=1` — surface the per-tick change kinds and the
        // resulting narrowing decision (issue #1058, extending the #1028
        // tick-class instrumentation). On a loaded arm64 macOS host an
        // in-place content edit can arrive as `Created` (FSEvents coalescing,
        // see `zfb_watcher::merge_kind` rule 2), which fails the all-`Modified`
        // `modified_only_content` gate → no narrowing → the lazy path marks the
        // route stale instead of eager-rendering it (the dropped eager render
        // #1058 is about). A `narrowing=false` line whose kinds include a
        // `Created` for an already-known content file is the smoking gun.
        // Behind the existing flag so the hot path keeps zero overhead.
        if dev_timing_enabled() {
            let kinds: Vec<String> = changes
                .iter()
                .map(|(p, k)| {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                    format!("{name}:{k:?}")
                })
                .collect();
            eprintln!(
                "[zfb-timing] tick(): kinds=[{}] eager_hint={} fan_out_safe={}",
                kinds.join(", "),
                plan.content_narrowing.is_some(),
                plan.content_narrowing
                    .as_ref()
                    .is_some_and(|n| n.fan_out_safe)
            );
        }

        if plan.is_noop() {
            return Ok(None);
        }
        self.resolve_all(&mut plan);
        if plan.pages.is_empty()
            && !plan.rerun_css
            && !plan.rerun_islands
            && !plan.rerun_client_scripts
            && !plan.ssr_reload_needed
            && plan.prune_paths.is_empty()
        {
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
    /// Pages-only by design: in dev, CSS is bundled eagerly at boot (the
    /// #494 wiring), and the islands bundle is produced out-of-band by the
    /// deferred boot task (the #377 / #1170 wiring — no longer before the
    /// orchestrator, but it never flows through this `apply` either), so
    /// re-running those sub-pipelines here would be redundant work. We force
    /// `PageSelection::All` (not a change-derived plan) so the result does
    /// not depend on the graph's reverse-edge state — the seeded page nodes
    /// are enough.
    ///
    /// Returns `Ok(None)` only when the graph has zero pages (nothing to
    /// render); otherwise `Ok(Some(outcome))` whose `pages_rendered`
    /// count the caller checks to detect a silent zero-page render.
    pub fn initial_build(&self, ctx: &BuildContext) -> Result<Option<BuildOutcome>> {
        let mut plan = RebuildPlan::empty();
        plan.mark_pages(PageSelection::All);
        // The dev renderer's page bundle is built eagerly at boot
        // (`boot_dev_renderer`, before the bind) — the renderer is already
        // bound to a fresh bundle, so the pipeline must not re-bundle it
        // again for the initial render. (This is the V8 page bundle, distinct
        // from the islands bundle, which the deferred boot task builds
        // out-of-band — issue #1170.)
        plan.mark_renderer_fresh();
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
    ///
    /// `discover` is the live watch-ADD discovery hook (issue #659).
    /// `None` keeps the legacy behaviour (a file created after boot 404s
    /// until restart); the `zfb dev` command passes `Some(..)` so a
    /// brand-new content file is rebundled + rediscovered in place. The
    /// hook only ever sees [`ChangeKind::Created`] paths, so the edit
    /// path is unaffected.
    ///
    /// Thin wrapper over [`run_with_boot`](Self::run_with_boot) with no
    /// boot hook — i.e. the legacy "watcher-driven only" behaviour, kept
    /// for callers (mostly tests) that drive a render through file-change
    /// events rather than an eager boot render.
    pub async fn run<F>(
        self,
        ctx: BuildContext,
        discover: Option<DiscoveryHook>,
        on_outcome: F,
    ) -> Result<()>
    where
        F: FnMut(&BuildOutcome) + Send + 'static,
        P: 'static,
    {
        self.run_with_boot(
            ctx,
            discover,
            on_outcome,
            None::<fn(&BuildOrchestrator<P>, &BuildContext) -> Option<BuildOutcome>>,
        )
        .await
    }

    /// [`run`](Self::run), but with a one-shot `boot` hook that runs
    /// AFTER the watcher's OS-level `watch()` is registered yet BEFORE the
    /// drain loop begins (issue #1166 startup-race fixes).
    ///
    /// Two startup races this ordering closes:
    ///
    /// 1. **Missed-edit window (Finding 2).** Before this, the dev command
    ///    ran its eager boot render and only THEN called `run`, which
    ///    registered the notify watch. A source edit saved in that window
    ///    was observed by nobody — `notify` was not yet watching — so the
    ///    dev server kept serving pre-save output until the *next* FS event.
    ///    Registering `watch()` first means `notify` buffers any event from
    ///    the boot-render window into its channel; the drain loop below
    ///    picks it up on its first `recv()` and re-renders the edited route.
    ///    The boot render writes only to the dev HTML root (NOT a watched
    ///    source root), so it cannot trigger a spurious self-tick — there is
    ///    no boot-vs-watcher double-render of the same routes from the boot
    ///    render's own writes. A *genuine* edit in the window does render
    ///    twice (boot render of all pages, then the watcher tick for the
    ///    edited route) — which is correct: the second render carries the
    ///    new bytes and is the authoritative result.
    ///
    /// 2. **Reload-after-boot-render (Finding 1).** The boot render's
    ///    outcome is fed through the SAME `on_outcome` path as a watcher
    ///    tick, so a browser that requested a route during the pre-render
    ///    window (and received the dev 404 page, which carries the
    ///    live-reload script) auto-refreshes the instant the eager render
    ///    lands. Returning `None` from `boot` (e.g. boot-lazy, which renders
    ///    on first request) broadcasts nothing.
    pub async fn run_with_boot<F, B>(
        self,
        ctx: BuildContext,
        discover: Option<DiscoveryHook>,
        mut on_outcome: F,
        boot: Option<B>,
    ) -> Result<()>
    where
        F: FnMut(&BuildOutcome) + Send + 'static,
        B: FnOnce(&BuildOrchestrator<P>, &BuildContext) -> Option<BuildOutcome>,
        P: 'static,
    {
        let debounce = self
            .config
            .debounce
            .unwrap_or(zfb_watcher::DEFAULT_DEBOUNCE);
        let (mut watcher, mut rx) = Watcher::start_with_extras(
            &self.config.project_root,
            self.config.watch_roots.iter().map(|p| p.as_path()),
            self.config.extra_watch_paths.iter().map(|p| p.as_path()),
            debounce,
        )?;

        info!(
            project_root = %self.config.project_root.display(),
            "build orchestrator running"
        );

        // Boot hook — runs with the watch already registered (so any edit
        // saved during it is buffered by notify and drained by the loop
        // below) but before the loop consumes events. Its outcome, if any,
        // is broadcast through the same `on_outcome` path a watcher tick
        // uses so early clients auto-refresh once the eager render lands.
        if let Some(boot) = boot {
            if let Some(outcome) = boot(&self, &ctx) {
                on_outcome(&outcome);
            }
        }
        // The eager boot browser passes may discover islands or client-script
        // worker dependencies outside the configured source roots. Register
        // each dependency's parent now so edits, deletes, and recreations enter
        // the same watcher channel. The registry exposes the last successful
        // closures, so a transient failed rebuild never drops recovery watches.
        register_dynamic_dependency_watches(&mut watcher, &self.config.policy);

        // Drain loop: wait for the first event, then drain everything
        // currently in the channel so we coalesce concurrent bursts
        // into one tick.
        //
        // Each tick runs seconds of blocking work — esbuild bundling, V8
        // rendering, file IO (issue #903). Running it inline would pin a
        // tokio worker thread for the whole tick, starving the dev
        // server's request handling. `spawn_blocking` moves the tick onto
        // tokio's dedicated blocking pool instead.
        //
        // `spawn_blocking` was chosen over `block_in_place` because
        // (a) `block_in_place` panics on a current-thread runtime, and
        // while `zfb dev` runs on the default multi-thread `#[tokio::main]`
        // runtime, library tests drive this loop from plain
        // `#[tokio::test]` (current-thread flavor); and (b) it leaves the
        // calling worker free to keep polling other tasks. The blocking
        // closure must be `'static`, so it cannot borrow loop state — the
        // orchestrator, build context, and discovery hook are moved into
        // the closure and handed back alongside the tick result every
        // iteration (hence the `P: 'static` bound on this method).
        let mut this = self;
        let mut ctx = ctx;
        let mut discover = discover;
        while let Some(first) = rx.recv().await {
            let mut batch: Vec<Change> = vec![first];
            while let Ok(c) = rx.try_recv() {
                batch.push(c);
            }

            let changes: Vec<(PathBuf, ChangeKind)> =
                batch.iter().map(|c| (c.path.clone(), c.kind)).collect();
            let tick = tokio::task::spawn_blocking(move || {
                let result = this.tick_with_kinds(changes, &ctx, discover.as_ref());
                (result, this, ctx, discover)
            })
            .await;
            let result = match tick {
                Ok((result, this_back, ctx_back, discover_back)) => {
                    this = this_back;
                    ctx = ctx_back;
                    discover = discover_back;
                    result
                }
                // Before #903 a panic inside the tick unwound straight
                // through `run` on the same thread; resume the panic so
                // caller-observable behaviour is unchanged. The non-panic
                // arm (task cancelled) cannot happen in practice — we
                // never abort the blocking task — but losing the
                // orchestrator state means the loop cannot continue, so
                // surface it as a hard error.
                Err(join_err) => match join_err.try_into_panic() {
                    Ok(payload) => std::panic::resume_unwind(payload),
                    Err(join_err) => {
                        return Err(anyhow::anyhow!(
                            "rebuild tick task failed to rejoin: {join_err}"
                        ));
                    }
                },
            };
            // Successful browser-pipeline ticks atomically replace their
            // dependency closures. Add newly-discovered parents before
            // waiting for the next event; the watcher deduplicates
            // existing/covered paths.
            register_dynamic_dependency_watches(&mut watcher, &this.config.policy);
            match result {
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
        // #1288 — a component (`Module`) edit now also re-runs the CSS content
        // scan, because it may author a new Tailwind utility class that must be
        // emitted into `/assets/styles.css` without touching the CSS entry
        // (symptom C of #1284). This flipped from the previous `!rerun_css`.
        assert!(plan.rerun_css);
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

    // ── Opt-in external-change narrowing hook (issue #1038) ─────────────

    /// Build an orchestrator whose config carries the given
    /// `external_invalidation` hook.
    fn make_orch_with_external_hook<P: AssetPipeline>(
        pipeline: P,
        hook: ExternalInvalidationHook,
    ) -> BuildOrchestrator<P> {
        BuildOrchestrator::new(
            OrchestratorConfig::new(
                "/proj",
                vec![PathBuf::from("pages"), PathBuf::from("content")],
            )
            .with_external_invalidation(hook),
            make_graph(),
            pipeline,
        )
    }

    /// When the hook maps an external path to a specific page set, the
    /// plan narrows to exactly that subset instead of the conservative
    /// `PageSelection::All`. The SSR host reload still fires.
    #[test]
    fn external_hook_some_narrows_to_specific_pages() {
        let hook: ExternalInvalidationHook =
            Arc::new(|_path: &Path| Some(vec![pid("/proj/pages/a.tsx")]));
        let orch = make_orch_with_external_hook(CountingPipeline::default(), hook);
        let plan = orch.plan_for_changes(vec![PathBuf::from("/srv/shared/assets/logo.png")]);
        match &plan.pages {
            PageSelection::Specific(s) => {
                assert_eq!(*s, BTreeSet::from([pid("/proj/pages/a.tsx")]));
            }
            other => unreachable!("expected PageSelection::Specific, got {other:?}"),
        }
        // Narrowing must NOT suppress the SSR reload — only the page set
        // is narrowed, the host reload always fires for an external change.
        assert!(plan.ssr_reload_needed);
    }

    /// An empty `Some(vec![])` verdict narrows to "no pages" — the
    /// consumer asserts this external change is not page-renderable. The
    /// SSR reload still fires (it is never gated by the hook).
    #[test]
    fn external_hook_some_empty_narrows_to_no_pages() {
        let hook: ExternalInvalidationHook = Arc::new(|_path: &Path| Some(vec![]));
        let orch = make_orch_with_external_hook(CountingPipeline::default(), hook);
        let plan = orch.plan_for_changes(vec![PathBuf::from("/srv/shared/assets/logo.png")]);
        assert!(plan.pages.is_empty());
        assert!(!plan.pages.is_all());
        assert!(plan.ssr_reload_needed);
    }

    /// When the hook returns `None` (cannot map this path), the plan
    /// falls back to the conservative `PageSelection::All` default.
    #[test]
    fn external_hook_none_falls_back_to_full_rebuild() {
        let hook: ExternalInvalidationHook = Arc::new(|_path: &Path| None);
        let orch = make_orch_with_external_hook(CountingPipeline::default(), hook);
        let plan = orch.plan_for_changes(vec![PathBuf::from("/srv/shared/assets/logo.png")]);
        assert!(
            plan.pages.is_all(),
            "a None hook verdict must keep the conservative All default; got {:?}",
            plan.pages,
        );
        assert!(plan.ssr_reload_needed);
    }

    /// Codex-review finding (issue #1038): an out-of-root file with a
    /// WHITELISTED extension (e.g. a skill `foo.mdx`) classifies as
    /// `Content`, never reaching the `External` arm — yet it is the
    /// issue's primary `foo.mdx → /skills/foo` use case. The hook keys on
    /// out-of-root, not on `PathClass::External`, so it must narrow this
    /// too rather than falling back to the `Content`-arm `All`.
    #[test]
    fn external_hook_narrows_out_of_root_whitelisted_extension() {
        let hook: ExternalInvalidationHook =
            Arc::new(|_path: &Path| Some(vec![pid("/proj/pages/a.tsx")]));
        let orch = make_orch_with_external_hook(CountingPipeline::default(), hook);
        // `.mdx` is whitelisted → would classify as Content, not External.
        let plan = orch.plan_for_changes(vec![PathBuf::from("/srv/skills/foo.mdx")]);
        match &plan.pages {
            PageSelection::Specific(s) => {
                assert_eq!(*s, BTreeSet::from([pid("/proj/pages/a.tsx")]));
            }
            other => unreachable!("expected PageSelection::Specific, got {other:?}"),
        }
        assert!(plan.ssr_reload_needed);
    }

    /// Default (no hook) for an out-of-root whitelisted-extension file
    /// stays conservative: the graph has no edges for it, so the `Content`
    /// arm's empty-dirty fallback yields `PageSelection::All`. Guards that
    /// the new pre-pass does not change the no-hook contract.
    #[test]
    fn out_of_root_whitelisted_extension_without_hook_is_full_rebuild() {
        let orch = make_orch(CountingPipeline::default());
        let plan = orch.plan_for_changes(vec![PathBuf::from("/srv/skills/foo.mdx")]);
        assert!(plan.pages.is_all());
    }

    /// Codex-review regression (issue #1038): narrowing the page set for an
    /// out-of-root CSS file must NOT suppress the CSS rerun. The hook's
    /// verdict narrows only the page selection; the `rerun_css` asset flag
    /// that the `Style` class arm would have set must still fire, or the
    /// consumer ends up with a stale stylesheet in `dist/assets/`.
    #[test]
    fn external_hook_narrowing_css_still_reruns_css() {
        let hook: ExternalInvalidationHook =
            Arc::new(|_path: &Path| Some(vec![pid("/proj/pages/a.tsx")]));
        let orch = make_orch_with_external_hook(CountingPipeline::default(), hook);
        let plan = orch.plan_for_changes(vec![PathBuf::from("/srv/shared/styles/theme.css")]);
        match &plan.pages {
            PageSelection::Specific(s) => {
                assert_eq!(*s, BTreeSet::from([pid("/proj/pages/a.tsx")]));
            }
            other => unreachable!("expected PageSelection::Specific, got {other:?}"),
        }
        assert!(
            plan.rerun_css,
            "narrowing an external CSS path must still rerun the CSS pipeline"
        );
        assert!(plan.ssr_reload_needed);
    }

    /// Codex-review regression (issue #1038): narrowing the page set for an
    /// out-of-root islands module must NOT suppress the islands re-bundle.
    /// The `.tsx` under a `components` segment classifies as a `Module` and
    /// is an islands candidate, so the `rerun_islands` flag the Module arm
    /// would have set must still fire alongside the narrowed page set.
    #[test]
    fn external_hook_narrowing_islands_module_still_reruns_islands() {
        let hook: ExternalInvalidationHook =
            Arc::new(|_path: &Path| Some(vec![pid("/proj/pages/a.tsx")]));
        let orch = make_orch_with_external_hook(CountingPipeline::default(), hook);
        let plan = orch.plan_for_changes(vec![PathBuf::from("/srv/shared/components/Widget.tsx")]);
        match &plan.pages {
            PageSelection::Specific(s) => {
                assert_eq!(*s, BTreeSet::from([pid("/proj/pages/a.tsx")]));
            }
            other => unreachable!("expected PageSelection::Specific, got {other:?}"),
        }
        assert!(
            plan.rerun_islands,
            "narrowing an external islands module must still rerun the islands bundle"
        );
        assert!(plan.ssr_reload_needed);
    }

    /// The hook receives the actual changed path so it can map it. Two
    /// different external paths can narrow to different page subsets.
    #[test]
    fn external_hook_receives_changed_path() {
        let hook: ExternalInvalidationHook = Arc::new(|path: &Path| {
            if path.ends_with("a.skill") {
                Some(vec![pid("/proj/pages/a.tsx")])
            } else {
                Some(vec![pid("/proj/pages/b.tsx")])
            }
        });
        let orch = make_orch_with_external_hook(CountingPipeline::default(), hook);

        let plan_a = orch.plan_for_changes(vec![PathBuf::from("/srv/skills/a.skill")]);
        match &plan_a.pages {
            PageSelection::Specific(s) => {
                assert_eq!(*s, BTreeSet::from([pid("/proj/pages/a.tsx")]));
            }
            other => unreachable!("expected Specific, got {other:?}"),
        }

        let plan_b = orch.plan_for_changes(vec![PathBuf::from("/srv/skills/b.skill")]);
        match &plan_b.pages {
            PageSelection::Specific(s) => {
                assert_eq!(*s, BTreeSet::from([pid("/proj/pages/b.tsx")]));
            }
            other => unreachable!("expected Specific, got {other:?}"),
        }
    }

    // ── Content-narrowing hint production (issue #958, §3) ──────────────

    /// Build a no-op `BuildContext` for driving `tick_with_kinds` against
    /// the recording [`CountingPipeline`].
    fn noop_ctx(dist: &std::path::Path) -> BuildContext {
        BuildContext {
            dist_root: dist.to_path_buf(),
            render_pages: Arc::new(|_, _| Ok(vec![])),
            run_css: None,
            run_islands: None,
            run_client_scripts: None,
            reload_renderer: None,
        }
    }

    /// A tick made exclusively of Modified content files produces the
    /// narrowing hint, carrying the changed paths verbatim, and is
    /// `fan_out_safe` (the eager path may narrow the fan-out).
    #[test]
    fn modified_only_content_tick_produces_fan_out_safe_hint() {
        use zfb_watcher::ChangeKind;
        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        let orch = make_orch(pipeline);
        let dist = tempfile::tempdir().unwrap();

        let changed = PathBuf::from("/proj/content/post.md");
        orch.tick_with_kinds(
            vec![(changed.clone(), ChangeKind::Modified)],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();

        let plans = applies.lock().unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(
            plans[0].content_narrowing,
            Some(crate::plan::ContentNarrowing {
                changed_content: vec![changed],
                fan_out_safe: true,
            }),
            "a Modified-only Content tick must carry the fan-out-safe narrowing hint"
        );
    }

    /// §3 / #1058: a tick mixing a content edit with a module edit must NOT
    /// be `fan_out_safe` — module changes can affect every page's output, so
    /// the eager path must run the full fan-out (G1). It still carries the
    /// edited CONTENT file for the lazy eager basis (the module is excluded),
    /// so a body edit's own route can eager-render even in a mixed tick.
    #[test]
    fn mixed_tick_with_module_trigger_is_not_fan_out_safe() {
        use zfb_watcher::ChangeKind;
        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        let orch = make_orch(pipeline);
        let dist = tempfile::tempdir().unwrap();

        orch.tick_with_kinds(
            vec![
                (PathBuf::from("/proj/content/post.md"), ChangeKind::Modified),
                (
                    PathBuf::from("/proj/components/Header.tsx"),
                    ChangeKind::Modified,
                ),
            ],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();

        let plans = applies.lock().unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(
            plans[0].content_narrowing,
            Some(crate::plan::ContentNarrowing {
                // Only the content file — the module is not content-class.
                changed_content: vec![PathBuf::from("/proj/content/post.md")],
                fan_out_safe: false,
            }),
            "a mixed content+module tick must not be fan-out-safe (G1), but still \
             carries the edited content for the lazy eager basis (#1058)"
        );
    }

    /// #1058: a `Created` for an already-KNOWN content source (the graph
    /// has consumers — here `post.md` feeds `c.tsx`) is a spurious FSEvents
    /// coalescing artifact for an in-place edit, so it is normalized to
    /// `Modified` at the top of the tick. The tick is then a pure in-place
    /// content edit → `fan_out_safe`, exactly as a real `Modified` would be.
    #[test]
    fn spurious_created_on_known_content_normalizes_to_modified() {
        use zfb_watcher::ChangeKind;
        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        let orch = make_orch(pipeline);
        let dist = tempfile::tempdir().unwrap();

        let post = PathBuf::from("/proj/content/post.md"); // known: consumers c.tsx
        orch.tick_with_kinds(
            vec![(post.clone(), ChangeKind::Created)],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();

        let plans = applies.lock().unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(
            plans[0].content_narrowing,
            Some(crate::plan::ContentNarrowing {
                changed_content: vec![post],
                fan_out_safe: true,
            }),
            "a Created for a known content source is normalized to a fan-out-safe edit (#1058)"
        );
    }

    /// #1058: a `Created` for a GENUINELY-NEW content file (`other.md` — no
    /// reverse edge in the graph) is NOT normalized — it stays `Created` for
    /// the discovery regime, so it poisons the strict gate (`!fan_out_safe`).
    /// It is still carried in the lazy eager basis (harmless: it has no
    /// routes yet, so the downstream route-table match renders nothing eager
    /// for it).
    #[test]
    fn unknown_created_file_is_not_fan_out_safe() {
        use zfb_watcher::ChangeKind;
        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        let orch = make_orch(pipeline);
        let dist = tempfile::tempdir().unwrap();

        let new_file = PathBuf::from("/proj/content/other.md"); // unknown / new
        let known = PathBuf::from("/proj/content/post.md"); // known: consumers c.tsx
        orch.tick_with_kinds(
            vec![
                (new_file.clone(), ChangeKind::Created),
                (known.clone(), ChangeKind::Modified),
            ],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();

        let plans = applies.lock().unwrap();
        assert_eq!(plans.len(), 1);
        let narrowing = plans[0]
            .content_narrowing
            .as_ref()
            .expect("tick carries the lazy eager basis");
        assert!(
            !narrowing.fan_out_safe,
            "a genuinely-new Created file poisons the strict gate (G1)"
        );
        assert!(narrowing.changed_content.contains(&new_file));
        assert!(narrowing.changed_content.contains(&known));
    }

    /// #1058: a `Removed` content file is excluded from the lazy eager basis
    /// (it runs the prune regime, not an eager render) and poisons the strict
    /// gate. The Modified sibling still carries the basis.
    #[test]
    fn removed_content_excluded_from_lazy_basis() {
        use zfb_watcher::ChangeKind;
        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        let orch = make_orch(pipeline);
        let dist = tempfile::tempdir().unwrap();

        let removed = PathBuf::from("/proj/content/other.md");
        let edited = PathBuf::from("/proj/content/post.md"); // known: consumers c.tsx
        orch.tick_with_kinds(
            vec![
                (edited.clone(), ChangeKind::Modified),
                (removed.clone(), ChangeKind::Removed),
            ],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();

        let plans = applies.lock().unwrap();
        assert_eq!(plans.len(), 1);
        let narrowing = plans[0]
            .content_narrowing
            .as_ref()
            .expect("tick carries the lazy eager basis");
        assert!(
            !narrowing.fan_out_safe,
            "a Removed change poisons the strict gate (G1)"
        );
        assert!(narrowing.changed_content.contains(&edited));
        assert!(
            !narrowing.changed_content.contains(&removed),
            "a Removed content file is excluded from the lazy eager basis"
        );
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
        use crate::pipeline::dev::DevAssetPipeline;
        use crate::pipeline::{BuildContext, RelDistPath, RenderedPage};
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
            render_pages: Arc::new(move |pages: &[PageId], _narrowing| {
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
            run_client_scripts: None,
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

    // ── Client-script rebuild trigger — per-root coverage (issue #979) ────

    /// Editing `pages/analytics.client.ts` must set `rerun_client_scripts`
    /// even though the file classifies as `PathClass::Page` (not Module).
    ///
    /// This is the BLOCKING acceptance test for the pages/ root: without the
    /// post-match `is_client_script_candidate` check in `plan_for_changes`,
    /// a `pages/*.client.ts` edit would never trigger the client-scripts
    /// rebuild pass because the `mark_islands` gate only fires for Module
    /// changes inside `islands_roots`.
    #[test]
    fn client_script_edit_under_pages_sets_rerun_client_scripts() {
        let orch = make_orch(CountingPipeline::default());
        let plan = orch.plan_for_changes(vec![PathBuf::from("/proj/pages/analytics.client.ts")]);
        assert!(
            plan.rerun_client_scripts,
            "*.client.ts under pages/ must set rerun_client_scripts"
        );
        // Also: the page edit path still fires.
        assert!(
            !plan.rerun_islands,
            "pages/ file must NOT trigger islands rerun"
        );
    }

    /// Editing `components/search-widget.client.ts` must set
    /// `rerun_client_scripts`. (`components/` is also an islands root, so
    /// `rerun_islands` fires here too — but the client-scripts rebuild is
    /// independent.)
    #[test]
    fn client_script_edit_under_components_sets_rerun_client_scripts() {
        let orch = make_orch(CountingPipeline::default());
        let plan = orch.plan_for_changes(vec![PathBuf::from(
            "/proj/components/search-widget.client.ts",
        )]);
        assert!(
            plan.rerun_client_scripts,
            "*.client.ts under components/ must set rerun_client_scripts"
        );
        assert!(
            plan.rerun_islands,
            "components/ is an islands root — rerun_islands must also fire"
        );
    }

    /// Editing `src/my-lib.client.ts` must set `rerun_client_scripts`.
    /// (`src/` is also an islands root, same dual-trigger expectation as
    /// `components/`.)
    #[test]
    fn client_script_edit_under_src_sets_rerun_client_scripts() {
        let orch = make_orch(CountingPipeline::default());
        let plan = orch.plan_for_changes(vec![PathBuf::from("/proj/src/my-lib.client.ts")]);
        assert!(
            plan.rerun_client_scripts,
            "*.client.ts under src/ must set rerun_client_scripts"
        );
        assert!(
            plan.rerun_islands,
            "src/ is an islands root — rerun_islands must also fire"
        );
    }

    /// A regular (non-client) `.tsx` edit under `pages/` must NOT set
    /// `rerun_client_scripts`.
    #[test]
    fn regular_tsx_edit_under_pages_does_not_set_rerun_client_scripts() {
        let orch = make_orch(CountingPipeline::default());
        let plan = orch.plan_for_changes(vec![PathBuf::from("/proj/pages/index.tsx")]);
        assert!(
            !plan.rerun_client_scripts,
            "regular .tsx under pages/ must NOT set rerun_client_scripts"
        );
    }

    #[test]
    fn original_raw_target_edit_reruns_islands_and_client_scripts() {
        let invalidation = crate::policy::RawImportInvalidation::default();
        let target = PathBuf::from("/proj/data/noise.frag");
        invalidation.replace_islands([target.clone()]);
        invalidation.replace_client_scripts([target.clone()]);
        let config = OrchestratorConfig::new("/proj", vec![PathBuf::from("data")]).with_policy(
            crate::policy::GranularityPolicy::default()
                .with_raw_import_invalidation(invalidation.clone()),
        );
        let orch = BuildOrchestrator::new(config, make_graph(), CountingPipeline::default());

        // The importer is unchanged; only the terminal target is the watcher
        // event. Both consumer pipelines must rerun.
        let plan = orch.plan_for_changes([target.clone()]);
        assert!(plan.rerun_islands, "raw target must rerun islands");
        assert!(
            plan.rerun_client_scripts,
            "raw target must rerun client scripts"
        );

        // Successful graph replacement is also the stale-dependency hygiene
        // contract: an import removal must stop old targets triggering work.
        invalidation.replace_islands(Vec::new());
        invalidation.replace_client_scripts(Vec::new());
        let plan = orch.plan_for_changes([target]);
        assert!(!plan.rerun_islands);
        assert!(!plan.rerun_client_scripts);
    }

    #[test]
    fn client_script_worker_dependency_replacement_stops_stale_pipeline_planning() {
        let invalidation = crate::policy::RawImportInvalidation::default();
        let old_helper = PathBuf::from("/proj/lib/old-worker-helper.ts");
        let next_helper = PathBuf::from("/proj/lib/next-worker-helper.ts");
        invalidation.replace_client_script_workers([old_helper.clone()]);
        let config = OrchestratorConfig::new("/proj", vec![PathBuf::from("pages")]).with_policy(
            crate::policy::GranularityPolicy::default()
                .with_raw_import_invalidation(invalidation.clone()),
        );
        let orch = BuildOrchestrator::new(config, make_graph(), CountingPipeline::default());

        let first_tick = orch.plan_for_changes([old_helper.clone()]);
        assert!(
            first_tick.rerun_client_scripts,
            "worker dependency edits must re-emit the owning client script"
        );
        assert!(!first_tick.rerun_islands);

        invalidation.replace_client_script_workers([next_helper.clone()]);
        let stale_tick = orch.plan_for_changes([old_helper]);
        assert!(
            !stale_tick.rerun_client_scripts,
            "a removed worker edge must clear stale invalidation ownership"
        );
        assert!(orch.plan_for_changes([next_helper]).rerun_client_scripts);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn client_worker_dependency_outside_boot_roots_is_watched() {
        use std::time::Duration;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("pages")).unwrap();
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let helper = root.join("lib/client-worker-helper.ts");
        std::fs::write(&helper, "export const marker = 'one';\n").unwrap();

        let invalidation = crate::policy::RawImportInvalidation::default();
        invalidation.replace_client_script_workers([helper.clone()]);
        let policy =
            crate::policy::GranularityPolicy::default().with_raw_import_invalidation(invalidation);
        assert!(policy.dynamic_dependency_paths().contains(&helper));
        assert!(policy.is_client_script_worker_target(&helper));
        assert!(!policy.is_islands_dependency(&helper));

        let (mut watcher, mut rx) = Watcher::start_with_debounce(
            &root,
            std::iter::once("pages"),
            Duration::from_millis(50),
        )
        .unwrap();
        register_dynamic_dependency_watches(&mut watcher, &policy);

        // `lib/` is deliberately absent from the recursive boot roots. The
        // client worker registry must add its parent as a dynamic watch.
        tokio::time::sleep(Duration::from_millis(100)).await;
        while rx.try_recv().is_ok() {}
        std::fs::write(&helper, "export const marker = 'two';\n").unwrap();
        let observed = tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(change) = rx.recv().await {
                if change.path == helper {
                    return Some(change.kind);
                }
            }
            None
        })
        .await
        .expect("outside-root client worker edit must reach the watcher");
        watcher.shutdown().await;
        assert!(
            matches!(observed, Some(ChangeKind::Created | ChangeKind::Modified)),
            "outside-root client worker edit must produce a write event, got {observed:?}"
        );
    }

    #[test]
    fn live_worker_closure_outside_islands_roots_survives_edit_delete_and_next_generation() {
        use zfb_watcher::ChangeKind;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("pages/workers")).unwrap();
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let worker = root.join("pages/workers/search.worker.ts");
        let helper = root.join("lib/tokenize.ts");
        let raw = root.join("lib/dictionary.txt");
        let css = root.join("lib/worker.css");
        for path in [&worker, &helper, &raw, &css] {
            std::fs::write(path, "generation one\n").unwrap();
        }

        let invalidation = crate::policy::RawImportInvalidation::default();
        invalidation.replace_islands([worker.clone(), helper.clone(), raw.clone(), css.clone()]);
        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        let config = OrchestratorConfig::new(&root, vec![PathBuf::from("pages")]).with_policy(
            crate::policy::GranularityPolicy::default()
                .with_raw_import_invalidation(invalidation.clone()),
        );
        let orch = BuildOrchestrator::new(config, make_graph(), pipeline);
        let dist = tempfile::tempdir().unwrap();

        // Tick 1: a helper under lib/ is neither a default islands candidate
        // nor a watcher boot root, but the live worker closure marks it.
        orch.tick_with_kinds(
            vec![(helper.clone(), ChangeKind::Modified)],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();
        assert!(applies.lock().unwrap().last().unwrap().rerun_islands);

        // Tick 2: deletion takes the removed-path planning branch. The
        // lexical dependency alias remains registered even though
        // canonicalisation now fails, so islands still reruns.
        std::fs::remove_file(&helper).unwrap();
        orch.tick_with_kinds(
            vec![(helper.clone(), ChangeKind::Removed)],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();
        assert!(applies.lock().unwrap().last().unwrap().rerun_islands);

        // The next successful preprocessing generation atomically replaces
        // the graph: recreated/new dependencies trigger, stale ones stop.
        std::fs::write(&helper, "generation two\n").unwrap();
        let next = root.join("lib/stemmer.ts");
        std::fs::write(&next, "generation two\n").unwrap();
        invalidation.replace_islands([worker, helper.clone(), css, next.clone()]);
        assert!(orch.plan_for_changes([helper]).rerun_islands);
        assert!(orch.plan_for_changes([next]).rerun_islands);
        assert!(
            !orch.plan_for_changes([raw]).rerun_islands,
            "paths absent from the replacement graph must not stay stale"
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
            render_pages: Arc::new(|_, _| Ok(vec![])),
            run_css: None,
            run_islands: None,
            run_client_scripts: None,
            reload_renderer: None,
        };
        assert!(
            orch.initial_build(&ctx).expect("must not error").is_none(),
            "empty graph must yield Ok(None)",
        );
    }
}
