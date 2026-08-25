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
use zfb_watcher::{Change, ChangeKind, WatchBackend, WatchOptions, Watcher};

use crate::pipeline::{AssetPipeline, BuildContext, BuildOutcome};
use crate::plan::{PageSelection, RebuildPlan};
use crate::policy::{classify_change_with_content_roots, GranularityPolicy, PathClass};

/// Register non-recursive parent watches for browser dependency closures
/// discovered outside the configured recursive source roots, AND reconcile
/// the CSS sibling-mirror-root recursive-directory watch set (issue #1802,
/// epic #1799) against the policy's current CSS source plan.
///
/// The parents newly watched by a call are the out-of-recursive-root
/// dependency directories that just entered the watch set — in a pnpm
/// workspace these are the sibling-package source directories backing a
/// client-script `?raw` / module-worker target the latest bundle discovered
/// (issue #1678). They become watched restart-free, on the very tick that
/// discovers them, so the edit that introduces a new sibling import is enough
/// to make subsequent sibling edits invalidate. That registration is
/// otherwise silent and asynchronous, so — behind the shared `ZFB_DEV_TIMING`
/// flag — each newly watched directory is surfaced as an observable
/// `watch-extra registered:` line. It is the deterministic signal an e2e keys
/// its "the sibling directory is now watched" wait on before editing the
/// sibling (see the deflaking recipe's Step-5 escalation).
///
/// The CSS mirror-root reconciliation (issue #1802) reuses the exact same
/// `watch-extra registered:` signal: `Watcher::sync_recursive_dir_watches`
/// (issue #1801) is called every tick with the policy's CURRENT full root
/// set (replace semantics — a root the latest CSS recompute no longer
/// claims is retired) and `css_mirror_skip_dir_names`, returning only the
/// roots genuinely newly watched this call, so an unchanged plan is a
/// cheap no-op that never re-emits the signal.
///
/// Returns the union of both newly-watched sets — every path this call
/// caused to become watched for the first time — for callers (and tests)
/// that want the signal without depending on the `ZFB_DEV_TIMING` env gate.
fn register_dynamic_dependency_watches(
    watcher: &mut Watcher,
    policy: &GranularityPolicy,
    css_mirror_skip_dir_names: &[String],
) -> Vec<PathBuf> {
    let mut newly_watched = watcher.watch_additional_files(policy.dynamic_dependency_paths());
    let newly_watched_dirs = watcher
        .sync_recursive_dir_watches(policy.css_mirror_root_paths(), css_mirror_skip_dir_names);
    if dev_timing_enabled() {
        for dir in newly_watched.iter().chain(newly_watched_dirs.iter()) {
            eprintln!("[zfb-timing] watch-extra registered: {}", dir.display());
        }
    }
    newly_watched.extend(newly_watched_dirs);
    newly_watched
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

/// Pure parse for `ZFB_DEV_TEST_ORCH_PANIC_ON_TICK` (issue #2100, Dev
/// Supervision epic #2099 Sub #2100 — deterministic fault-injection knobs).
/// Mirrors the tolerant boolean idiom `dev_timing_enabled` above uses: unset,
/// empty, `"0"`, or `"false"` (case-insensitive) is NOT armed; anything else
/// is armed. No `cfg(test)` gate — read in production code, inert when unset.
///
/// When armed, [`BuildOrchestrator::run_with_boot`]'s drain loop panics
/// INSIDE the `spawn_blocking` tick closure on the very next dispatched
/// tick, exercising the `std::panic::resume_unwind` re-raise path a few
/// lines below in this file — the same shape a genuine tick panic takes.
///
/// Marker lines this knob prints (exact wording; downstream e2e tests grep
/// these verbatim — keep stable):
///   armed: `"[zfb-timing] fault armed: ZFB_DEV_TEST_ORCH_PANIC_ON_TICK"`
///   fired: `"[zfb-timing] fault fired: ZFB_DEV_TEST_ORCH_PANIC_ON_TICK"`
fn orch_panic_on_tick_armed(raw: Option<&str>) -> bool {
    match raw {
        None => false,
        Some(v) => {
            let v = v.trim();
            !(v.is_empty() || v.eq_ignore_ascii_case("0") || v.eq_ignore_ascii_case("false"))
        }
    }
}

/// Pure parse for `ZFB_DEV_TEST_ORCH_STOP_MS` (issue #2100, Dev Supervision
/// epic #2099 Sub #2100). Returns `Some(ms)` when the raw value parses as a
/// non-negative integer number of milliseconds (`"0"` is a valid, armed
/// boundary value — see below); `None` when unset, empty, or unparsable
/// (inert, matching the tolerant-parse idiom of `ZFB_DEV_TEST_SLOW_*`
/// elsewhere in this codebase).
///
/// When `Some(ms)`, [`BuildOrchestrator::run_with_boot`]'s drain loop
/// returns `Ok(())` (the same silent-channel-close shape an ordinary
/// watcher-dropped-elsewhere exit takes) `ms` milliseconds after the boot
/// hook completes — NOT `ms` after `run_with_boot` was entered, and never
/// before the watcher's live handshake, so a slow boot can never make this
/// fire before the watcher is actually observing. `ms = 0` fires on the
/// very first drain-loop poll after the boot hook returns.
///
/// **Timing caveat (only checked between ticks, not preemptive):** the
/// deadline is raced against the receiver only while the loop is idle,
/// waiting for the *next* [`Change`] — see [`recv_with_stop_deadline`]. If a
/// tick is already dispatched to `spawn_blocking` when the deadline elapses,
/// the fault fires only once that tick's `spawn_blocking` future resolves,
/// not mid-flight — tokio blocking tasks cannot be safely preempted without
/// losing the orchestrator/session state the closure owns for that
/// iteration, and the elapsed-but-not-yet-observed deadline is itself
/// consistent with the "silent channel close" shape this knob emulates (an
/// ordinary channel close is likewise only ever noticed between ticks, never
/// mid-tick). A test relying on `ms` as a tight upper bound must keep the
/// fixture idle (no watched-file churn) between the boot hook completing and
/// the deadline elapsing.
///
/// Marker lines this knob prints (exact wording; downstream e2e tests grep
/// these verbatim — keep stable):
///   armed: `"[zfb-timing] fault armed: ZFB_DEV_TEST_ORCH_STOP_MS <ms>ms"`
///   fired: `"[zfb-timing] fault fired: ZFB_DEV_TEST_ORCH_STOP_MS"`
fn orch_stop_ms_decision(raw: Option<&str>) -> Option<u64> {
    match raw {
        None => None,
        Some(v) => {
            let v = v.trim();
            if v.is_empty() {
                None
            } else {
                v.parse::<u64>().ok()
            }
        }
    }
}

/// Awaits the next [`Change`] off `rx`, but — when `stop_deadline` is set —
/// races that against the deadline and returns `None` (the same shape as a
/// closed channel) the instant the deadline elapses, printing the
/// `ZFB_DEV_TEST_ORCH_STOP_MS` fired marker first. `biased` ensures an
/// already-elapsed deadline (the `ms = 0` boundary) always wins over a
/// simultaneously-ready `Change` rather than being starved by it.
async fn recv_with_stop_deadline(
    rx: &mut tokio::sync::mpsc::Receiver<Change>,
    stop_deadline: Option<tokio::time::Instant>,
) -> Option<Change> {
    match stop_deadline {
        None => rx.recv().await,
        Some(deadline) => {
            tokio::select! {
                biased;
                _ = tokio::time::sleep_until(deadline) => {
                    eprintln!("[zfb-timing] fault fired: ZFB_DEV_TEST_ORCH_STOP_MS");
                    None
                }
                change = rx.recv() => change,
            }
        }
    }
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

/// The boxed future a [`PreTickRefreshHook`] invocation returns. Owned
/// (`'static`): the hook takes its path batch by value so the loop can
/// await the future without borrowing loop state across the await.
pub type PreTickRefreshFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'static>>;

/// Optional async pre-tick plugin-refresh hook (issue #2169, epic #2166
/// "Plugin Watch Hook").
///
/// Consulted from [`BuildOrchestrator::run_with_boot`]'s drain loop once
/// per debounced batch, BEFORE the tick is dispatched to the blocking
/// pool — and only when at least one of the batch's paths is a registered
/// plugin virtual-module watch file
/// ([`crate::policy::RawImportInvalidation::is_plugin_watch_target`], the
/// `watchFiles` registry issue #2168 populates once at boot from #2167's
/// `addVirtualModule(..., { watchFiles })` option; see
/// `pre_tick_refresh_applies` for the exact gate). The hook receives
/// the batch's FULL changed-path set (its implementation resolves
/// ownership itself — `PluginRefreshState::refresh` in
/// `crate::plugin_refresh` ignores paths no loader watches) and is
/// AWAITED to completion before `tick_with_kinds` runs, so the tick's
/// islands / client-scripts / CSS closures — which snapshot the shared
/// `PluginVirtualModuleStore` at use time — already see the freshly
/// re-invoked loader output. The refreshed source ships in the SAME tick
/// as the watched file's change, never one tick late.
///
/// Mirrors the [`ExternalInvalidationHook`] pattern in shape (an opaque
/// `Arc` closure installed on [`OrchestratorConfig`] by the `zfb` command
/// layer, `None` = no behavior change), but differs in kind: that hook is
/// a pure path→pages query used to NARROW the plan; this one MUTATES
/// session state and never influences page selection at all. Page
/// selection for a watched path is decided in
/// [`BuildOrchestrator::plan_for_changes`] (and, for a removal, in
/// [`BuildOrchestrator::tick_with_kinds`]'s removed-path fold), which
/// marks `PageSelection::All` for every plugin watch target regardless of
/// how the path classifies — an in-root watch file under an unrecognized
/// directory classifies `Unclassified` with no dependency edge, so
/// `dirty_pages` alone would leave a prerendered consumer page stale
/// forever (issue #2181). The All fallback is the accepted first cut
/// (issue #2169); narrowing `dirty_pages` here would trade a perf bug for
/// an aggregate-page under-render (see issue #1583).
///
/// The hook is awaited directly on the orchestrator's async task (the
/// underlying plugin-host call is async — no `spawn_blocking` wrapper).
/// An `Err` is logged and the tick proceeds anyway: a failed refresh
/// leaves the store serving its last-good memo, which is exactly what the
/// tick should render (the atomicity contract of
/// `PluginRefreshState::refresh`, which reports per-loader failures via
/// its outcome rather than `Err`).
pub type PreTickRefreshHook =
    std::sync::Arc<dyn Fn(Vec<PathBuf>) -> PreTickRefreshFuture + Send + Sync + 'static>;

/// Pure gate for [`PreTickRefreshHook`] invocation: does this batch touch
/// the plugin virtual-module watch set at all? Split out of
/// [`BuildOrchestrator::run_with_boot`]'s drain loop (mirroring
/// `orch_panic_on_tick_armed` / `orch_stop_ms_decision`) so the decision
/// is unit-testable without spinning up a watcher.
///
/// Kind-agnostic on purpose: a `Removed` watched path must still trigger
/// the refresh — the owning loader's re-invocation fails (its read
/// throws), the store keeps the last-good memo, and the failure is queued
/// for retry on the next refresh call (the delete→recreate recovery
/// `plugin_refresh.rs`'s tests pin). The watch set itself is STATIC —
/// populated once at boot, never purged by the #1581 `Removed` fold
/// (which owns `known_content`, a different registry) — so gating on
/// membership is stable across removal ticks; see
/// `plugin_watch_set_is_static_across_removed_ticks`.
fn pre_tick_refresh_applies(policy: &GranularityPolicy, changes: &[(PathBuf, ChangeKind)]) -> bool {
    changes
        .iter()
        .any(|(path, _)| policy.is_plugin_watch_target(path))
}

/// Await the configured [`PreTickRefreshHook`] when `changes` touches the
/// plugin watch set; a no-op otherwise (issue #2169).
///
/// Extracted from [`BuildOrchestrator::run_with_boot`]'s drain loop so the
/// hook contract — awaited to completion, full batch handed over, `Err`
/// logged and swallowed — is unit-testable without a live watcher. The
/// loop-level integration test
/// (`pre_tick_hook_gates_the_tick_dispatch_in_the_live_loop`) separately
/// pins that the drain loop actually calls this BEFORE the tick dispatch —
/// a helper nobody awaits at the right seam would read as coverage while
/// guarding nothing (the #1058/#1581 dead-guard lesson).
async fn maybe_pre_tick_refresh(config: &OrchestratorConfig, changes: &[(PathBuf, ChangeKind)]) {
    let Some(hook) = config.pre_tick_refresh.as_ref() else {
        return;
    };
    if !pre_tick_refresh_applies(&config.policy, changes) {
        return;
    }
    let paths: Vec<PathBuf> = changes.iter().map(|(path, _)| path.clone()).collect();
    let refresh_result = hook(paths).await;
    if dev_timing_enabled() {
        eprintln!(
            "[zfb-timing] plugin-refresh: pre-tick refresh completed ok={}",
            refresh_result.is_ok()
        );
    }
    if let Err(err) = refresh_result {
        warn!(
            error = %err,
            "pre-tick plugin refresh failed; ticking on last-good sources"
        );
    }
}

/// Opt-in watch-intake suppression predicate (issue #2345).
///
/// Consulted from [`BuildOrchestrator::run_with_boot`]'s drain loop for
/// every path in every debounced batch, BEFORE any tick processing —
/// before `maybe_pre_tick_refresh`, before the tick dispatch, and
/// therefore before `tick_with_kinds`'s removed-path fold and the
/// discovery hook. A path the predicate returns `true` for is dropped
/// from the batch; a batch left empty skips its tick entirely.
///
/// Owned by the `zfb` command layer, like
/// [`OrchestratorConfig::css_mirror_skip_dir_names`]: this crate stores
/// the closure opaquely and knows nothing about what it matches. The
/// motivating consumer is the CSS engine's own synthesised Tailwind entry
/// temp file (`zfb_css::is_tailwind_entry_tmp`), which lands in a watched
/// directory on every CSS pass — without suppression each pass's watch
/// event triggers the next CSS pass under a fresh random name, so the
/// dev loop never goes idle (issue #2343).
///
/// Kind-agnostic on purpose — suppression must cover `Created`,
/// `Modified`, AND `Removed`: a close-after-write can be delivered as
/// `Modified`, `tick_with_kinds`'s existence reconciliation can change a
/// kind after the fact, and a `Removed` path bypasses `plan_for_changes`
/// yet still triggers CSS via the removed-path fold. The invariant is
/// that a suppressed path's event never reaches ANY tick processing.
pub type IntakeSuppressionPredicate = std::sync::Arc<dyn Fn(&Path) -> bool + Send + Sync + 'static>;

/// Apply the configured [`IntakeSuppressionPredicate`] to a debounced
/// batch; identity when no predicate is configured. Split out of
/// [`BuildOrchestrator::run_with_boot`]'s drain loop (mirroring
/// [`pre_tick_refresh_applies`]) so the batch semantics are unit-testable
/// without spinning up a watcher; the loop-level test
/// (`suppressed_only_batches_produce_no_tick_in_the_live_loop`)
/// separately pins that the drain loop actually filters at this seam —
/// a helper nobody calls would read as coverage while guarding nothing
/// (the #1058/#1581 dead-guard lesson).
fn retain_unsuppressed_changes(
    config: &OrchestratorConfig,
    changes: Vec<(PathBuf, ChangeKind)>,
) -> Vec<(PathBuf, ChangeKind)> {
    let Some(suppress) = config.intake_suppression.as_ref() else {
        return changes;
    };
    changes
        .into_iter()
        .filter(|(path, _)| !suppress(path))
        .collect()
}

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

    /// Relative source roots to watch. Typical (the zfb command layer's
    /// `DEFAULT_WATCH_ROOTS`, `crates/zfb/src/commands/dev.rs`): `["pages",
    /// "content", "components", "layouts", "styles", "data", "src",
    /// "zfb.config.json", "zfb.config.ts"]` plus any configured collection
    /// path. `public` is deliberately excluded — it is served directly
    /// from disk and does not feed the dep-graph or the renderer (issue
    /// #1165).
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

    /// Directory-name skip list applied to the CSS sibling-mirror-root
    /// recursive-directory watch (issue #1802, epic #1799) —
    /// [`zfb_watcher::Watcher::sync_recursive_dir_watches`]'s
    /// `skip_dir_names` parameter. Owned by the `zfb` command layer
    /// (`CSS_SIBLING_MIRROR_SKIP_DIRS`); this crate stores it opaquely and
    /// knows nothing about CSS. Empty by default — an empty list means no
    /// suppression, i.e. every file under a registered mirror root is
    /// delivered.
    pub css_mirror_skip_dir_names: Vec<String>,

    /// Optional override for the watcher debounce window. `None` =
    /// `zfb_watcher::DEFAULT_DEBOUNCE` (50ms).
    pub debounce: Option<Duration>,

    /// Which [`WatchBackend`] the dev-loop watcher drives (issue #2174).
    ///
    /// Sourced from `Config::watch_poll_fallback` /
    /// `Config::watch_poll_interval_ms` — the caller (the `zfb dev`
    /// command layer) is responsible for translating those two config
    /// fields into a [`WatchBackend`] value before populating this.
    /// Default: [`WatchBackend::Native`], matching pre-#2174 behavior.
    pub backend: WatchBackend,

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

    /// Optional async pre-tick plugin-refresh hook (issue #2169).
    ///
    /// `None` (the default) keeps the pre-#2169 behavior: a plugin
    /// virtual-module watch-file change ticks on whatever source text the
    /// shared store already holds. `Some(hook)` makes the drain loop
    /// await the hook to completion before dispatching any tick whose
    /// batch touches the plugin watch set — see [`PreTickRefreshHook`]
    /// for the full contract (state-mutating, never narrows the plan,
    /// errors are logged and the tick proceeds).
    pub pre_tick_refresh: Option<PreTickRefreshHook>,

    /// Opt-in watch-intake suppression predicate (issue #2345).
    ///
    /// `None` (the default) delivers every debounced batch to the tick
    /// unchanged. `Some(predicate)` drops matching paths from every
    /// batch before ANY tick processing, regardless of change kind —
    /// see [`IntakeSuppressionPredicate`] for the full contract and why
    /// it must stay kind-agnostic. Owned by the `zfb` command layer;
    /// stored opaquely here.
    pub intake_suppression: Option<IntakeSuppressionPredicate>,
}

impl std::fmt::Debug for OrchestratorConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrchestratorConfig")
            .field("project_root", &self.project_root)
            .field("watch_roots", &self.watch_roots)
            .field("extra_watch_paths", &self.extra_watch_paths)
            .field("policy", &self.policy)
            .field("css_mirror_skip_dir_names", &self.css_mirror_skip_dir_names)
            .field("debounce", &self.debounce)
            .field("backend", &self.backend)
            .field(
                "external_invalidation",
                &self.external_invalidation.as_ref().map(|_| "<hook>"),
            )
            .field(
                "pre_tick_refresh",
                &self.pre_tick_refresh.as_ref().map(|_| "<hook>"),
            )
            .field(
                "intake_suppression",
                &self.intake_suppression.as_ref().map(|_| "<predicate>"),
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
            css_mirror_skip_dir_names: Vec::new(),
            debounce: None,
            backend: WatchBackend::default(),
            external_invalidation: None,
            pre_tick_refresh: None,
            intake_suppression: None,
        }
    }

    /// Override the policy (chainable).
    pub fn with_policy(mut self, policy: GranularityPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Set the CSS sibling-mirror-root skip-dir names (chainable, issue
    /// #1802). See [`Self::css_mirror_skip_dir_names`].
    pub fn with_css_mirror_skip_dir_names(mut self, names: Vec<String>) -> Self {
        self.css_mirror_skip_dir_names = names;
        self
    }

    /// Override the debounce window (chainable).
    pub fn with_debounce(mut self, debounce: Duration) -> Self {
        self.debounce = Some(debounce);
        self
    }

    /// Override the watch backend (chainable, issue #2174). See
    /// [`Self::backend`].
    pub fn with_backend(mut self, backend: WatchBackend) -> Self {
        self.backend = backend;
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

    /// Set the async pre-tick plugin-refresh hook (chainable, issue
    /// #2169). See [`PreTickRefreshHook`]. Without this, plugin
    /// virtual-module watch-file changes tick on the store's existing
    /// (possibly stale) source text.
    pub fn with_pre_tick_refresh(mut self, hook: PreTickRefreshHook) -> Self {
        self.pre_tick_refresh = Some(hook);
        self
    }

    /// Set the watch-intake suppression predicate (chainable, issue
    /// #2345). See [`IntakeSuppressionPredicate`]. Without this, every
    /// debounced batch reaches the tick unchanged.
    pub fn with_intake_suppression(mut self, predicate: IntakeSuppressionPredicate) -> Self {
        self.intake_suppression = Some(predicate);
        self
    }
}

/// Pure derivation of the [`WatchOptions`] used to start the dev-loop
/// watcher (issue #2174): the configured debounce (or
/// [`zfb_watcher::DEFAULT_DEBOUNCE`] when absent) plus the configured
/// backend. Split out of [`BuildOrchestrator::run_with_boot`] so backend
/// selection is unit-testable without booting a real watcher.
fn watch_options_for(config: &OrchestratorConfig) -> WatchOptions {
    let debounce = config.debounce.unwrap_or(zfb_watcher::DEFAULT_DEBOUNCE);
    WatchOptions::default()
        .with_debounce(debounce)
        .with_backend(config.backend)
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

    /// Class-agnostic containment test: does `path` live inside a
    /// registered CSS sibling-mirror root (issue #1802's `css_mirror_roots`
    /// registry), once the two containment rules below are applied? This is
    /// "the `@source` scan would have read this file, wherever it sits in
    /// the tree" — it carries no opinion on `PathClass` at all. Split out of
    /// `content_under_css_mirror_root` in #2077 so the REMOVED-path fold
    /// (`tick_with_kinds`) can consult it for every path class, not just the
    /// three the live-edit arms happen to gate on.
    ///
    /// Two containment rules ride on top of the bare subtree test, both
    /// keeping this gate equivalent to "the `@source` scan would have read
    /// this file":
    ///
    /// - **Non-degeneracy.** A root that CONTAINS `project_root` would match
    ///   every path in the project and silently convert this into the
    ///   rejected option (a). `SiblingMirrorPlan`'s `resolve_mirror_root`
    ///   never publishes such a root (pinned by
    ///   `bundler::tests::resolve_mirror_root_never_returns_an_ancestor_of_project_root`),
    ///   so this is defense in depth against a future claim-policy change —
    ///   not a live condition.
    /// - **Infra skip-dirs.** Tailwind's `@source` globs exclude the
    ///   `CSS_SIBLING_MIRROR_SKIP_DIRS` infra dirs (`dist/`,
    ///   `node_modules/`, …) at any depth under a mirror root, so an event
    ///   from one cannot change the emitted CSS. The list is the command
    ///   layer's, threaded down through
    ///   [`OrchestratorConfig::css_mirror_skip_dir_names`] — the SAME value
    ///   the recursive-directory watch already suppresses on — rather than
    ///   re-spelled here, so the two can never drift into two different
    ///   definitions of "inside a claimed mirror region".
    fn path_under_css_mirror_root(&self, path: &Path) -> bool {
        let Some((root, relative)) = self.config.policy.css_mirror_root_match(path) else {
            return false;
        };
        let root_swallows_the_project =
            crate::policy::RawImportInvalidation::path_aliases(&self.config.project_root)
                .iter()
                .any(|project_alias| project_alias.starts_with(&root));
        if root_swallows_the_project {
            return false;
        }
        !relative.components().any(|component| match component {
            std::path::Component::Normal(name) => self
                .config
                .css_mirror_skip_dir_names
                .iter()
                .any(|skip| name == std::ffi::OsStr::new(skip)),
            _ => false,
        })
    }

    /// Issue #1819 (epic #1995) — option (b): a `PathClass::Content` change
    /// (`.md`/`.mdx`) must rerun the Tailwind content scan ONLY when it lies
    /// under a registered CSS sibling-mirror root. Thin class-gated wrapper
    /// around [`Self::path_under_css_mirror_root`] for the LIVE-edit arms
    /// (`plan_for_changes`'s three call sites) — behaviourally identical to
    /// the pre-#2077 combined function for every class it ever accepted.
    ///
    /// `discover_css_source_files` (`crates/zfb/src/commands/build.rs`) scans
    /// `.md`/`.mdx` inside a claimed mirror root, and Tailwind's `@source`
    /// globs cover the whole subtree — so a utility class authored only in a
    /// sibling markdown file IS part of the CSS input, but #1288's `mark_css`
    /// rule is gated on `PathClass::Module` alone and never fired for it. The
    /// symptom is dev-loop only: prod builds rescan unconditionally.
    ///
    /// Deliberately NOT unconditional on `Content`: that would make every
    /// ordinary markdown edit rerun the Tailwind scan, which is a real
    /// dev-loop cost on content-heavy sites. The mirror-root gate is what
    /// keeps in-root content edits as cheap as they are today.
    ///
    /// `Data` and `External` ride along with `Content` because Tailwind's
    /// `@source` scan covers the WHOLE mirror-root subtree, not just the
    /// extensions this classifier happens to whitelist: an out-of-root
    /// `.json`/`.yaml` classifies `Data`, an out-of-root
    /// `.html`/`.vue`/`.svelte` classifies `External`, and a class token in
    /// either is real CSS input. Those three are the complete set that can
    /// reach a CSS-inert arm from outside the project root — an out-of-root
    /// path never classifies `Page`, while `Module` and `Style` already
    /// `mark_css` unconditionally.
    ///
    /// This is a CSS-rerun signal and nothing else — it never touches page
    /// selection (see the `PageSelection::All` note in the
    /// `Page | Module | Content | Data` arm).
    fn content_under_css_mirror_root(&self, class: PathClass, path: &Path) -> bool {
        if !matches!(
            class,
            PathClass::Content | PathClass::Data | PathClass::External
        ) {
            return false;
        }
        self.path_under_css_mirror_root(path)
    }

    /// Apply the plugin virtual-module invalidation for `path` when it is a
    /// registered `watchFiles` entry (issues #2169 / #2181); a no-op
    /// otherwise.
    ///
    /// A plugin watch file is a dependency of whatever imports the owning
    /// loader's virtual module: the SSR bundle, islands, client scripts,
    /// the CSS content scan, and any PRERENDERED page whose committed HTML
    /// embeds the virtual module's value. The pre-tick refresh
    /// ([`PreTickRefreshHook`]) updates the STORE; these flags are what
    /// rebuild the consumers that read it — without them an
    /// `External`-classified watch file never reruns islands/
    /// client-scripts/CSS, and an `Asset`-classified one produces a no-op
    /// plan, leaving the freshly refreshed source unshipped (codex review,
    /// #2169).
    ///
    /// Everything here is deliberately blunt, including the
    /// `PageSelection::All`. WHICH pages or bundles import a given virtual
    /// specifier is not knowable at this layer: the loader reads its watch
    /// files through `node:fs`, a read no dependency edge can record. A
    /// watch file inside the project root under an unrecognized directory
    /// (e.g. `plugin-watched/note.txt`) therefore classifies as
    /// `PathClass::Unclassified`, whose arm consults only
    /// `graph.dirty_pages` — empty — so a prerendered consumer page kept
    /// re-serving stale committed HTML indefinitely (#2181). `All` is the
    /// same first cut the `External` arm already takes; narrowing it would
    /// need per-specifier consumer provenance nothing here has (#1583).
    ///
    /// Called from THREE sites, all of which a watch target can reach:
    ///
    /// 1. the classified-path fold in [`Self::plan_for_changes`] — the
    ///    ordinary live-edit path;
    /// 2. that fold's `external_invalidation` override branch (#1038),
    ///    which `continue`s before reaching site 1. That hook narrows page
    ///    SELECTION only and never the asset-rebuild flags — the rule the
    ///    branch's other `mark_*` re-applications already encode — and for
    ///    a watch target it cannot inform page selection either, since the
    ///    virtual-module edge it would need does not exist in the graph;
    /// 3. [`Self::tick_with_kinds`]'s removed-path fold, which the
    ///    `plan_paths` filter likewise keeps away from site 1.
    fn apply_plugin_watch_invalidation(&self, plan: &mut RebuildPlan, path: &Path) {
        if !self.config.policy.is_plugin_watch_target(path) {
            return;
        }
        plan.mark_pages(PageSelection::All);
        plan.mark_islands();
        plan.mark_client_scripts();
        plan.mark_css();
        plan.mark_ssr_reload_needed();
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
                // #1288/#1804 — the same unconditional Module→mark_css rule
                // the main classified-path arm below applies (a `.tsx` edit
                // may author a new Tailwind utility class) must also apply
                // here: a narrowing hook only overrides the page SELECTION,
                // not the asset-rebuild flags (see the comment above this
                // arm), and without this line a hook-narrowed external
                // Module edit silently dropped the CSS content rescan.
                if matches!(class, PathClass::Module) {
                    plan.mark_css();
                }
                // #1819 — same reasoning one step further: a hook-narrowed
                // external path that happens to live under a claimed CSS
                // mirror root is still CSS input, and the hook narrows the
                // page SELECTION only.
                if self.content_under_css_mirror_root(class, &path) {
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
                    || self.config.policy.is_client_script_sibling_target(&path)
                {
                    plan.mark_client_scripts();
                }
                // #2181 — same reasoning as the asset flags above, one step
                // further: this branch `continue`s past the classified-path
                // fold, so an out-of-root plugin watch file that the hook
                // returned a verdict for would otherwise skip its
                // invalidation entirely and leave prerendered virtual-module
                // consumers stale. The hook cannot narrow this: the edge it
                // would need does not exist in the graph. See
                // [`Self::apply_plugin_watch_invalidation`].
                self.apply_plugin_watch_invalidation(&mut plan, &path);
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
                    // #1819 (epic #1995) — the mirror-root half of the same
                    // rule for `.md`/`.mdx`. See
                    // `content_under_css_mirror_root`.
                    if self.content_under_css_mirror_root(class, &path) {
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
                    // #1819 (epic #1995) — a whole-site page re-render does
                    // NOT rerun the CSS content scan. An external file under
                    // a claimed CSS mirror root is Tailwind `@source` input,
                    // so it needs the flag explicitly.
                    if self.content_under_css_mirror_root(class, &path) {
                        plan.mark_css();
                    }
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
                || self.config.policy.is_client_script_sibling_target(&path)
            {
                plan.mark_client_scripts();
            }
            // Issues #2169 / #2181 — a plugin virtual-module watch file (a
            // loader's `watchFiles` entry) invalidates every consumer that
            // could read the refreshed source, plus every page, regardless
            // of how the path itself classifies. See
            // [`Self::apply_plugin_watch_invalidation`].
            self.apply_plugin_watch_invalidation(&mut plan, &path);
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
        // Issue #1581 — a `Removed` path is no longer a known collection
        // entry, so forget it (and, for a removed directory, everything
        // beneath it) BEFORE the `Created` normalization below reads the
        // registry. Order is load-bearing: the watcher can batch a `Removed`
        // and a `Created` into the SAME tick, and purging afterwards would
        // let a genuine delete→recreate normalize to `Modified` and skip the
        // discovery regime that must re-establish its routes.
        for (path, kind) in &changes {
            if *kind == ChangeKind::Removed {
                self.config
                    .policy
                    .known_content
                    .remove_path_and_descendants(path);
            }
        }

        // Issue #1058 — normalize a spurious `Created` for an already-known
        // content source back to `Modified`. On a loaded arm64 macOS host,
        // FSEvents coalescing can deliver an in-place edit of an EXISTING
        // file as `Created` (see `zfb_watcher::merge_kind` rule 2). Left as
        // `Created` the change routes through the discovery regime (watch-ADD)
        // instead of the in-place-edit regime, so the lazy path never
        // eager-renders the edited entry's own route, and the strict #958
        // `fan_out_safe` gate below (all-`Modified`) is poisoned — costing
        // the whole tick its eager narrowing and re-stamping every route.
        //
        // "Already known" has TWO sources, and the registry is the load-
        // bearing one (issue #1581):
        //
        // - The session-live collection-entry registry (`known_content`) —
        //   seeded at boot from the collection membership walk and extended
        //   by discovery. This is authoritative.
        // - The dependency graph's reverse edge (`consumers_of` non-empty) —
        //   #1058's original check, kept because a warm PERSISTED graph can
        //   restore Content edges from a previous session. On a COLD boot it
        //   is always empty for a pre-existing entry (the dev server's only
        //   `DepKind::Content` writer is the discovery hook, which fires just
        //   for newly-created files), which is why #1058 alone never fired
        //   for the first edit of any boot-time entry.
        //
        // A genuinely new file is in neither, so it stays `Created` for
        // discovery. Known-ness is read OUTSIDE the graph mutex to keep the
        // two locks uncoupled.
        let known_created: Vec<bool> = changes
            .iter()
            .map(|(path, kind)| {
                *kind == ChangeKind::Created && self.config.policy.is_known_content_entry(path)
            })
            .collect();
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
                .zip(known_created)
                .map(|((path, kind), known)| {
                    let spurious_created = kind == ChangeKind::Created
                        && (known || graph.consumers_of(&path).is_some_and(|c| !c.is_empty()))
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
            // #1819 (epic #1995), widened by #2077 — DELETING a path under a
            // registered CSS mirror root always changes the Tailwind content
            // set: its classes must stop being emitted. Unlike the live-edit
            // arms above (which gate on `content_under_css_mirror_root`'s
            // `Content`/`Data`/`External` class list, because the narrower
            // `Module`/`Style` classes already `mark_css` unconditionally on
            // a LIVE edit), a removal has no such per-class shortcut — none
            // of the match arms above already covers a mirror-root deletion
            // — so this consults the class-agnostic
            // [`Self::path_under_css_mirror_root`] directly, unconditionally,
            // for EVERY removed-path class reached above: `Global` and
            // `Style` already call `mark_css` unconditionally (this is a
            // harmless no-op re-set for them), and `Page`/`Module`/`Content`/
            // `Data`/`External`/`Asset`/`Unclassified` all now gain the
            // mirror-root signal a `content_under_css_mirror_root(class, ..)`
            // call could never give `Module`/`Asset`/`Unclassified` — those
            // classes never pass its class gate.
            //
            // In-root deletions (any class) remain UNCHANGED:
            // `path_under_css_mirror_root` only matches a path inside a
            // REGISTERED mirror root, and a mirror root can never swallow the
            // project (`root_swallows_the_project`, checked inside the
            // helper), so a deleted in-root `.tsx` still does not rerun the
            // scan. That gap is PRE-EXISTING (deleted in-root modules have
            // always behaved this way) and deliberately out of scope here;
            // closing it is a broader behaviour change tracked separately.
            if self.path_under_css_mirror_root(path) {
                plan.mark_css();
            }
            if self.config.policy.is_islands_dependency(path) {
                plan.mark_islands();
            }
            if self.config.policy.is_client_script_candidate(path)
                || self.config.policy.is_client_script_raw_target(path)
                || self.config.policy.is_client_script_worker_target(path)
                || self.config.policy.is_client_script_sibling_target(path)
            {
                plan.mark_client_scripts();
            }
            // Issue #2181 — a removed path never reaches
            // `plan_for_changes`'s plugin-watch handling (the `plan_paths`
            // filter above excludes every `Removed` change to keep the
            // All-fallback away from deletions), yet the pre-tick refresh
            // gate IS kind-agnostic, so the loader is re-invoked for a
            // deleted watch file too: one that intentionally handles a
            // missing optional file publishes fallback source into the
            // shared store, and without this nothing rebuilds the consumers
            // that read it — the fallback is published but never shipped.
            // `removed_consumers` cannot cover the pages either (an
            // edge-less watch file has none). See
            // [`Self::apply_plugin_watch_invalidation`].
            self.apply_plugin_watch_invalidation(&mut plan, path);
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
                // Issue #2190 — a plugin `watchFiles` target that is itself
                // under a collection root classifies `PathClass::Content`,
                // so a Modified-only tick touching it would otherwise set
                // `fan_out_safe: true` and narrow fan-out to that file's OWN
                // routes, undoing the `PageSelection::All` invalidation that
                // virtual-module consumer pages rely on. Reuse
                // `pre_tick_refresh_applies` verbatim (same predicate that
                // gates the pre-tick loader refresh) so "refresh ran this
                // tick" and "narrowing off this tick" cannot drift apart.
                fan_out_safe: modified_only_content
                    && !pre_tick_refresh_applies(&self.config.policy, &changes),
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
        on_outcome: F,
        boot: Option<B>,
    ) -> Result<()>
    where
        F: FnMut(&BuildOutcome) + Send + 'static,
        B: FnOnce(&BuildOrchestrator<P>, &BuildContext) -> Option<BuildOutcome>,
        P: 'static,
    {
        let (watcher, rx) = Watcher::start_with_options(
            &self.config.project_root,
            self.config.watch_roots.iter().map(|p| p.as_path()),
            self.config.extra_watch_paths.iter().map(|p| p.as_path()),
            watch_options_for(&self.config),
        )?;

        info!(
            project_root = %self.config.project_root.display(),
            "build orchestrator running"
        );

        self.run_drain_loop(ctx, discover, on_outcome, boot, watcher, rx)
            .await
    }

    /// The drain loop proper, extracted from [`run_with_boot`](Self::run_with_boot)
    /// (issue #2253) so tests can drive it with a synthetic, test-owned
    /// `Change` channel instead of real filesystem events reaching a real
    /// `Watcher`'s own channel, while still exercising this exact loop body.
    /// `watcher` and `rx` are already constructed — `run_with_boot` is the
    /// only production caller, and it always pairs a freshly booted
    /// `Watcher::start_with_options` with its own receiver.
    async fn run_drain_loop<F, B>(
        self,
        ctx: BuildContext,
        discover: Option<DiscoveryHook>,
        mut on_outcome: F,
        boot: Option<B>,
        mut watcher: Watcher,
        mut rx: tokio::sync::mpsc::Receiver<Change>,
    ) -> Result<()>
    where
        F: FnMut(&BuildOutcome) + Send + 'static,
        B: FnOnce(&BuildOrchestrator<P>, &BuildContext) -> Option<BuildOutcome>,
        P: 'static,
    {
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
        // The eager boot browser passes may discover islands, client raw
        // targets, or client-script worker dependencies outside the configured
        // source roots. Register each dependency's parent now so edits,
        // deletes, and recreations enter the same watcher channel. The
        // registry exposes the last successful closures, so a transient failed
        // rebuild never drops recovery watches.
        register_dynamic_dependency_watches(
            &mut watcher,
            &self.config.policy,
            &self.config.css_mirror_skip_dir_names,
        );

        // Deterministic fault-injection knobs (issue #2100, Dev Supervision
        // epic #2099 Sub #2100). Both are read here — AFTER the boot hook
        // has completed and the post-boot watch registration above has run
        // — never earlier, so a slow boot can never make either fault fire
        // before the watcher has performed its live handshake. See
        // `orch_panic_on_tick_armed` / `orch_stop_ms_decision` for the pure
        // parse/decision logic and the exact marker-line wording.
        let panic_on_tick_armed = orch_panic_on_tick_armed(
            std::env::var("ZFB_DEV_TEST_ORCH_PANIC_ON_TICK")
                .ok()
                .as_deref(),
        );
        if panic_on_tick_armed {
            eprintln!("[zfb-timing] fault armed: ZFB_DEV_TEST_ORCH_PANIC_ON_TICK");
        }
        let stop_deadline = orch_stop_ms_decision(
            std::env::var("ZFB_DEV_TEST_ORCH_STOP_MS").ok().as_deref(),
        )
        .map(|ms| {
            eprintln!("[zfb-timing] fault armed: ZFB_DEV_TEST_ORCH_STOP_MS {ms}ms");
            tokio::time::Instant::now() + Duration::from_millis(ms)
        });

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
        while let Some(first) = recv_with_stop_deadline(&mut rx, stop_deadline).await {
            let mut batch: Vec<Change> = vec![first];
            while let Ok(c) = rx.try_recv() {
                batch.push(c);
            }

            let changes: Vec<(PathBuf, ChangeKind)> =
                batch.iter().map(|c| (c.path.clone(), c.kind)).collect();

            // Watch-intake suppression (issue #2345) — applied to the raw
            // batch BEFORE the pre-tick refresh and the tick dispatch, so a
            // suppressed path's event never reaches ANY tick processing
            // (including `tick_with_kinds`'s removed-path fold and the
            // discovery hook). A batch left empty skips its tick entirely —
            // that skip is what lets the dev loop go idle after a CSS pass
            // instead of ticking on the pass's own temp-entry write.
            let unfiltered_len = changes.len();
            let changes = retain_unsuppressed_changes(&this.config, changes);
            let suppressed = unfiltered_len - changes.len();
            if suppressed > 0 && dev_timing_enabled() {
                eprintln!("[zfb-timing] intake: suppressed {suppressed} watch event(s)");
            }
            if changes.is_empty() {
                continue;
            }

            // Pre-tick plugin refresh (issue #2169) — awaited HERE, before
            // the tick is dispatched to the blocking pool, so the shared
            // virtual-module store already holds the re-invoked loader
            // output when the tick's snapshot-at-use-time consumers read
            // it. Awaited directly (the plugin-host call is async, not
            // blocking work); an `Err` is logged inside the helper and the
            // tick proceeds on the store's last-good memo — a refresh
            // failure must never kill the dev loop. See
            // [`PreTickRefreshHook`] / [`maybe_pre_tick_refresh`].
            maybe_pre_tick_refresh(&this.config, &changes).await;

            let tick = tokio::task::spawn_blocking(move || {
                // Fault injection (issue #2100): fires INSIDE this
                // spawn_blocking closure, on the thread that would
                // otherwise run `tick_with_kinds`, so the panic takes the
                // exact `resume_unwind` re-raise path below that a genuine
                // tick panic would.
                if panic_on_tick_armed {
                    eprintln!("[zfb-timing] fault fired: ZFB_DEV_TEST_ORCH_PANIC_ON_TICK");
                    panic!("ZFB_DEV_TEST_ORCH_PANIC_ON_TICK fault injection");
                }
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
            register_dynamic_dependency_watches(
                &mut watcher,
                &this.config.policy,
                &this.config.css_mirror_skip_dir_names,
            );
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

    // -----------------------------------------------------------------
    // Watch backend selection (issue #2174, constructor-selection site a)
    // -----------------------------------------------------------------

    #[test]
    fn watch_options_for_defaults_to_native_backend() {
        let config = OrchestratorConfig::new("/proj", vec![PathBuf::from("pages")]);
        let options = watch_options_for(&config);
        assert_eq!(options.backend, WatchBackend::Native);
    }

    #[test]
    fn watch_options_for_selects_poll_backend_when_configured() {
        let poll_backend = WatchBackend::Poll {
            interval: Duration::from_millis(250),
        };
        let config = OrchestratorConfig::new("/proj", vec![PathBuf::from("pages")])
            .with_backend(poll_backend);
        let options = watch_options_for(&config);
        assert_eq!(options.backend, poll_backend);
    }

    #[test]
    fn watch_options_for_carries_the_configured_debounce_alongside_the_backend() {
        let poll_backend = WatchBackend::Poll {
            interval: Duration::from_millis(250),
        };
        let config = OrchestratorConfig::new("/proj", vec![PathBuf::from("pages")])
            .with_debounce(Duration::from_millis(10))
            .with_backend(poll_backend);
        let options = watch_options_for(&config);
        assert_eq!(options.debounce, Duration::from_millis(10));
        assert_eq!(options.backend, poll_backend);
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

    /// Issue #1804 (Tailwind Sibling Source epic #1799, Wave 3 confirm
    /// pass): end-to-end proof of the invalidation chain a workspace-sibling
    /// mirror root relies on, with NO `external_invalidation` hook
    /// configured — the realistic zfb dev-server default (`grep -rn
    /// with_external_invalidation crates/zfb/src` finds no call site; only
    /// `zfb-build`'s own tests and the opt-in consumer API exercise the
    /// hook). The chain:
    ///
    /// 1. An out-of-root `.tsx` classifies as `PathClass::Module` via
    ///    extension sniff, NOT the in-tree root-segment walk — proven
    ///    directly by `out_of_root_paths_skip_root_segment_walk` in
    ///    `policy.rs` (`classify_change_with_content_roots`,
    ///    `policy.rs:349-358` region).
    /// 2. That `Module` classification hits the SAME unconditional #1288
    ///    `mark_css` rule `shared_component_dirties_all_consumers` (just
    ///    above) proves for an IN-root path — the main classified-path arm
    ///    does not distinguish in-root from out-of-root, only `PathClass`.
    /// 3. `plan.rerun_css` being set is what
    ///    `pipeline::dev::tests::css_rerun_invokes_callback` proves drives
    ///    `ctx.run_css()` (`pipeline/dev.rs`, the `if plan.rerun_css { .. }`
    ///    block).
    ///
    /// **Scope boundary — read before assuming this covers content too:**
    /// this chain holds for `.tsx` / `PathClass::Module` sibling edits
    /// ONLY. An out-of-root `.md`/`.mdx` edit classifies as
    /// `PathClass::Content` instead (see `classify_by_extension`), and
    /// `mark_css` is gated on `PathClass::Module` alone — so a
    /// Content-classified mirror-root edit does NOT rerun the Tailwind
    /// scan today, even though `discover_css_source_files` also scans
    /// `.md`/`.mdx`. That gap is tracked separately as issue #1819 (found
    /// during this epic's Wave 2 codex review) and is deliberately NOT
    /// fixed here — it shares planner logic with a much wider blast
    /// radius and needs its own test-first pass.
    #[test]
    fn out_of_root_module_change_without_hook_still_reruns_css() {
        let orch = make_orch(CountingPipeline::default());
        let plan = orch.plan_for_changes(vec![PathBuf::from("/srv/shared/lib/Utils.tsx")]);
        assert!(
            plan.pages.is_all(),
            "the graph has no edges for an unknown out-of-root module, so the \
             conservative PageSelection::All fallback applies here -- accepted \
             in this epic (narrowing aggregate-page provenance is issue #1583's \
             job, NOT this one); got {:?}",
            plan.pages,
        );
        assert!(
            plan.rerun_css,
            "an out-of-root Module change must still rerun the CSS content scan (#1288)"
        );
    }

    /// Issues #2063 / #2064 — what the dev server's content-provenance
    /// FAILURE path actually costs.
    ///
    /// When `reconcile_content_provenance` errors, the dev session wipes
    /// every `DepKind::Content` edge from this graph (see
    /// `replace_content_edges` in `crates/zfb/src/commands/dev.rs`). This
    /// test models exactly that end state — page `c` keeps its self-edge but
    /// no longer records `content/post.md` — and pins the consequence: the
    /// content path becomes UNKNOWN to the graph, which trips the
    /// conservative `PageSelection::All` fallback.
    ///
    /// That is the load-bearing fact for #2063's hypothesis. A provenance
    /// failure degrades to "rebuild EVERY page", never to "rebuild NO page",
    /// so it cannot be what empties `BuildOutcome::pages_stale` and gates
    /// `ReloadEvent::Page` out of `outcome_to_events`. #2063's missing
    /// reload therefore has a different cause than #2064.
    #[test]
    fn content_edit_after_a_provenance_wipe_falls_back_to_a_full_rebuild() {
        let mut g = DependencyGraph::new();
        // The post-wipe shape: the page survives, its Content edge does not.
        g.upsert(PageDeps::new(pid("/proj/pages/c.tsx"), vec![]));
        let orch = BuildOrchestrator::new(
            OrchestratorConfig::new(
                "/proj",
                vec![PathBuf::from("pages"), PathBuf::from("content")],
            ),
            Arc::new(Mutex::new(g)),
            CountingPipeline::default(),
        );

        let plan = orch.plan_for_changes(vec![PathBuf::from("/proj/content/post.md")]);
        assert!(
            plan.pages.is_all(),
            "a content path the graph no longer knows must take the conservative \
             whole-site fallback — narrowing off, NOT rendering off; got {:?}",
            plan.pages,
        );
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

    /// Issue #1804 (Tailwind Sibling Source epic #1799, Wave 3 confirm
    /// pass): the #1288 rule — a `Module` change may author a new Tailwind
    /// utility class, so `mark_css` fires unconditionally on any
    /// `Module`-classified change — was applied only in the main
    /// classified-path arm, never in this external-override arm. A
    /// narrowing hook could accept an out-of-root `.tsx` edit and the CSS
    /// content scan would never re-run, leaving a newly-introduced utility
    /// class unemitted from `dist/assets/styles-*.css`.
    ///
    /// `lib/` is deliberately NOT one of the default `islands_roots`
    /// (`components`, `src`), so `rerun_islands` stays false here — this
    /// isolates the CSS-only assertion from the islands rule the sibling
    /// test above already covers.
    #[test]
    fn external_hook_narrowing_module_still_reruns_css() {
        let hook: ExternalInvalidationHook =
            Arc::new(|_path: &Path| Some(vec![pid("/proj/pages/a.tsx")]));
        let orch = make_orch_with_external_hook(CountingPipeline::default(), hook);
        let plan = orch.plan_for_changes(vec![PathBuf::from("/srv/shared/lib/Utils.tsx")]);
        match &plan.pages {
            PageSelection::Specific(s) => {
                assert_eq!(*s, BTreeSet::from([pid("/proj/pages/a.tsx")]));
            }
            other => unreachable!("expected PageSelection::Specific, got {other:?}"),
        }
        assert!(
            plan.rerun_css,
            "narrowing an external Module path must still rerun the CSS content scan (#1288)"
        );
        assert!(
            !plan.rerun_islands,
            "lib/ is not an islands root by default; only the CSS rule should fire here"
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

    /// Issue #2190 — a plugin `watchFiles` target that is itself under a
    /// collection root classifies `PathClass::Content` (the root-segment
    /// walk, `policy::classify_change_with_content_roots`), so a
    /// Modified-only tick touching it would otherwise set
    /// `fan_out_safe: true` and the eager path would narrow fan-out to the
    /// watched file's own routes — undoing the `PageSelection::All`
    /// invalidation that virtual-module consumer pages rely on
    /// (prerendered pages importing the virtual module could serve stale
    /// content). See the CONTROL test right below, which pins that a
    /// non-watched sibling content edit is unaffected.
    #[test]
    fn content_classified_plugin_watch_file_tick_is_not_fan_out_safe() {
        use zfb_watcher::ChangeKind;
        let watched = PathBuf::from("/proj/content/watched.md");
        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        let orch = BuildOrchestrator::new(
            OrchestratorConfig::new(
                "/proj",
                vec![PathBuf::from("pages"), PathBuf::from("content")],
            )
            .with_policy(policy_with_plugin_watch_files([watched.clone()])),
            make_graph(),
            pipeline,
        );
        let dist = tempfile::tempdir().unwrap();

        // Precondition: prove the gate is actually armed for this path, or
        // the assertions below would read as coverage while guarding
        // nothing (the #1581 dead-guard lesson).
        assert!(
            orch.policy().is_plugin_watch_target(&watched),
            "the watched path must be registered as a plugin watch target"
        );

        orch.tick_with_kinds(
            vec![(watched.clone(), ChangeKind::Modified)],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();

        let plans = applies.lock().unwrap();
        let plan = plans
            .last()
            .expect("a plugin-watch content tick must produce a plan");
        assert_eq!(
            plan.content_narrowing,
            Some(crate::plan::ContentNarrowing {
                changed_content: vec![watched],
                fan_out_safe: false,
            }),
            "a Modified-only tick touching a Content-classified plugin watch file must NOT \
             be fan-out-safe (#2190) — the pre-tick loader refresh ran this tick, so the \
             eager path must not narrow fan-out to the watched file's own routes"
        );
        match &plan.pages {
            PageSelection::Specific(pages) => {
                let expected: std::collections::BTreeSet<PageId> = [
                    pid("/proj/pages/a.tsx"),
                    pid("/proj/pages/b.tsx"),
                    pid("/proj/pages/c.tsx"),
                ]
                .into_iter()
                .collect();
                assert_eq!(
                    *pages, expected,
                    "a plugin-watch content tick must still invalidate every page (the \
                     resolved form of PageSelection::All) — the narrowing hint alone must \
                     never narrow the plan's own page selection"
                );
            }
            PageSelection::All => unreachable!("resolve_all runs before the pipeline apply"),
        }
    }

    /// CONTROL for the test above (#2190): an identical Modified-only tick
    /// for a sibling content file that is NOT a plugin watch target must
    /// keep `fan_out_safe: true` — pins that the new gate cannot silently
    /// become always-false and regress the narrowing machinery's whole
    /// purpose.
    #[test]
    fn non_watched_sibling_content_tick_stays_fan_out_safe() {
        use zfb_watcher::ChangeKind;
        let watched = PathBuf::from("/proj/content/watched.md");
        let sibling = PathBuf::from("/proj/content/sibling.md");
        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        let orch = BuildOrchestrator::new(
            OrchestratorConfig::new(
                "/proj",
                vec![PathBuf::from("pages"), PathBuf::from("content")],
            )
            .with_policy(policy_with_plugin_watch_files([watched])),
            make_graph(),
            pipeline,
        );
        let dist = tempfile::tempdir().unwrap();

        orch.tick_with_kinds(
            vec![(sibling.clone(), ChangeKind::Modified)],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();

        let plans = applies.lock().unwrap();
        let plan = plans
            .last()
            .expect("a Modified-only content tick must produce a plan");
        assert_eq!(
            plan.content_narrowing,
            Some(crate::plan::ContentNarrowing {
                changed_content: vec![sibling],
                fan_out_safe: true,
            }),
            "a Modified-only tick for a non-watched sibling content file must stay \
             fan-out-safe (#2190 control) — the plugin-watch gate must not fire for paths \
             outside the watch set"
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

    /// Build an orchestrator whose policy carries a live known-content
    /// registry (issue #1581), pre-seeded with `known`.
    fn make_orch_with_known_content<P: AssetPipeline>(
        pipeline: P,
        known: &[&str],
    ) -> (BuildOrchestrator<P>, crate::policy::KnownContentEntries) {
        let registry = crate::policy::KnownContentEntries::default();
        registry.insert_many(known.iter().map(PathBuf::from));
        let orch = BuildOrchestrator::new(
            OrchestratorConfig::new(
                "/proj",
                vec![PathBuf::from("pages"), PathBuf::from("content")],
            )
            .with_policy(GranularityPolicy::default().with_known_content(registry.clone())),
            make_graph(),
            pipeline,
        );
        (orch, registry)
    }

    /// #1581 — the regression this issue is about. `other.md` is a content
    /// entry with NO graph reverse edge (`consumers_of` is `None`), which is
    /// the state of EVERY collection entry on a cold `zfb dev` boot: the only
    /// `DepKind::Content` writer is the discovery hook, and it fires just for
    /// newly-created files. #1058's graph-keyed normalization therefore never
    /// fired for a pre-existing entry, so the first macOS FSEvents-coalesced
    /// `Created` for it lost the whole tick's #958 eager narrowing.
    ///
    /// With the entry in the known-content registry the `Created` is now
    /// recognized as the artifact it is and normalized to `Modified` →
    /// `fan_out_safe`, WITHOUT the graph needing any Content edge.
    #[test]
    fn edge_less_known_boot_entry_created_normalizes_to_modified() {
        use zfb_watcher::ChangeKind;
        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        // `other.md` deliberately has no consumers in `make_graph()`.
        let (orch, _registry) = make_orch_with_known_content(pipeline, &["/proj/content/other.md"]);
        let dist = tempfile::tempdir().unwrap();

        let boot_entry = PathBuf::from("/proj/content/other.md");
        orch.tick_with_kinds(
            vec![(boot_entry.clone(), ChangeKind::Created)],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();

        let plans = applies.lock().unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(
            plans[0].content_narrowing,
            Some(crate::plan::ContentNarrowing {
                changed_content: vec![boot_entry],
                fan_out_safe: true,
            }),
            "a Created for a registry-known boot entry must normalize to a \
             fan-out-safe edit even though the graph has no Content edge for it"
        );
    }

    /// #1581 — the discrimination that keeps discovery working: a file the
    /// registry does NOT know is genuinely new, so it stays `Created` and
    /// still poisons the strict gate (routing it through the discovery
    /// regime that must establish its routes).
    #[test]
    fn genuinely_new_entry_absent_from_registry_stays_created() {
        use zfb_watcher::ChangeKind;
        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        // Registry knows `other.md`, but the tick creates `brand-new.md`.
        let (orch, _registry) = make_orch_with_known_content(pipeline, &["/proj/content/other.md"]);
        let dist = tempfile::tempdir().unwrap();

        let brand_new = PathBuf::from("/proj/content/brand-new.md");
        orch.tick_with_kinds(
            vec![(brand_new.clone(), ChangeKind::Created)],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();

        let plans = applies.lock().unwrap();
        assert_eq!(plans.len(), 1);
        assert!(
            !plans[0]
                .content_narrowing
                .as_ref()
                .expect("content narrowing hint")
                .fan_out_safe,
            "a Created for a file the registry has never seen is genuinely new \
             and must NOT be normalized — it belongs to the discovery regime"
        );
    }

    /// #1581 — delete→recreate must still re-discover. A `Removed` purges the
    /// path from the registry, so the NEXT tick's `Created` for it is treated
    /// as genuinely new again rather than as an in-place-edit artifact.
    #[test]
    fn removed_entry_is_purged_so_a_later_recreate_still_discovers() {
        use zfb_watcher::ChangeKind;
        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        let (orch, registry) = make_orch_with_known_content(pipeline, &["/proj/content/other.md"]);
        let dist = tempfile::tempdir().unwrap();

        let entry = PathBuf::from("/proj/content/other.md");
        assert!(registry.contains(&entry), "precondition: seeded as known");

        // Tick 1 — the file is deleted.
        orch.tick_with_kinds(
            vec![(entry.clone(), ChangeKind::Removed)],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();
        assert!(
            !registry.contains(&entry),
            "a Removed must purge the path from the known-content registry"
        );

        // Tick 2 — it comes back. It is new again, so it must not normalize.
        orch.tick_with_kinds(
            vec![(entry.clone(), ChangeKind::Created)],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();

        let plans = applies.lock().unwrap();
        assert!(
            !plans[1]
                .content_narrowing
                .as_ref()
                .expect("content narrowing hint")
                .fan_out_safe,
            "after a delete, a recreate must route through discovery again — \
             not be mistaken for a spurious FSEvents Created"
        );
    }

    /// #1581 — the purge must run BEFORE the normalization, not after: the
    /// watcher can batch a removed DIRECTORY and a `Created` for a file
    /// beneath it into the SAME tick. Purging afterwards would let the child
    /// normalize to `Modified` off a registry entry that is already dead.
    #[test]
    fn removed_directory_purges_descendants_before_normalizing_same_tick() {
        use zfb_watcher::ChangeKind;
        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        let (orch, registry) =
            make_orch_with_known_content(pipeline, &["/proj/content/nested/x.md"]);
        let dist = tempfile::tempdir().unwrap();

        let dir = PathBuf::from("/proj/content/nested");
        let child = PathBuf::from("/proj/content/nested/x.md");
        orch.tick_with_kinds(
            vec![
                (dir, ChangeKind::Removed),
                (child.clone(), ChangeKind::Created),
            ],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();

        assert!(
            !registry.contains(&child),
            "removing a directory must purge its descendants from the registry"
        );
        let plans = applies.lock().unwrap();
        assert!(
            !plans[0]
                .content_narrowing
                .as_ref()
                .expect("content narrowing hint")
                .fan_out_safe,
            "the same-tick Created under a removed directory must NOT normalize \
             — the purge runs first, so the child is no longer known"
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

    /// A removed `pages/*.client.ts` entry is excluded from the ordinary
    /// change fold because deletion must not trigger the unknown-path
    /// All-fallback. The explicit Removed fold must nevertheless rerun the
    /// client-script pass so the vanished entry is removed from the next
    /// publication generation.
    #[test]
    fn removed_client_script_under_pages_sets_rerun_client_scripts() {
        use zfb_watcher::ChangeKind;

        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        let orch = make_orch(pipeline);
        let dist = tempfile::tempdir().unwrap();
        let removed = PathBuf::from("/proj/pages/analytics.client.ts");

        orch.tick_with_kinds(
            vec![(removed, ChangeKind::Removed)],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();

        let plans = applies.lock().unwrap();
        assert_eq!(plans.len(), 1);
        assert!(
            plans[0].rerun_client_scripts,
            "removing a pages/*.client.ts entry must rerun client scripts"
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
    fn client_raw_dependency_planning_survives_delete_recreate_and_replaces_stale_targets() {
        let invalidation = crate::policy::RawImportInvalidation::default();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("pages")).unwrap();
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let payload = root.join("lib/client-payload.txt");
        let next_payload = root.join("lib/next-client-payload.txt");
        std::fs::write(&payload, "generation one\n").unwrap();
        invalidation.replace_client_scripts([payload.clone()]);
        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        let config = OrchestratorConfig::new(&root, vec![PathBuf::from("pages")]).with_policy(
            crate::policy::GranularityPolicy::default()
                .with_raw_import_invalidation(invalidation.clone()),
        );
        let orch = BuildOrchestrator::new(config, make_graph(), pipeline);
        let dist = tempfile::tempdir().unwrap();

        orch.tick_with_kinds(
            vec![(payload.clone(), ChangeKind::Modified)],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();
        let edited = applies.lock().unwrap().last().unwrap().clone();
        assert!(edited.rerun_client_scripts);
        assert!(
            !edited.rerun_islands,
            "client-owned raw targets must not cross-classify as islands dependencies"
        );

        std::fs::remove_file(&payload).unwrap();
        orch.tick_with_kinds(
            vec![(payload.clone(), ChangeKind::Removed)],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();
        let deleted = applies.lock().unwrap().last().unwrap().clone();
        assert!(deleted.rerun_client_scripts);
        assert!(!deleted.rerun_islands);

        // The lexical alias remains live while the target is absent, so a
        // recreate can recover. The next successful bundle then atomically
        // replaces it with the newly-discovered target.
        std::fs::write(&payload, "generation two\n").unwrap();
        let recreated = orch.plan_for_changes([payload.clone()]);
        assert!(recreated.rerun_client_scripts);
        assert!(!recreated.rerun_islands);
        std::fs::write(&next_payload, "next generation\n").unwrap();
        invalidation.replace_client_scripts([next_payload.clone()]);
        let stale_tick = orch.plan_for_changes([payload]);
        assert!(
            !stale_tick.rerun_client_scripts,
            "successful client raw graph replacement must clear stale ownership"
        );
        let next_tick = orch.plan_for_changes([next_payload]);
        assert!(next_tick.rerun_client_scripts);
        assert!(!next_tick.rerun_islands);
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

    /// Issue #1710: a workspace-sibling PLAIN module (neither a `?raw` target
    /// nor a worker dependency) must still trigger `mark_client_scripts()` —
    /// this is the orchestrator-side half of the bug fix, mirroring the
    /// worker-registry test above for the new `client_script_siblings` set.
    #[test]
    fn client_script_sibling_dependency_replacement_stops_stale_pipeline_planning() {
        let invalidation = crate::policy::RawImportInvalidation::default();
        let old_sibling = PathBuf::from("/workspace/lib/shared/plain.ts");
        let next_sibling = PathBuf::from("/workspace/lib/shared/next-plain.ts");
        invalidation.replace_client_script_siblings([old_sibling.clone()]);
        let config = OrchestratorConfig::new("/proj", vec![PathBuf::from("pages")]).with_policy(
            crate::policy::GranularityPolicy::default()
                .with_raw_import_invalidation(invalidation.clone()),
        );
        let orch = BuildOrchestrator::new(config, make_graph(), CountingPipeline::default());

        let first_tick = orch.plan_for_changes([old_sibling.clone()]);
        assert!(
            first_tick.rerun_client_scripts,
            "a sibling plain-module edit must re-emit the owning client script"
        );
        assert!(!first_tick.rerun_islands);

        invalidation.replace_client_script_siblings([next_sibling.clone()]);
        let stale_tick = orch.plan_for_changes([old_sibling]);
        assert!(
            !stale_tick.rerun_client_scripts,
            "a sibling that stopped being reachable must clear stale invalidation ownership"
        );
        assert!(orch.plan_for_changes([next_sibling]).rerun_client_scripts);
    }

    /// Issue #1711 (Sibling Invalidation epic #1709, confirm pass) — the
    /// DELETION gate. `tick_with_kinds` excludes removed paths from
    /// `plan_for_changes`'s classification (the cold-start All-fallback
    /// would be wrong for an intentional deletion) and instead applies the
    /// `is_client_script_*_target` checks directly against the removed path
    /// in its own loop. `is_client_script_sibling_target` was added to that
    /// loop alongside raw/worker in #1710; this proves the sibling registry
    /// is actually wired into the deletion path, not just the two edit-time
    /// gates already covered above. Mirrors
    /// `client_raw_dependency_planning_survives_delete_recreate_and_replaces_stale_targets`.
    #[test]
    fn client_script_sibling_dependency_deletion_reruns_client_scripts_and_clears_stale_ownership()
    {
        let invalidation = crate::policy::RawImportInvalidation::default();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("pages")).unwrap();
        std::fs::create_dir_all(root.join("lib/shared")).unwrap();
        let sibling = root.join("lib/shared/plain.ts");
        let next_sibling = root.join("lib/shared/next-plain.ts");
        std::fs::write(&sibling, "export const plain = 'ZFB_SIBLING';\n").unwrap();
        invalidation.replace_client_script_siblings([sibling.clone()]);
        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        let config = OrchestratorConfig::new(&root, vec![PathBuf::from("pages")]).with_policy(
            crate::policy::GranularityPolicy::default()
                .with_raw_import_invalidation(invalidation.clone()),
        );
        let orch = BuildOrchestrator::new(config, make_graph(), pipeline);
        let dist = tempfile::tempdir().unwrap();

        orch.tick_with_kinds(
            vec![(sibling.clone(), ChangeKind::Modified)],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();
        let edited = applies.lock().unwrap().last().unwrap().clone();
        assert!(edited.rerun_client_scripts);
        assert!(!edited.rerun_islands);

        std::fs::remove_file(&sibling).unwrap();
        orch.tick_with_kinds(
            vec![(sibling.clone(), ChangeKind::Removed)],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();
        let deleted = applies.lock().unwrap().last().unwrap().clone();
        assert!(
            deleted.rerun_client_scripts,
            "deleting a sibling plain module must still rerun client scripts \
             so the owning bundle is re-emitted without the removed import"
        );
        assert!(!deleted.rerun_islands);

        // Stale-ownership hygiene: after a deletion the lexical alias stays
        // live (recreate can recover), but once a successful bundle replaces
        // the graph with a different target, the old path must stop owning
        // client-script reruns.
        invalidation.replace_client_script_siblings([next_sibling.clone()]);
        let stale_tick = orch.plan_for_changes([sibling]);
        assert!(
            !stale_tick.rerun_client_scripts,
            "successful sibling graph replacement must clear stale ownership after a deletion"
        );
        assert!(orch.plan_for_changes([next_sibling]).rerun_client_scripts);
    }

    // -----------------------------------------------------------------
    // Issue #1819 / epic #1995 — option (b): `PathClass::Content` reruns
    // the Tailwind content scan ONLY under a registered CSS mirror root.
    // -----------------------------------------------------------------

    /// Build a workspace-shaped fixture whose HOST project sits at
    /// `sub-packages/host` and whose claimed CSS sibling-mirror root is
    /// `lib/ushared`, with the mirror root already published into a fresh
    /// [`crate::policy::RawImportInvalidation`]. Returns the host project
    /// root, the mirror root, and the policy carrying the registry.
    fn css_mirror_root_fixture(ws: &std::path::Path) -> (PathBuf, PathBuf, GranularityPolicy) {
        let project = ws.join("sub-packages/host");
        std::fs::create_dir_all(project.join("pages")).unwrap();
        std::fs::create_dir_all(project.join("content")).unwrap();
        let mirror_root = ws.join("lib/ushared");
        std::fs::create_dir_all(&mirror_root).unwrap();

        let invalidation = crate::policy::RawImportInvalidation::default();
        invalidation.replace_css_mirror_roots([mirror_root.clone()]);
        let policy = GranularityPolicy::default().with_raw_import_invalidation(invalidation);
        (project, mirror_root, policy)
    }

    /// The infra dir names the `zfb` command layer threads down in dev
    /// (`CSS_SIBLING_MIRROR_SKIP_DIRS`, via
    /// `OrchestratorConfig::with_css_mirror_skip_dir_names`). Spelled here
    /// only to give these unit fixtures the production wiring — the gate
    /// itself never carries a list of its own.
    fn css_mirror_skip_dir_names() -> Vec<String> {
        [
            "node_modules",
            "dist",
            ".git",
            "target",
            ".turbo",
            ".next",
            ".vercel",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }

    fn orch_for_css_mirror_root<P: AssetPipeline>(
        pipeline: P,
        project: &std::path::Path,
        policy: GranularityPolicy,
    ) -> BuildOrchestrator<P> {
        BuildOrchestrator::new(
            OrchestratorConfig::new(
                project,
                vec![PathBuf::from("pages"), PathBuf::from("content")],
            )
            .with_policy(policy)
            .with_css_mirror_skip_dir_names(css_mirror_skip_dir_names()),
            make_graph(),
            pipeline,
        )
    }

    /// The positive case, plus the MANDATORY registry-population assertion.
    ///
    /// `l-lessons-dev-watcher-narrowing`: #1058 shipped a guard that was
    /// correct in shape and dead in practice for two releases because the
    /// registry it keyed on was never populated in the guarded scenario. So
    /// this test proves BOTH halves — `css_mirror_root_paths()` is non-empty
    /// AND the edited path actually resolves inside one of those roots.
    /// (The end-to-end half — that the real dev boot CSS pass publishes a
    /// root containing a sibling `.mdx` — is asserted in the `zfb` crate by
    /// `dev::tests::dev_boot_css_mirror_roots_cover_a_sibling_mdx_file`.)
    #[test]
    fn sibling_mirror_root_mdx_edit_reruns_css() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().canonicalize().unwrap();
        let (project, mirror_root, policy) = css_mirror_root_fixture(&ws);
        let sibling_mdx = mirror_root.join("notes.mdx");
        std::fs::write(&sibling_mdx, "# sibling notes\n").unwrap();

        let roots = policy.css_mirror_root_paths();
        assert!(
            !roots.is_empty(),
            "the mirror-root registry the gate is keyed on must be POPULATED in the \
             scenario it guards — an empty registry makes the gate dead code that \
             reads as coverage (see #1058 / l-lessons-dev-watcher-narrowing)"
        );
        assert!(
            policy.is_under_css_mirror_root(&sibling_mdx),
            "a populated registry that does not actually CONTAIN the edited path is the \
             same dead guard with extra steps: {roots:?} vs {}",
            sibling_mdx.display()
        );

        assert_eq!(
            classify_change_with_content_roots(
                &sibling_mdx,
                &project,
                &[PathBuf::from("pages"), PathBuf::from("content")],
                |_| false,
            ),
            PathClass::Content,
            "fixture sanity: the out-of-root .mdx must classify as Content, which is \
             exactly the class #1288's Module-only mark_css rule skipped"
        );

        let orch = orch_for_css_mirror_root(CountingPipeline::default(), &project, policy);
        let plan = orch.plan_for_changes([sibling_mdx]);

        assert!(
            plan.rerun_css,
            "a .mdx edit inside a claimed CSS mirror root must rerun the Tailwind \
             content scan (#1819)"
        );
        // Page selection is deliberately UNTOUCHED by this epic: an unknown
        // content path must keep tripping the conservative All-fallback,
        // which is currently the only thing re-rendering aggregate pages
        // (issue #1583).
        assert!(
            plan.pages.is_all(),
            "this epic adds a CSS-rerun signal only — page selection must be unchanged"
        );
    }

    /// The negative that distinguishes option (b) from the REJECTED option
    /// (a) (`Content` → `mark_css` unconditionally). An ordinary in-root
    /// markdown edit must NOT gain a Tailwind rescan — that would be a real
    /// dev-loop cost on content-heavy sites.
    ///
    /// The registry is non-empty here on purpose: the gate must discriminate
    /// by LOCATION, not merely be switched off.
    #[test]
    fn in_root_content_edit_outside_mirror_roots_does_not_rerun_css() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().canonicalize().unwrap();
        let (project, _mirror_root, policy) = css_mirror_root_fixture(&ws);
        let in_root_mdx = project.join("content/post.mdx");
        std::fs::write(&in_root_mdx, "# ordinary post\n").unwrap();

        assert!(
            !policy.css_mirror_root_paths().is_empty(),
            "the registry must be populated, or this negative proves nothing"
        );
        assert!(!policy.is_under_css_mirror_root(&in_root_mdx));

        let orch = orch_for_css_mirror_root(CountingPipeline::default(), &project, policy);
        let plan = orch.plan_for_changes([in_root_mdx]);

        assert!(
            !plan.rerun_css,
            "an ordinary in-root markdown edit must NOT rerun the Tailwind content \
             scan — that is the whole reason option (b) was chosen over the \
             unconditional option (a)"
        );
    }

    /// The gate's NON-DEGENERACY, asserted directly.
    ///
    /// `is_under_css_mirror_root` is a subtree test, so a root that CONTAINS
    /// `project_root` matches every path in the project — silently turning
    /// option (b) into the rejected option (a) (every ordinary markdown edit
    /// reruns the Tailwind scan) with no other test failing. Today the
    /// registry cannot hold such a root, because `resolve_mirror_root`
    /// rejects project-containing claims
    /// (`bundler::tests::resolve_mirror_root_never_returns_an_ancestor_of_project_root`);
    /// this pins the gate's own defensive re-check so a future claim-policy
    /// change cannot quietly widen it.
    #[test]
    fn degenerate_project_containing_mirror_root_does_not_rerun_css() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().canonicalize().unwrap();
        let (project, _mirror_root, _policy) = css_mirror_root_fixture(&ws);
        let in_root_mdx = project.join("content/post.mdx");
        std::fs::write(&in_root_mdx, "# ordinary post\n").unwrap();

        // Publish a root that swallows the project — the exact shape
        // `resolve_mirror_root` refuses to produce.
        let invalidation = crate::policy::RawImportInvalidation::default();
        invalidation.replace_css_mirror_roots([ws.join("sub-packages")]);
        let policy = GranularityPolicy::default().with_raw_import_invalidation(invalidation);
        assert!(
            policy.is_under_css_mirror_root(&in_root_mdx),
            "fixture sanity: the bare subtree test DOES match here — that is \
             precisely why the gate needs its own guard"
        );

        let orch = orch_for_css_mirror_root(CountingPipeline::default(), &project, policy);
        assert!(
            !orch.plan_for_changes([in_root_mdx]).rerun_css,
            "a mirror root containing the project must not make every in-root \
             markdown edit rerun the Tailwind scan — that is option (a), which \
             this epic rejected"
        );
    }

    /// The gate must apply the SAME infra-dir exclusions the `@source` scan
    /// applies (`CSS_SIBLING_MIRROR_SKIP_DIRS`, threaded down as
    /// `OrchestratorConfig::css_mirror_skip_dir_names`). A build artifact
    /// under a sibling's `dist/` cannot change the emitted CSS — the
    /// exclusion globs guarantee Tailwind never reads it — so rerunning the
    /// scan for it is pure cost, and a gate that disagreed with the scan
    /// would be a second, drifting definition of "inside a claimed mirror
    /// region".
    #[test]
    fn mirror_root_infra_dir_event_does_not_rerun_css() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().canonicalize().unwrap();
        let (project, mirror_root, policy) = css_mirror_root_fixture(&ws);

        let generated = mirror_root.join("dist/generated.mdx");
        std::fs::create_dir_all(generated.parent().unwrap()).unwrap();
        std::fs::write(&generated, "# generated\n").unwrap();
        let nested = mirror_root.join("pkg/node_modules/dep/readme.md");
        std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
        std::fs::write(&nested, "# vendored\n").unwrap();
        let source = mirror_root.join("docs/notes.mdx");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "# real source\n").unwrap();

        let orch = orch_for_css_mirror_root(CountingPipeline::default(), &project, policy);

        assert!(
            !orch.plan_for_changes([generated]).rerun_css,
            "a file under a mirror root's `dist/` is excluded from the @source \
             scan, so it can never change the emitted CSS"
        );
        assert!(
            !orch.plan_for_changes([nested]).rerun_css,
            "the skip-dir filter must apply at ANY depth, not just directly \
             under the mirror root"
        );
        assert!(
            orch.plan_for_changes([source]).rerun_css,
            "a genuine source file elsewhere under the same root must still \
             rerun the scan — the filter must not break sibling scanning \
             wholesale"
        );
    }

    /// The removed-path fold: deleting a mirror-root markdown file changes
    /// the Tailwind content set too (its classes must stop being emitted),
    /// so the fold applies the same rule as the live arm.
    #[test]
    fn sibling_mirror_root_mdx_removal_reruns_css() {
        use zfb_watcher::ChangeKind;
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().canonicalize().unwrap();
        let (project, mirror_root, policy) = css_mirror_root_fixture(&ws);
        let sibling_mdx = mirror_root.join("notes.mdx");
        std::fs::write(&sibling_mdx, "# sibling notes\n").unwrap();

        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        let orch = orch_for_css_mirror_root(pipeline, &project, policy);
        let dist = tempfile::tempdir().unwrap();

        std::fs::remove_file(&sibling_mdx).unwrap();
        orch.tick_with_kinds(
            vec![(sibling_mdx, ChangeKind::Removed)],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();

        let plan = applies.lock().unwrap().last().unwrap().clone();
        assert!(
            plan.rerun_css,
            "deleting a mirror-root .mdx must rerun the content scan so the classes it \
             was the sole source of stop being emitted"
        );
    }

    /// Issue #2077: the gap `sibling_mirror_root_mdx_removal_reruns_css`
    /// left open. Deleting a mirror-root `PathClass::Module` (`.tsx`) file
    /// leaves its utility classes in the served stylesheet until restart,
    /// because the pre-#2077 removed-path fold only ever consulted
    /// `content_under_css_mirror_root`, whose class gate never accepts
    /// `Module`. This is the RED-before / GREEN-after test for the fix —
    /// see the PR body for the recorded RED→GREEN transcript.
    #[test]
    fn sibling_mirror_root_module_removal_reruns_css() {
        use zfb_watcher::ChangeKind;
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().canonicalize().unwrap();
        let (project, mirror_root, policy) = css_mirror_root_fixture(&ws);
        let sibling_tsx = mirror_root.join("Widget.tsx");
        std::fs::write(&sibling_tsx, "export const Widget = () => null;\n").unwrap();

        assert_eq!(
            classify_change_with_content_roots(
                &sibling_tsx,
                &project,
                &[PathBuf::from("pages"), PathBuf::from("content")],
                |_| false,
            ),
            PathClass::Module,
            "fixture sanity: an out-of-root .tsx must classify as Module — exactly the \
             class `content_under_css_mirror_root`'s class gate always rejected"
        );

        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        let orch = orch_for_css_mirror_root(pipeline, &project, policy);
        let dist = tempfile::tempdir().unwrap();

        std::fs::remove_file(&sibling_tsx).unwrap();
        orch.tick_with_kinds(
            vec![(sibling_tsx, ChangeKind::Removed)],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();

        let plan = applies.lock().unwrap().last().unwrap().clone();
        assert!(
            plan.rerun_css,
            "deleting a mirror-root .tsx must rerun the content scan so the utility \
             classes it was the sole source of stop being emitted (#2077)"
        );
    }

    /// Negative paired with the RED test above: an ORDINARY in-root Module
    /// removal must NOT gain a Tailwind rescan — that gap is documented,
    /// pre-existing, and deliberately out of scope for #2077 (see the fold's
    /// own doc comment). Must pass BOTH before and after the fix, proving
    /// in-root behavior is genuinely unchanged rather than merely uncovered.
    #[test]
    fn in_root_module_removal_does_not_rerun_css() {
        use zfb_watcher::ChangeKind;
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().canonicalize().unwrap();
        let (project, _mirror_root, policy) = css_mirror_root_fixture(&ws);
        std::fs::create_dir_all(project.join("components")).unwrap();
        let in_root_tsx = project.join("components/Widget.tsx");
        std::fs::write(&in_root_tsx, "export const Widget = () => null;\n").unwrap();

        assert_eq!(
            classify_change_with_content_roots(
                &in_root_tsx,
                &project,
                &[PathBuf::from("pages"), PathBuf::from("content")],
                |_| false,
            ),
            PathClass::Module,
            "fixture sanity: an in-root .tsx under components/ must classify as Module"
        );
        assert!(
            !policy.is_under_css_mirror_root(&in_root_tsx),
            "fixture sanity: the in-root path must not itself be under the registered \
             mirror root, or this negative proves nothing"
        );

        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        let orch = orch_for_css_mirror_root(pipeline, &project, policy);
        let dist = tempfile::tempdir().unwrap();

        std::fs::remove_file(&in_root_tsx).unwrap();
        orch.tick_with_kinds(
            vec![(in_root_tsx, ChangeKind::Removed)],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();

        let plan = applies.lock().unwrap().last().unwrap().clone();
        assert!(
            !plan.rerun_css,
            "an in-root Module deletion must NOT rerun the Tailwind content scan — that \
             gap is documented and deliberately out of scope for #2077"
        );
    }

    /// Negative paired with the RED test above: a removal under a mirror
    /// root's `dist/` (a `css_mirror_skip_dir_names` infra dir) must NOT
    /// rerun the Tailwind scan, matching the live-edit arm's own
    /// `mirror_root_infra_dir_event_does_not_rerun_css` — the skip-dir
    /// exclusion must hold for the removed-path fold too, even though the
    /// fold now consults the class-agnostic check unconditionally.
    #[test]
    fn mirror_root_infra_dir_module_removal_does_not_rerun_css() {
        use zfb_watcher::ChangeKind;
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().canonicalize().unwrap();
        let (project, mirror_root, policy) = css_mirror_root_fixture(&ws);
        let generated_tsx = mirror_root.join("dist/Generated.tsx");
        std::fs::create_dir_all(generated_tsx.parent().unwrap()).unwrap();
        std::fs::write(&generated_tsx, "export const Generated = () => null;\n").unwrap();

        assert_eq!(
            classify_change_with_content_roots(
                &generated_tsx,
                &project,
                &[PathBuf::from("pages"), PathBuf::from("content")],
                |_| false,
            ),
            PathClass::Module,
            "fixture sanity: an out-of-root .tsx must classify as Module, so this \
             negative genuinely exercises the skip-dir exclusion against the SAME \
             class the RED test above proves the fold now covers"
        );
        assert!(
            policy.is_under_css_mirror_root(&generated_tsx),
            "fixture sanity: the path must be under the registered mirror root as a \
             bare subtree match, or this negative proves nothing about the skip-dir \
             exclusion specifically"
        );

        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        let orch = orch_for_css_mirror_root(pipeline, &project, policy);
        let dist = tempfile::tempdir().unwrap();

        std::fs::remove_file(&generated_tsx).unwrap();
        orch.tick_with_kinds(
            vec![(generated_tsx, ChangeKind::Removed)],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();

        let plan = applies.lock().unwrap().last().unwrap().clone();
        assert!(
            !plan.rerun_css,
            "a removal under a mirror root's dist/ is excluded from the @source scan, \
             so it can never change the emitted CSS"
        );
    }

    /// `PathClass::Data`: an out-of-root `.json`/`.yaml` inside a mirror
    /// root is read by the same whole-subtree `@source` scan, so a class
    /// token authored there is CSS input like any other. Raised by codex
    /// review of the first #1997 pass, which covered only `Content` and
    /// `External`.
    #[test]
    fn sibling_mirror_root_data_file_reruns_css() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().canonicalize().unwrap();
        let (project, mirror_root, policy) = css_mirror_root_fixture(&ws);
        let data = mirror_root.join("tokens.json");
        std::fs::write(&data, "{\"cls\":\"bg-[#1a2b3c]\"}\n").unwrap();

        assert_eq!(
            classify_change_with_content_roots(
                &data,
                &project,
                &[PathBuf::from("pages"), PathBuf::from("content")],
                |_| false,
            ),
            PathClass::Data,
            "fixture sanity: an out-of-root .json classifies Data"
        );

        let orch = orch_for_css_mirror_root(CountingPipeline::default(), &project, policy);
        assert!(orch.plan_for_changes([data]).rerun_css);

        // Discrimination: an in-root data file is untouched.
        let tmp2 = tempfile::tempdir().unwrap();
        let ws2 = tmp2.path().canonicalize().unwrap();
        let (project2, _mirror2, policy2) = css_mirror_root_fixture(&ws2);
        let in_root_data = project2.join("content/tokens.json");
        std::fs::write(&in_root_data, "{}\n").unwrap();
        let orch2 = orch_for_css_mirror_root(CountingPipeline::default(), &project2, policy2);
        assert!(!orch2.plan_for_changes([in_root_data]).rerun_css);
    }

    /// The `External` arm: a mirror root can hold files whose extension is
    /// not on the classifier's whitelist (an out-of-root `.vue` classifies
    /// `External`), while Tailwind's own `@source` scanner still reads them.
    /// The live `External` arm's `PageSelection::All` does not imply a CSS
    /// rescan, so the flag is set explicitly.
    #[test]
    fn sibling_mirror_root_external_file_reruns_css() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().canonicalize().unwrap();
        let (project, mirror_root, policy) = css_mirror_root_fixture(&ws);
        let external = mirror_root.join("widget.vue");
        std::fs::write(&external, "<template />\n").unwrap();

        assert_eq!(
            classify_change_with_content_roots(
                &external,
                &project,
                &[PathBuf::from("pages"), PathBuf::from("content")],
                |_| false,
            ),
            PathClass::External,
            "fixture sanity: a non-whitelisted out-of-root extension classifies External"
        );

        let orch = orch_for_css_mirror_root(CountingPipeline::default(), &project, policy);
        assert!(orch.plan_for_changes([external]).rerun_css);

        // Discrimination: the same extension OUTSIDE any mirror root stays
        // CSS-inert.
        let tmp2 = tempfile::tempdir().unwrap();
        let ws2 = tmp2.path().canonicalize().unwrap();
        let (project2, _mirror2, policy2) = css_mirror_root_fixture(&ws2);
        let unrelated = ws2.join("elsewhere/widget.vue");
        std::fs::create_dir_all(unrelated.parent().unwrap()).unwrap();
        std::fs::write(&unrelated, "<template />\n").unwrap();
        let orch2 = orch_for_css_mirror_root(CountingPipeline::default(), &project2, policy2);
        assert!(!orch2.plan_for_changes([unrelated]).rerun_css);
    }

    /// The hook-interception arm (`external_overrides`): a narrowing hook
    /// overrides the page SELECTION only, never the asset-rebuild flags —
    /// the same reasoning #1804 applied to `Module`.
    #[test]
    fn hook_narrowed_mirror_root_mdx_still_reruns_css() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().canonicalize().unwrap();
        let (project, mirror_root, policy) = css_mirror_root_fixture(&ws);
        let sibling_mdx = mirror_root.join("notes.mdx");
        std::fs::write(&sibling_mdx, "# sibling notes\n").unwrap();

        let hook: ExternalInvalidationHook =
            Arc::new(|_path: &Path| Some(vec![pid("/proj/pages/a.tsx")]));
        let orch = BuildOrchestrator::new(
            OrchestratorConfig::new(
                &project,
                vec![PathBuf::from("pages"), PathBuf::from("content")],
            )
            .with_policy(policy)
            .with_external_invalidation(hook),
            make_graph(),
            CountingPipeline::default(),
        );

        let plan = orch.plan_for_changes([sibling_mdx]);
        assert!(
            plan.rerun_css,
            "a hook may narrow the page set, but the mirror-root CSS rescan is not \
             the hook's to drop"
        );
        assert!(!plan.pages.is_all(), "fixture sanity: the hook did narrow");
    }

    /// Issue #1711 (Sibling Invalidation epic #1709, confirm pass) — the
    /// EXTERNAL-INVALIDATION gate. When an `external_invalidation` hook
    /// narrows an out-of-root path to a specific page set (issue #1038),
    /// `plan_for_changes` re-applies the asset-flag side effects (CSS /
    /// islands / client-scripts) additively rather than letting the hook's
    /// narrowing suppress them — `is_client_script_sibling_target` was added
    /// to that additive re-apply in #1710 alongside raw/worker. Mirrors
    /// `external_hook_narrowing_css_still_reruns_css` /
    /// `external_hook_narrowing_islands_module_still_reruns_islands`.
    #[test]
    fn external_hook_narrowing_client_script_sibling_still_reruns_client_scripts() {
        let sibling = PathBuf::from("/srv/shared/lib/plain.ts");
        let invalidation = crate::policy::RawImportInvalidation::default();
        invalidation.replace_client_script_siblings([sibling.clone()]);
        let hook: ExternalInvalidationHook =
            Arc::new(|_path: &Path| Some(vec![pid("/proj/pages/a.tsx")]));
        let orch = BuildOrchestrator::new(
            OrchestratorConfig::new(
                "/proj",
                vec![PathBuf::from("pages"), PathBuf::from("content")],
            )
            .with_policy(
                crate::policy::GranularityPolicy::default()
                    .with_raw_import_invalidation(invalidation),
            )
            .with_external_invalidation(hook),
            make_graph(),
            CountingPipeline::default(),
        );

        let plan = orch.plan_for_changes(vec![sibling]);
        match &plan.pages {
            PageSelection::Specific(s) => {
                assert_eq!(*s, BTreeSet::from([pid("/proj/pages/a.tsx")]));
            }
            other => unreachable!("expected PageSelection::Specific, got {other:?}"),
        }
        assert!(
            plan.rerun_client_scripts,
            "narrowing an external sibling plain-module path must still rerun client scripts"
        );
        assert!(!plan.rerun_islands);
        assert!(plan.ssr_reload_needed);
    }

    /// Condition-keyed replacement for a fixed `sleep(100ms)` + blind-drain
    /// pre-wait (issue #1835): proves a dynamically-registered watch over
    /// `sentinel_dir` is genuinely live by writing fresh-named sentinel
    /// files into it until one is observed on `rx`, draining every change
    /// seen along the way (including boot-time notify noise) so the
    /// caller's subsequent doubted write lands on a settled channel.
    ///
    /// `signal_seen` only counts a change as proof-of-life when its path is
    /// one of THIS call's own freshly-written sentinels (tracked in
    /// `written`) — a `starts_with(sentinel_dir)` predicate could be
    /// satisfied by unrelated queued traffic and would not be
    /// condition-keyed to this specific attempt.
    ///
    /// Mirrors `zfb-watcher/tests/recursive_dirs.rs`'s `sentinel_round_trip`;
    /// that helper can't be reused directly because `zfb-test-utils` (which
    /// owns the underlying `watcher_live_handshake` primitive) must not
    /// depend on `zfb-watcher` — see that crate's module docs — so this
    /// crate keeps its own thin wrapper.
    async fn settle_watch_with_sentinels(
        rx: &mut tokio::sync::mpsc::Receiver<Change>,
        sentinel_dir: &Path,
        label: &str,
    ) {
        let written: std::rc::Rc<std::cell::RefCell<Vec<PathBuf>>> =
            std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let written_writer = std::rc::Rc::clone(&written);
        let dir = sentinel_dir.to_path_buf();
        let lbl = label.to_string();

        let res = zfb_test_utils::watcher_live_handshake(
            zfb_test_utils::HandshakeOpts::new(Duration::from_secs(10)),
            move |idx| {
                let path = dir.join(format!("sentinel-{lbl}-{idx}.txt"));
                std::fs::write(&path, b"sentinel").expect("write sentinel file");
                written_writer.borrow_mut().push(path);
            },
            move || loop {
                match rx.try_recv() {
                    Ok(change) => {
                        if written.borrow().contains(&change.path) {
                            return true;
                        }
                    }
                    Err(_) => return false,
                }
            },
        )
        .await;

        assert!(
            res.live,
            "sentinel handshake under {sentinel_dir:?} ({label}) never observed one of its \
             own sentinels within {:?} ({} markers written) — the watch never came up live",
            res.elapsed, res.markers_written,
        );
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
        register_dynamic_dependency_watches(&mut watcher, &policy, &[]);

        // `lib/` is deliberately absent from the recursive boot roots. The
        // client worker registry must add its parent as a dynamic watch.
        // Prove that watch is genuinely live with a sentinel handshake
        // instead of a fixed settle sleep (see `settle_watch_with_sentinels`).
        settle_watch_with_sentinels(&mut rx, helper.parent().unwrap(), "client-worker").await;
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn client_raw_dependency_outside_boot_roots_survives_edit_delete_and_recreate() {
        use std::time::Duration;

        async fn next_kind_for(
            rx: &mut tokio::sync::mpsc::Receiver<zfb_watcher::Change>,
            target: &Path,
        ) -> Option<ChangeKind> {
            tokio::time::timeout(Duration::from_secs(3), async {
                while let Some(change) = rx.recv().await {
                    if change.path == target {
                        return Some(change.kind);
                    }
                }
                None
            })
            .await
            .ok()
            .flatten()
        }

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("pages")).unwrap();
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let payload = root.join("lib/client-payload.txt");
        std::fs::write(&payload, "generation one\n").unwrap();

        let invalidation = crate::policy::RawImportInvalidation::default();
        invalidation.replace_client_scripts([payload.clone()]);
        let policy =
            crate::policy::GranularityPolicy::default().with_raw_import_invalidation(invalidation);
        assert!(policy.dynamic_dependency_paths().contains(&payload));
        assert!(policy.is_client_script_raw_target(&payload));
        assert!(!policy.is_client_script_worker_target(&payload));
        assert!(!policy.is_islands_dependency(&payload));

        let (mut watcher, mut rx) = Watcher::start_with_debounce(
            &root,
            std::iter::once("pages"),
            Duration::from_millis(50),
        )
        .unwrap();
        register_dynamic_dependency_watches(&mut watcher, &policy, &[]);

        // `lib/` is deliberately absent from the recursive boot roots. The
        // client raw snapshot must register its parent dynamically and keep
        // that parent alive while the terminal file is missing. Prove that
        // watch is genuinely live with a sentinel handshake instead of a
        // fixed settle sleep (see `settle_watch_with_sentinels`).
        settle_watch_with_sentinels(&mut rx, payload.parent().unwrap(), "client-raw").await;

        std::fs::write(&payload, "generation two\n").unwrap();
        assert!(
            matches!(
                next_kind_for(&mut rx, &payload).await,
                Some(ChangeKind::Created | ChangeKind::Modified)
            ),
            "outside-root client raw edit must reach the watcher"
        );

        std::fs::remove_file(&payload).unwrap();
        assert_eq!(
            next_kind_for(&mut rx, &payload).await,
            Some(ChangeKind::Removed),
            "watching the raw target parent must preserve delete visibility"
        );

        std::fs::write(&payload, "generation three\n").unwrap();
        assert!(
            matches!(
                next_kind_for(&mut rx, &payload).await,
                Some(ChangeKind::Created | ChangeKind::Modified)
            ),
            "watching the raw target parent must preserve recreate recovery"
        );
        watcher.shutdown().await;
    }

    /// Issue #1802 (epic #1799, gap (a)): `register_dynamic_dependency_watches`
    /// reconciles the policy's CSS sibling-mirror-root set through
    /// `Watcher::sync_recursive_dir_watches` every tick. The first call over
    /// a root outside the boot recursive roots must register it (the
    /// `watch-extra registered:` signal, returned here directly rather than
    /// scraped from `ZFB_DEV_TIMING` output) AND that registration must be
    /// genuinely live, not just recorded. A second call against the
    /// UNCHANGED policy must be a no-op: no re-emitted signal for a root
    /// already known.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn css_mirror_root_reconciliation_is_idempotent_and_watches_new_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("pages")).unwrap();
        let sibling = root.join("sibling");
        std::fs::create_dir_all(&sibling).unwrap();

        let invalidation = crate::policy::RawImportInvalidation::default();
        invalidation.replace_css_mirror_roots([sibling.clone()]);
        let policy =
            crate::policy::GranularityPolicy::default().with_raw_import_invalidation(invalidation);
        assert!(policy.css_mirror_root_paths().contains(&sibling));

        let (mut watcher, mut rx) = Watcher::start_with_debounce(
            &root,
            std::iter::once("pages"),
            Duration::from_millis(50),
        )
        .unwrap();

        // `sibling/` is deliberately absent from the recursive boot roots
        // (only `pages/` is watched at boot). The first reconciliation must
        // report it as newly watched.
        let first = register_dynamic_dependency_watches(&mut watcher, &policy, &[]);
        assert!(
            first.contains(&sibling),
            "the first reconciliation must report the mirror root as newly watched: {first:?}"
        );

        // Prove it's genuinely live, not just recorded in a registration set
        // — a sentinel handshake instead of a fixed settle sleep (see
        // `settle_watch_with_sentinels`).
        settle_watch_with_sentinels(&mut rx, &sibling, "css-mirror-root").await;
        let marker = sibling.join("marker.txt");
        std::fs::write(&marker, b"one").unwrap();
        let observed = tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(change) = rx.recv().await {
                // Exact-path filter: the sentinels above also live under
                // `sibling/`, so a `starts_with(&sibling)` predicate could
                // let a straggler sentinel satisfy this wait instead of the
                // real marker write.
                if change.path == marker {
                    return Some(change.kind);
                }
            }
            None
        })
        .await
        .expect("a write under the newly-registered mirror root must reach the watcher");
        assert!(
            matches!(observed, Some(ChangeKind::Created | ChangeKind::Modified)),
            "expected a Created/Modified event under the mirror root, got {observed:?}"
        );

        // A second reconciliation against the SAME unchanged policy must not
        // re-report the root — it is already known/watched.
        let second = register_dynamic_dependency_watches(&mut watcher, &policy, &[]);
        assert!(
            second.is_empty(),
            "reconciling an unchanged mirror-root set must not re-emit the \
             `watch-extra registered:` signal: {second:?}"
        );

        watcher.shutdown().await;
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

    // Issue #2100 (Dev Supervision epic #2099 Sub #2100): Level-1 unit
    // coverage for the two fault-injection knobs' pure parse/decision
    // functions, per zudo-test-wisdom — these don't need a booted dev
    // server to exercise the parse/decision logic.

    #[test]
    fn orch_panic_on_tick_armed_unset_is_not_armed() {
        assert!(!orch_panic_on_tick_armed(None));
    }

    #[test]
    fn orch_panic_on_tick_armed_set_invalid_falsy_values_are_not_armed() {
        assert!(!orch_panic_on_tick_armed(Some("")));
        assert!(!orch_panic_on_tick_armed(Some("0")));
        assert!(!orch_panic_on_tick_armed(Some("false")));
        assert!(!orch_panic_on_tick_armed(Some("FALSE")));
        assert!(!orch_panic_on_tick_armed(Some("  ")));
    }

    #[test]
    fn orch_panic_on_tick_armed_set_valid_is_armed() {
        assert!(orch_panic_on_tick_armed(Some("1")));
        assert!(orch_panic_on_tick_armed(Some("true")));
        assert!(orch_panic_on_tick_armed(Some("yes")));
    }

    #[test]
    fn orch_stop_ms_decision_unset_is_none() {
        assert_eq!(orch_stop_ms_decision(None), None);
    }

    #[test]
    fn orch_stop_ms_decision_set_invalid_is_none() {
        assert_eq!(orch_stop_ms_decision(Some("")), None);
        assert_eq!(orch_stop_ms_decision(Some("  ")), None);
        assert_eq!(orch_stop_ms_decision(Some("not-a-number")), None);
        assert_eq!(orch_stop_ms_decision(Some("-5")), None);
        assert_eq!(orch_stop_ms_decision(Some("12.5")), None);
    }

    #[test]
    fn orch_stop_ms_decision_set_valid_parses() {
        assert_eq!(orch_stop_ms_decision(Some("250")), Some(250));
        assert_eq!(orch_stop_ms_decision(Some(" 250 ")), Some(250));
    }

    /// Boundary value: `STOP_MS=0` is a VALID, armed value (fires on the
    /// very next drain-loop poll after the boot hook completes) — not
    /// treated the same as unset.
    #[test]
    fn orch_stop_ms_decision_boundary_zero_is_armed() {
        assert_eq!(orch_stop_ms_decision(Some("0")), Some(0));
    }

    /// `recv_with_stop_deadline` with no deadline is a plain passthrough to
    /// `rx.recv()` — proves the knob is provably inert when unset, without
    /// needing to boot a real orchestrator/watcher.
    #[tokio::test]
    async fn recv_with_stop_deadline_none_is_plain_passthrough() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Change>(1);
        tx.send(Change {
            path: PathBuf::from("/proj/pages/a.tsx"),
            kind: ChangeKind::Modified,
        })
        .await
        .expect("send");
        let got = recv_with_stop_deadline(&mut rx, None).await;
        assert_eq!(
            got.map(|c| c.path),
            Some(PathBuf::from("/proj/pages/a.tsx"))
        );
    }

    /// The `STOP_MS=0` boundary: an elapsed (already-past) deadline wins
    /// over the channel and returns `None` — the silent channel-close shape
    /// — even though the channel is never closed and never receives an
    /// item.
    #[tokio::test]
    async fn recv_with_stop_deadline_elapsed_deadline_returns_none() {
        let (_tx, mut rx) = tokio::sync::mpsc::channel::<Change>(1);
        let deadline = tokio::time::Instant::now();
        tokio::time::sleep(Duration::from_millis(5)).await;
        let got = recv_with_stop_deadline(&mut rx, Some(deadline)).await;
        assert!(got.is_none(), "an elapsed deadline must return None");
    }

    // ── Pre-tick plugin-refresh hook (issue #2169, epic #2166) ──────────

    /// Policy whose plugin watch-file registry contains exactly `paths`.
    fn policy_with_plugin_watch_files(
        paths: impl IntoIterator<Item = PathBuf>,
    ) -> crate::policy::GranularityPolicy {
        let invalidation = crate::policy::RawImportInvalidation::default();
        invalidation.replace_plugin_watch_files(paths);
        crate::policy::GranularityPolicy::default().with_raw_import_invalidation(invalidation)
    }

    #[test]
    fn pre_tick_refresh_applies_only_for_registered_plugin_watch_paths() {
        let watched = PathBuf::from("/ext/data/watched.json");
        let policy = policy_with_plugin_watch_files([watched.clone()]);
        let unrelated = PathBuf::from("/proj/pages/index.tsx");

        // Kind-agnostic: a Removed watched path still gates the refresh in
        // (the failing loader re-invocation + pending-retry queueing is the
        // refresh fn's own delete→recreate recovery contract).
        for kind in [
            ChangeKind::Created,
            ChangeKind::Modified,
            ChangeKind::Removed,
        ] {
            assert!(
                pre_tick_refresh_applies(&policy, &[(watched.clone(), kind)]),
                "a watched path must gate the refresh in regardless of kind ({kind:?})"
            );
        }
        // A mixed batch with one watched member applies.
        assert!(pre_tick_refresh_applies(
            &policy,
            &[
                (unrelated.clone(), ChangeKind::Modified),
                (watched.clone(), ChangeKind::Modified),
            ]
        ));
        // No watched member — skip.
        assert!(!pre_tick_refresh_applies(
            &policy,
            &[(unrelated.clone(), ChangeKind::Modified)]
        ));
        // Empty registry (pluginless project, or no loader declared
        // `watchFiles`) — never applies.
        let empty_policy = crate::policy::GranularityPolicy::default();
        assert!(!pre_tick_refresh_applies(
            &empty_policy,
            &[(watched, ChangeKind::Modified)]
        ));
    }

    /// A plugin watch-file change must conservatively rerun every
    /// virtual-module consumer (islands, client scripts, CSS scan, SSR
    /// reload) regardless of the path's class — the pre-tick refresh only
    /// updates the STORE; these flags are what ship the refreshed source
    /// (codex review, #2169). Page selection is widened to
    /// `PageSelection::All` for the same reason (#2181) — see
    /// `plugin_watch_file_change_invalidates_prerendered_pages` for the
    /// in-root `Unclassified` shape that made it necessary.
    #[test]
    fn plugin_watch_file_change_reruns_all_virtual_module_consumers() {
        let watched_external = PathBuf::from("/ext/data/watched.json");
        let watched_asset = PathBuf::from("/proj/public/blob.bin");
        let orch = BuildOrchestrator::new(
            OrchestratorConfig::new(
                "/proj",
                vec![PathBuf::from("pages"), PathBuf::from("content")],
            )
            .with_policy(policy_with_plugin_watch_files([
                watched_external.clone(),
                watched_asset.clone(),
            ])),
            make_graph(),
            CountingPipeline::default(),
        );

        // Out-of-root watch file: External classification — All pages +
        // SSR reload as before, PLUS all three consumer flags.
        let plan = orch.plan_for_changes(vec![watched_external]);
        assert!(plan.pages.is_all(), "External page selection must stay All");
        assert!(plan.ssr_reload_needed);
        assert!(plan.rerun_islands, "islands consume virtual-module source");
        assert!(
            plan.rerun_client_scripts,
            "client scripts consume virtual-module source"
        );
        assert!(
            plan.rerun_css,
            "the CSS scan consumes virtual-module source"
        );

        // In-root `public/**` watch file: Asset classification alone would
        // be a NO-OP plan — the refreshed store would never ship.
        let plan = orch.plan_for_changes(vec![watched_asset]);
        assert!(
            !plan.is_noop(),
            "an Asset-classified watch file must still produce a rebuilding tick"
        );
        assert!(plan.rerun_islands && plan.rerun_client_scripts && plan.rerun_css);
        assert!(plan.ssr_reload_needed);

        // Control: a NON-watched asset stays a no-op (the flags come from
        // watch-set membership, not from a loosened Asset arm).
        let plan = orch.plan_for_changes(vec![PathBuf::from("/proj/public/other.bin")]);
        assert!(plan.is_noop());
    }

    /// Issue #2181 — the in-root `Unclassified` shape that the #2170
    /// confirm e2e traced and deliberately routed around by making its
    /// fixture page SSR-only.
    ///
    /// A watch file under a project-root child directory that is not one of
    /// the recognized top segments (`pages`/`content`/`styles`/`data`/
    /// `public`/`components`/`layouts`/`lib`/`src`) and whose extension is
    /// not on the sniffing whitelist classifies as `PathClass::Unclassified`
    /// — NOT `External`, which is reserved for paths that fail
    /// `strip_prefix(project_root)`. That arm consults `graph.dirty_pages`
    /// only, and the loader reads the file through `node:fs`, so no
    /// dependency edge exists to consult: page selection came out EMPTY and
    /// a prerendered consumer page kept re-serving its stale committed HTML
    /// indefinitely. The plugin-watch block must therefore widen to
    /// `PageSelection::All` itself rather than relying on the class arms.
    #[test]
    fn plugin_watch_file_change_invalidates_prerendered_pages() {
        let watched = PathBuf::from("/proj/plugin-watched/note.txt");
        let orch = BuildOrchestrator::new(
            OrchestratorConfig::new(
                "/proj",
                vec![PathBuf::from("pages"), PathBuf::from("content")],
            )
            .with_policy(policy_with_plugin_watch_files([watched.clone()])),
            make_graph(),
            CountingPipeline::default(),
        );

        // Precondition: the graph knows nothing about this path, so the
        // `Unclassified` arm's `dirty_pages` contributes nothing. Asserted
        // directly so a future graph change can never make this test pass
        // for the wrong reason (an edge silently supplying the pages).
        assert_eq!(
            orch.graph.lock().unwrap().dirty_pages(&watched).as_pages(),
            Some(Vec::new()),
            "the fixture shape requires an edge-less watch file — the loader reads it via \
             `node:fs`, which no dependency edge can record"
        );

        let plan = orch.plan_for_changes(vec![watched]);
        assert!(
            plan.pages.is_all(),
            "an in-root Unclassified plugin watch file must invalidate every page — a \
             prerendered consumer of the refreshed virtual module has no other route to \
             staleness (issue #2181)"
        );
        assert!(plan.rerun_islands && plan.rerun_client_scripts && plan.rerun_css);
        assert!(plan.ssr_reload_needed);

        // Control: an unwatched sibling under the SAME unrecognized
        // directory stays a no-op with no pages at all — the widening comes
        // from watch-set membership, not from a loosened `Unclassified` arm.
        let plan = orch.plan_for_changes(vec![PathBuf::from("/proj/plugin-watched/other.txt")]);
        assert!(
            plan.is_noop(),
            "an unwatched in-root .txt must stay a no-op"
        );
        assert!(!plan.pages.is_all());
    }

    /// Issue #2181 (codex review of the #2181 fix) — the
    /// `external_invalidation` override branch (#1038) `continue`s before
    /// the classified-path fold, so an OUT-OF-ROOT watch file the hook
    /// returned a verdict for used to skip plugin-watch invalidation
    /// entirely: neither the consumer flags nor the page widening fired,
    /// and prerendered virtual-module consumers outside the hook's verdict
    /// stayed stale.
    ///
    /// The hook narrows page SELECTION only — never the asset-rebuild
    /// flags, the rule that branch's other `mark_*` re-applications already
    /// encode — and for a watch target it cannot inform page selection
    /// either: the virtual-module edge it would need does not exist in the
    /// graph (that is the premise of this whole issue). So the verdict is
    /// superseded by `All` here rather than honoured.
    ///
    /// Latent rather than live today: `grep -rn with_external_invalidation
    /// crates/zfb/src` finds no call site — the hook is an opt-in embedding
    /// API, so only an embedding consumer can reach this path.
    #[test]
    fn plugin_watch_target_invalidation_survives_external_hook_narrowing() {
        let watched = PathBuf::from("/srv/plugin-watched/note.txt");
        let hook: ExternalInvalidationHook =
            Arc::new(|_path: &Path| Some(vec![pid("/proj/pages/a.tsx")]));
        let orch = BuildOrchestrator::new(
            OrchestratorConfig::new(
                "/proj",
                vec![PathBuf::from("pages"), PathBuf::from("content")],
            )
            .with_policy(policy_with_plugin_watch_files([watched.clone()]))
            .with_external_invalidation(hook),
            make_graph(),
            CountingPipeline::default(),
        );

        let plan = orch.plan_for_changes(vec![watched]);
        assert!(
            plan.pages.is_all(),
            "a narrowing verdict must not shrink a plugin watch target's page set — the hook \
             cannot see virtual-module consumers, so honouring it would strand prerendered \
             pages outside the verdict"
        );
        assert!(plan.rerun_islands && plan.rerun_client_scripts && plan.rerun_css);
        assert!(plan.ssr_reload_needed);

        // Control: an out-of-root NON-watched path with the same hook still
        // narrows exactly as #1038 specifies — the override branch was not
        // broken wholesale into an always-All fallback.
        let plan = orch.plan_for_changes(vec![PathBuf::from("/srv/shared/styles/theme.css")]);
        match &plan.pages {
            PageSelection::Specific(s) => {
                assert_eq!(*s, BTreeSet::from([pid("/proj/pages/a.tsx")]));
            }
            other => unreachable!("expected the hook's narrowed verdict, got {other:?}"),
        }
    }

    /// Issue #2181 (P2) — a `Removed` watch file gets the same treatment.
    ///
    /// `tick_with_kinds` excludes every `Removed` path from
    /// `plan_for_changes` (the All-fallback would be wrong for a deletion),
    /// so the live-edit plugin-watch block is unreachable for a removal and
    /// the removed-path fold must mirror it. This matters because the
    /// pre-tick refresh gate is KIND-AGNOSTIC: the loader is re-invoked for
    /// a deleted watch file too, and one that intentionally handles a
    /// missing optional file publishes fallback source into the shared
    /// store — which nothing would rebuild or ship without these flags.
    ///
    /// Asserted through `CountingPipeline`'s recorded plan (the removal
    /// plan is built inside `tick_with_kinds` and never returned). Pages
    /// arrive already resolved by `resolve_all`, so the observable form of
    /// `All` is "every page the graph knows".
    #[test]
    fn removed_plugin_watch_file_reruns_consumers_and_invalidates_pages() {
        let watched = PathBuf::from("/proj/plugin-watched/note.txt");
        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        let orch = BuildOrchestrator::new(
            OrchestratorConfig::new(
                "/proj",
                vec![PathBuf::from("pages"), PathBuf::from("content")],
            )
            .with_policy(policy_with_plugin_watch_files([watched.clone()])),
            make_graph(),
            pipeline,
        );
        let dist = tempfile::tempdir().unwrap();

        orch.tick_with_kinds(
            vec![(watched, ChangeKind::Removed)],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();

        let plans = applies.lock().unwrap();
        let plan = plans
            .last()
            .expect("a removed plugin watch file must produce a non-noop tick, not be skipped");
        assert!(plan.rerun_islands, "islands consume virtual-module source");
        assert!(
            plan.rerun_client_scripts,
            "client scripts consume virtual-module source"
        );
        assert!(
            plan.rerun_css,
            "the CSS scan consumes virtual-module source"
        );
        assert!(plan.ssr_reload_needed);
        match &plan.pages {
            PageSelection::Specific(pages) => {
                let expected: std::collections::BTreeSet<PageId> = [
                    pid("/proj/pages/a.tsx"),
                    pid("/proj/pages/b.tsx"),
                    pid("/proj/pages/c.tsx"),
                ]
                .into_iter()
                .collect();
                assert_eq!(
                    *pages, expected,
                    "a removed watch file must invalidate every page (the resolved form of \
                     PageSelection::All) — `removed_consumers` is empty for an edge-less \
                     watch file, so nothing else would mark a prerendered consumer stale"
                );
            }
            PageSelection::All => unreachable!("resolve_all runs before the pipeline apply"),
        }
    }

    /// The plugin watch set is STATIC: populated once at boot, and — unlike
    /// `known_content`, which `tick_with_kinds`'s #1581 fold purges on every
    /// `Removed` — never ejected by a removal tick. Asserting this here pins
    /// the assumption the pre-tick gate relies on: after a watched file is
    /// deleted, the recreate event must still gate the refresh in (that
    /// recovery is what `plugin_refresh.rs`'s forced-failure tests exercise
    /// downstream of the gate).
    #[test]
    fn plugin_watch_set_is_static_across_removed_ticks() {
        let watched = PathBuf::from("/proj/data/watched.json");
        let pipeline = CountingPipeline::default();
        let orch = BuildOrchestrator::new(
            OrchestratorConfig::new(
                "/proj",
                vec![PathBuf::from("pages"), PathBuf::from("content")],
            )
            .with_policy(policy_with_plugin_watch_files([watched.clone()])),
            make_graph(),
            pipeline,
        );
        let dist = tempfile::tempdir().unwrap();

        orch.tick_with_kinds(
            vec![(watched.clone(), ChangeKind::Removed)],
            &noop_ctx(dist.path()),
            None,
        )
        .unwrap();

        assert!(
            orch.policy().is_plugin_watch_target(&watched),
            "a Removed tick must not purge the plugin watch set — it is boot-populated \
             and static (the #1581 Removed fold owns known_content, a different registry)"
        );
        assert!(
            pre_tick_refresh_applies(orch.policy(), &[(watched.clone(), ChangeKind::Created)]),
            "the delete→recreate event must still gate the pre-tick refresh in"
        );
    }

    #[tokio::test]
    async fn maybe_pre_tick_refresh_awaits_hook_to_completion_with_the_full_batch() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let watched = PathBuf::from("/proj/data/watched.json");
        let unrelated = PathBuf::from("/proj/pages/index.tsx");
        let completed = Arc::new(AtomicBool::new(false));
        let seen: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
        let hook: PreTickRefreshHook = {
            let completed = Arc::clone(&completed);
            let seen = Arc::clone(&seen);
            Arc::new(move |paths: Vec<PathBuf>| {
                let completed = Arc::clone(&completed);
                let seen = Arc::clone(&seen);
                Box::pin(async move {
                    // Suspend a few times so returning from the helper
                    // genuinely requires polling this future to the end —
                    // an unawaited (fire-and-forget) call would leave
                    // `completed` false.
                    for _ in 0..3 {
                        tokio::task::yield_now().await;
                    }
                    seen.lock().unwrap().extend(paths);
                    completed.store(true, Ordering::SeqCst);
                    Ok(())
                })
            })
        };
        let config = OrchestratorConfig::new("/proj", vec![PathBuf::from("pages")])
            .with_policy(policy_with_plugin_watch_files([watched.clone()]))
            .with_pre_tick_refresh(hook);

        maybe_pre_tick_refresh(
            &config,
            &[
                (unrelated.clone(), ChangeKind::Modified),
                (watched.clone(), ChangeKind::Modified),
            ],
        )
        .await;

        assert!(
            completed.load(Ordering::SeqCst),
            "the helper must await the hook future to completion before returning"
        );
        assert_eq!(
            *seen.lock().unwrap(),
            vec![unrelated, watched],
            "the hook receives the batch's FULL path set, in batch order — ownership \
             resolution is the refresh fn's job, not the gate's"
        );
    }

    #[tokio::test]
    async fn maybe_pre_tick_refresh_skips_non_matching_batches_and_absent_hook() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let watched = PathBuf::from("/proj/data/watched.json");
        let unrelated = PathBuf::from("/proj/pages/index.tsx");
        let calls = Arc::new(AtomicUsize::new(0));
        let hook: PreTickRefreshHook = {
            let calls = Arc::clone(&calls);
            Arc::new(move |_paths: Vec<PathBuf>| {
                let calls = Arc::clone(&calls);
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
        };
        let config = OrchestratorConfig::new("/proj", vec![PathBuf::from("pages")])
            .with_policy(policy_with_plugin_watch_files([watched.clone()]))
            .with_pre_tick_refresh(hook);

        maybe_pre_tick_refresh(&config, &[(unrelated.clone(), ChangeKind::Modified)]).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "a batch with no watched member must not invoke the hook"
        );

        maybe_pre_tick_refresh(&config, &[(watched.clone(), ChangeKind::Modified)]).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // No hook configured (the default) — a watched batch is a no-op.
        let hookless = OrchestratorConfig::new("/proj", vec![PathBuf::from("pages")])
            .with_policy(policy_with_plugin_watch_files([watched.clone()]));
        maybe_pre_tick_refresh(&hookless, &[(watched, ChangeKind::Modified)]).await;
    }

    #[tokio::test]
    async fn maybe_pre_tick_refresh_swallows_hook_errors() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let watched = PathBuf::from("/proj/data/watched.json");
        let calls = Arc::new(AtomicUsize::new(0));
        let hook: PreTickRefreshHook = {
            let calls = Arc::clone(&calls);
            Arc::new(move |_paths: Vec<PathBuf>| {
                let calls = Arc::clone(&calls);
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Err(anyhow::anyhow!("synthetic refresh failure"))
                })
            })
        };
        let config = OrchestratorConfig::new("/proj", vec![PathBuf::from("pages")])
            .with_policy(policy_with_plugin_watch_files([watched.clone()]))
            .with_pre_tick_refresh(hook);

        // Returning normally IS the assertion — an `Err` must be logged and
        // swallowed, never propagated (the loop would otherwise die and the
        // dev server would stop rebuilding).
        maybe_pre_tick_refresh(&config, &[(watched, ChangeKind::Modified)]).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// Records every pipeline apply as a `"tick"` entry in a shared,
    /// interleaved event log the pre-tick hook also writes to — the
    /// loop-level ordering evidence for the live-loop test below.
    #[derive(Debug, Clone)]
    struct LoggingPipeline {
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    impl AssetPipeline for LoggingPipeline {
        fn apply(&self, _plan: &RebuildPlan, _ctx: &BuildContext) -> Result<BuildOutcome> {
            self.log.lock().unwrap().push("tick");
            Ok(BuildOutcome::default())
        }
    }

    /// Poll `probe` every 10ms until it returns true or `secs` elapse.
    /// Condition-keyed (never a bare settle sleep): the caller's assertion
    /// message names what never became true.
    async fn wait_until(secs: u64, mut probe: impl FnMut() -> bool) -> bool {
        tokio::time::timeout(Duration::from_secs(secs), async {
            loop {
                if probe() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok()
    }

    /// Spawn [`BuildOrchestrator::run_drain_loop`] (private — accessible
    /// here because `tests` is a descendant module) wired to a
    /// test-owned, synthetic `Change` channel instead of a real
    /// `Watcher`'s own receiver (issue #2253). A real `Watcher` is still
    /// constructed from `orch`'s own config, mirroring exactly what
    /// `run_with_boot` does — `register_dynamic_dependency_watches` needs
    /// a genuine handle, though it is a no-op for the
    /// `policy_with_plugin_watch_files` policies these tests use. The
    /// watcher's own receiver is discarded; only the returned `Sender`
    /// feeds the loop, so event DELIVERY is synthetic while the fixture
    /// files a caller writes are still real disk state the tick reads.
    fn spawn_drain_loop<P: AssetPipeline + 'static>(
        orch: BuildOrchestrator<P>,
        ctx: BuildContext,
    ) -> (
        tokio::task::JoinHandle<Result<()>>,
        tokio::sync::mpsc::Sender<Change>,
    ) {
        let (watcher, _real_rx) = Watcher::start_with_options(
            &orch.config.project_root,
            orch.config.watch_roots.iter().map(|p| p.as_path()),
            orch.config.extra_watch_paths.iter().map(|p| p.as_path()),
            watch_options_for(&orch.config),
        )
        .expect("start test watcher");
        let (tx, rx) = tokio::sync::mpsc::channel::<Change>(16);
        let handle = tokio::spawn(orch.run_drain_loop(
            ctx,
            None,
            |_: &BuildOutcome| {},
            None::<fn(&BuildOrchestrator<P>, &BuildContext) -> Option<BuildOutcome>>,
            watcher,
            rx,
        ));
        (handle, tx)
    }

    /// The live-loop wiring proof for issue #2169: `run_with_boot`'s drain
    /// loop must AWAIT the pre-tick hook to completion before dispatching
    /// the tick. The unit tests above prove `maybe_pre_tick_refresh`'s own
    /// contract; without this test a helper nobody calls at the right seam
    /// would read as coverage (the #1058/#1581 dead-guard lesson).
    ///
    /// Determinism: the hook suspends on a zero-permit semaphore the TEST
    /// releases. While the hook is suspended the drain loop is suspended
    /// with it (`spawn_blocking` for the tick has not been reached), so a
    /// correctly-wired loop can NEVER log a `"tick"` inside a
    /// refresh-start→refresh-complete window — no timing margin involved.
    /// A fire-and-forget regression dispatches the tick while the gate is
    /// still closed, landing a `"tick"` inside the first window (or before
    /// the first `"refresh-start"`), and fails the scan below.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pre_tick_hook_gates_the_tick_dispatch_in_the_live_loop() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let project_root = root.join("proj");
        std::fs::create_dir_all(project_root.join("pages")).unwrap();
        let ext_dir = root.join("ext");
        std::fs::create_dir_all(&ext_dir).unwrap();
        let watched = ext_dir.join("watched.json");
        std::fs::write(&watched, "v-boot").unwrap();

        let log: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
        let seen: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));

        let hook: PreTickRefreshHook = {
            let log = Arc::clone(&log);
            let seen = Arc::clone(&seen);
            let gate = Arc::clone(&gate);
            Arc::new(move |paths: Vec<PathBuf>| {
                let log = Arc::clone(&log);
                let seen = Arc::clone(&seen);
                let gate = Arc::clone(&gate);
                Box::pin(async move {
                    log.lock().unwrap().push("refresh-start");
                    let _permit = gate.acquire().await.expect("gate never closed");
                    seen.lock().unwrap().extend(paths);
                    log.lock().unwrap().push("refresh-complete");
                    Ok(())
                })
            })
        };

        let orch = BuildOrchestrator::new(
            OrchestratorConfig::new(&project_root, vec![PathBuf::from("pages")])
                .with_policy(policy_with_plugin_watch_files([watched.clone()]))
                .with_debounce(Duration::from_millis(25))
                .with_pre_tick_refresh(hook),
            Arc::new(Mutex::new(DependencyGraph::new())),
            LoggingPipeline {
                log: Arc::clone(&log),
            },
        );
        let dist = tempfile::tempdir().unwrap();
        let (run, tx) = spawn_drain_loop(orch, noop_ctx(dist.path()));

        // Synthetic delivery (issue #2253): the fixture write is still
        // real disk state, but the "the watcher observed it" step is
        // replaced by a direct send on the test-owned channel — no
        // real-watcher round trip to wait out.
        std::fs::write(&watched, "v1").expect("write watched file");
        tx.send(Change {
            path: watched.clone(),
            kind: ChangeKind::Modified,
        })
        .await
        .expect("send watched-file change");
        let log_probe = Arc::clone(&log);
        let reached_hook = wait_until(10, move || {
            log_probe.lock().unwrap().contains(&"refresh-start")
        })
        .await;
        assert!(
            reached_hook,
            "the synthetic watched-file change never reached the pre-tick hook within 10s"
        );

        // Open the gate for this and every subsequent hook invocation, then
        // wait for a completed refresh followed by its tick.
        gate.add_permits(1_000_000);
        let log_probe = Arc::clone(&log);
        let done = wait_until(10, move || {
            let log = log_probe.lock().unwrap();
            let first_complete = log.iter().position(|entry| *entry == "refresh-complete");
            match first_complete {
                Some(idx) => log[idx..].contains(&"tick"),
                None => false,
            }
        })
        .await;
        assert!(
            done,
            "after opening the gate, a refresh-complete followed by its tick never appeared"
        );

        let snapshot = log.lock().unwrap().clone();
        // The load-bearing scan: no tick may land while a refresh window is
        // open, and — because the ONLY file ever written in this fixture is
        // the watched one, so every batch must contain it — no tick may
        // precede the first refresh-start either.
        let mut in_window = false;
        let mut seen_first_start = false;
        for entry in &snapshot {
            match *entry {
                "refresh-start" => {
                    in_window = true;
                    seen_first_start = true;
                }
                "refresh-complete" => in_window = false,
                "tick" => {
                    assert!(
                        !in_window,
                        "a tick landed while the pre-tick hook was still suspended — the \
                         loop is not awaiting the hook before dispatch: {snapshot:?}"
                    );
                    assert!(
                        seen_first_start,
                        "a tick landed before the first refresh-start, but every batch in \
                         this fixture contains the watched file: {snapshot:?}"
                    );
                }
                _ => {}
            }
        }
        assert!(
            seen.lock()
                .unwrap()
                .iter()
                .any(|path| path.ends_with("watched.json")),
            "the hook must receive the batch's changed paths"
        );

        run.abort();
        let _ = run.await;
    }

    /// An erroring hook must not kill the drain loop: ticks keep landing
    /// and the hook keeps being consulted on subsequent batches (the store
    /// side of "tick proceeds on last-good memo" is pinned by
    /// `plugin_refresh.rs`'s own forced-failure tests).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pre_tick_hook_error_does_not_kill_the_live_loop() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let project_root = root.join("proj");
        std::fs::create_dir_all(project_root.join("pages")).unwrap();
        let ext_dir = root.join("ext");
        std::fs::create_dir_all(&ext_dir).unwrap();
        let watched = ext_dir.join("watched.json");
        std::fs::write(&watched, "v-boot").unwrap();

        let hook_calls = Arc::new(AtomicUsize::new(0));
        let hook: PreTickRefreshHook = {
            let hook_calls = Arc::clone(&hook_calls);
            Arc::new(move |_paths: Vec<PathBuf>| {
                let hook_calls = Arc::clone(&hook_calls);
                Box::pin(async move {
                    hook_calls.fetch_add(1, Ordering::SeqCst);
                    Err(anyhow::anyhow!("synthetic pre-tick refresh failure"))
                })
            })
        };

        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        let orch = BuildOrchestrator::new(
            OrchestratorConfig::new(&project_root, vec![PathBuf::from("pages")])
                .with_policy(policy_with_plugin_watch_files([watched.clone()]))
                .with_debounce(Duration::from_millis(25))
                .with_pre_tick_refresh(hook),
            Arc::new(Mutex::new(DependencyGraph::new())),
            pipeline,
        );
        let dist = tempfile::tempdir().unwrap();
        let (run, tx) = spawn_drain_loop(orch, noop_ctx(dist.path()));

        // Two full error rounds, driven one at a time (issue #2253): each
        // synthetic send is awaited into its own drain-loop batch before
        // the next is sent, so a batch never coalesces two rounds into
        // one — hook_calls >= 2 proves the loop survived the first Err
        // and consulted the hook again; applies >= 2 proves the ticks
        // themselves kept dispatching.
        for idx in 1..=2u32 {
            std::fs::write(&watched, format!("v{idx}")).expect("write watched file");
            tx.send(Change {
                path: watched.clone(),
                kind: ChangeKind::Modified,
            })
            .await
            .expect("send watched-file change");
            let applies_probe = Arc::clone(&applies);
            let landed = wait_until(5, move || {
                applies_probe.lock().unwrap().len() >= idx as usize
            })
            .await;
            assert!(
                landed,
                "round {idx}: no tick landed after the (erroring) pre-tick hook ran"
            );
        }
        assert!(
            hook_calls.load(Ordering::SeqCst) >= 2 && applies.lock().unwrap().len() >= 2,
            "the loop never reached 2 hook calls + 2 ticks after an erroring hook — an Err is \
             killing the drain loop (hook_calls={}, applies={})",
            hook_calls.load(Ordering::SeqCst),
            applies.lock().unwrap().len(),
        );

        run.abort();
        let _ = run.await;
    }

    /// Batches with no plugin-watch member never consult the hook, even
    /// with a NON-EMPTY registry (an empty registry would make this pass
    /// trivially). Race-free by construction: the watched file is created
    /// before the watcher boots and never touched again, so no batch can
    /// ever legitimately contain it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pre_tick_hook_is_not_consulted_for_non_matching_batches_in_the_live_loop() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let project_root = root.join("proj");
        std::fs::create_dir_all(project_root.join("pages")).unwrap();
        let ext_dir = root.join("ext");
        std::fs::create_dir_all(&ext_dir).unwrap();
        let watched = ext_dir.join("watched.json");
        std::fs::write(&watched, "v-boot").unwrap();

        let hook_calls = Arc::new(AtomicUsize::new(0));
        let hook: PreTickRefreshHook = {
            let hook_calls = Arc::clone(&hook_calls);
            Arc::new(move |_paths: Vec<PathBuf>| {
                let hook_calls = Arc::clone(&hook_calls);
                Box::pin(async move {
                    hook_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
        };

        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        let orch = BuildOrchestrator::new(
            OrchestratorConfig::new(&project_root, vec![PathBuf::from("pages")])
                .with_policy(policy_with_plugin_watch_files([watched.clone()]))
                .with_debounce(Duration::from_millis(25))
                .with_pre_tick_refresh(hook),
            Arc::new(Mutex::new(DependencyGraph::new())),
            pipeline,
        );
        let dist = tempfile::tempdir().unwrap();
        let (run, tx) = spawn_drain_loop(orch, noop_ctx(dist.path()));

        // A synthetic page-file change (issue #2253) — a boot-watched
        // recursive root with no plugin-watch member, so the batch must
        // never consult the hook even with a NON-EMPTY registry (an
        // empty registry would make this pass trivially).
        let page_path = project_root.join("pages").join("probe-0.tsx");
        std::fs::write(&page_path, "export default () => null;\n").expect("write page file");
        tx.send(Change {
            path: page_path,
            kind: ChangeKind::Created,
        })
        .await
        .expect("send page-file change");

        let applies_probe = Arc::clone(&applies);
        let landed = wait_until(10, move || !applies_probe.lock().unwrap().is_empty()).await;
        assert!(
            landed,
            "no tick ever landed for the synthetic page-file batch within 10s"
        );

        assert_eq!(
            hook_calls.load(Ordering::SeqCst),
            0,
            "a batch with no plugin-watch member must never consult the pre-tick hook"
        );

        run.abort();
        let _ = run.await;
    }

    // -----------------------------------------------------------------
    // Watch-intake suppression (issue #2345)
    // -----------------------------------------------------------------

    /// A test stand-in for `zfb_css::is_tailwind_entry_tmp` — this crate
    /// must not depend on `zfb-css` (the knob is opaque by design), so the
    /// suppression tests carry their own shape-alike predicate as plain
    /// test data.
    fn temp_entry_suppression() -> IntakeSuppressionPredicate {
        Arc::new(|path: &Path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("zfb-tailwind-entry-") && n.ends_with(".css"))
        })
    }

    /// The over-suppression guard: a mixed batch carrying temp-entry
    /// events of ALL THREE normalized kinds alongside a real
    /// `styles/main.css` change must lose exactly the temp events — and
    /// the surviving change must still plan a CSS rerun.
    #[test]
    fn intake_suppression_mixed_batch_drops_temp_entries_and_keeps_the_real_css_change() {
        let orch = make_orch(CountingPipeline::default());
        let config = OrchestratorConfig::new(
            "/proj",
            vec![PathBuf::from("pages"), PathBuf::from("content")],
        )
        .with_intake_suppression(temp_entry_suppression());

        let main_css = PathBuf::from("/proj/styles/main.css");
        let batch = vec![
            (
                PathBuf::from("/proj/styles/zfb-tailwind-entry-a1B2c3.css"),
                ChangeKind::Created,
            ),
            (
                PathBuf::from("/proj/styles/zfb-tailwind-entry-a1B2c3.css"),
                ChangeKind::Modified,
            ),
            (
                PathBuf::from("/proj/styles/zfb-tailwind-entry-d4E5f6.css"),
                ChangeKind::Removed,
            ),
            (main_css.clone(), ChangeKind::Modified),
        ];

        let filtered = retain_unsuppressed_changes(&config, batch);
        assert_eq!(
            filtered,
            vec![(main_css.clone(), ChangeKind::Modified)],
            "every temp-entry event (Created/Modified/Removed) must be dropped; \
             the real CSS change must survive"
        );

        let plan = orch.plan_for_changes(filtered.into_iter().map(|(path, _)| path));
        assert!(
            plan.rerun_css,
            "the surviving styles/main.css change must still trigger the CSS rerun"
        );
    }

    /// A batch consisting ONLY of temp-entry events filters to empty —
    /// the drain loop's empty-batch skip is what turns this into "no tick
    /// outcome at all" (pinned at the loop level by
    /// `suppressed_only_batches_produce_no_tick_in_the_live_loop`).
    #[test]
    fn intake_suppression_all_temp_batch_filters_to_empty() {
        let config = OrchestratorConfig::new("/proj", vec![PathBuf::from("pages")])
            .with_intake_suppression(temp_entry_suppression());
        let filtered = retain_unsuppressed_changes(
            &config,
            vec![
                (
                    PathBuf::from("/proj/styles/zfb-tailwind-entry-a1B2c3.css"),
                    ChangeKind::Created,
                ),
                (
                    PathBuf::from("/proj/styles/zfb-tailwind-entry-a1B2c3.css"),
                    ChangeKind::Modified,
                ),
                (
                    PathBuf::from("/proj/styles/zfb-tailwind-entry-a1B2c3.css"),
                    ChangeKind::Removed,
                ),
            ],
        );
        assert!(
            filtered.is_empty(),
            "an all-temp batch must filter to empty; got {filtered:?}"
        );
    }

    /// Without a configured predicate the filter is the identity — the
    /// pre-#2345 behavior for every existing consumer.
    #[test]
    fn intake_suppression_absent_is_identity() {
        let config = OrchestratorConfig::new("/proj", vec![PathBuf::from("pages")]);
        let batch = vec![
            (
                PathBuf::from("/proj/styles/zfb-tailwind-entry-a1B2c3.css"),
                ChangeKind::Created,
            ),
            (PathBuf::from("/proj/styles/main.css"), ChangeKind::Modified),
        ];
        assert_eq!(retain_unsuppressed_changes(&config, batch.clone()), batch);
    }

    /// The live-loop wiring proof for issue #2345 (the #1058/#1581
    /// dead-guard lesson: a filter helper nobody calls at the right seam
    /// would read as coverage). Temp-entry events of all three kinds are
    /// sent first; a real `styles/main.css` change follows as the bound —
    /// the channel is FIFO and batches process in order, so by the time
    /// the real change's tick lands, every temp event has already been
    /// consumed. Exactly ONE tick total proves the suppressed-only
    /// batches produced no tick outcome at all (idle quiescence), in
    /// every possible coalescing of the sends into batches.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn suppressed_only_batches_produce_no_tick_in_the_live_loop() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let project_root = root.join("proj");
        std::fs::create_dir_all(project_root.join("pages")).unwrap();
        std::fs::create_dir_all(project_root.join("styles")).unwrap();
        let main_css = project_root.join("styles").join("main.css");
        std::fs::write(&main_css, "body { margin: 0; }\n").unwrap();
        let temp_entry = project_root
            .join("styles")
            .join("zfb-tailwind-entry-a1B2c3.css");
        std::fs::write(&temp_entry, "/* synthesised entry */\n").unwrap();

        let pipeline = CountingPipeline::default();
        let applies = pipeline.applies.clone();
        let orch = BuildOrchestrator::new(
            OrchestratorConfig::new(&project_root, vec![PathBuf::from("pages")])
                .with_debounce(Duration::from_millis(25))
                .with_intake_suppression(temp_entry_suppression()),
            Arc::new(Mutex::new(DependencyGraph::new())),
            pipeline,
        );
        let dist = tempfile::tempdir().unwrap();
        let (run, tx) = spawn_drain_loop(orch, noop_ctx(dist.path()));

        // Synthetic delivery (issue #2253): the CSS pass's own temp-entry
        // lifecycle as the watcher would report it.
        for kind in [
            ChangeKind::Created,
            ChangeKind::Modified,
            ChangeKind::Removed,
        ] {
            tx.send(Change {
                path: temp_entry.clone(),
                kind,
            })
            .await
            .expect("send temp-entry change");
        }
        tx.send(Change {
            path: main_css.clone(),
            kind: ChangeKind::Modified,
        })
        .await
        .expect("send real css change");

        let applies_probe = Arc::clone(&applies);
        let landed = wait_until(10, move || {
            applies_probe
                .lock()
                .unwrap()
                .iter()
                .any(|plan| plan.triggers.contains(&main_css))
        })
        .await;
        assert!(
            landed,
            "the real styles/main.css change never produced a tick within 10s"
        );

        let snapshot = applies.lock().unwrap().clone();
        assert_eq!(
            snapshot.len(),
            1,
            "the suppressed-only batches must produce NO tick — only the real \
             css change's single tick may land; got plans: {snapshot:?}"
        );
        assert!(
            snapshot[0].rerun_css,
            "the surviving real css change must still rerun CSS"
        );
        assert!(
            !snapshot[0].triggers.contains(&temp_entry),
            "a suppressed temp-entry path must never appear among tick triggers: {:?}",
            snapshot[0].triggers
        );

        run.abort();
        let _ = run.await;
    }
}
