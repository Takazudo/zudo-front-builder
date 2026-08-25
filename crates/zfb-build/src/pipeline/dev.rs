//! [`DevAssetPipeline`] — the watcher-driven incremental pipeline.
//!
//! Behaviour:
//!
//! - Calls `ctx.render_pages` for every selected page (or "all known
//!   pages" if the plan is `PageSelection::All` — but the orchestrator
//!   resolves "All" before handing the plan in, so this impl just
//!   handles `Specific`).
//! - Calls `ctx.run_css` if the plan asks for it.
//! - Calls `ctx.run_islands` if the plan asks for it.
//! - Writes each `RenderedPage.html` atomically under `ctx.dist_root`.
//! - Tracks bytes-changed-or-not so [`super::BuildOutcome::pages_written`]
//!   reflects only pages whose bytes actually moved (cheap reload signal
//!   for the dev server's WebSocket).
//! - Filenames stay **stable across rebuilds**. The dev server's URL
//!   contract does not change between watcher ticks; production-style
//!   content hashing is the production pipeline's job (see
//!   [`super::prod::ProductionAssetPipeline`]).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use zfb_graph::PageId;

use crate::atomic::{atomic_write, validate_output_path};

/// Parse the shared `ZFB_DEV_TIMING` gate used by the dev command and
/// orchestrator. Keeping this local avoids coupling the build crate to the
/// command crate while preserving the same truthy values everywhere.
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

fn format_tick_page_write_complete_line() -> &'static str {
    "[zfb-timing] tick: page write complete"
}

fn format_tick_islands_published_line() -> &'static str {
    "[zfb-timing] tick: islands published"
}

fn format_tick_client_scripts_published_line() -> &'static str {
    "[zfb-timing] tick: client scripts published"
}

use crate::pipeline::{
    AssetPipeline, BuildContext, BuildOutcome, DynamicInjectedProbe, RefreshOutcome,
    SsrPublishProbe, StaleProbe,
};
use crate::plan::{PageSelection, RebuildPlan};

/// One coherent Dev document/entry publication transaction boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentPublicationEvent {
    Begin,
    /// Register every currently stale document that still has an on-disk
    /// fallback before an ordinary publication commit decides retention.
    /// The pipeline emits this while holding the tick/request exclusion.
    SnapshotLazyFallbacks(Vec<PathBuf>),
    DocumentMutationStarted,
    DocumentWriteStarted(PathBuf),
    DocumentWriteCompleted(PathBuf),
    DocumentPathsRemoved(Vec<PathBuf>),
    CommitPublished,
    CommitReadyOnRequest,
    /// The final request-time repair drained a previously failed lazy
    /// document transaction. Commit readiness now, but retain fallback
    /// dependencies until a later ordinary publication boundary.
    CommitLazyRepair,
    CommitNotExpected,
    CommitEntriesOnly,
    AbortBeforeDocumentWrite,
    LeavePending,
}

/// Command-layer bridge for the server-owned publication state.
pub type DocumentPublicationHook = Arc<dyn Fn(DocumentPublicationEvent) + Send + Sync>;

/// Snapshot of the render session's complete stale map, already mapped to
/// exact absolute on-disk fallback document paths.
pub type LazyFallbackProbe = Arc<dyn Fn() -> Vec<PathBuf> + Send + Sync>;

/// Ensures every begun publication transaction reaches either its explicit
/// hydration boundary or the conservative failure phase on early return.
struct DocumentPublicationTransaction<'a> {
    hook: Option<&'a DocumentPublicationHook>,
    fallback: Option<DocumentPublicationEvent>,
}

impl<'a> DocumentPublicationTransaction<'a> {
    fn begin(hook: Option<&'a DocumentPublicationHook>, publication_relevant: bool) -> Self {
        if publication_relevant {
            if let Some(hook) = hook {
                hook(DocumentPublicationEvent::Begin);
            }
        }
        Self {
            hook,
            fallback: publication_relevant
                .then_some(DocumentPublicationEvent::AbortBeforeDocumentWrite),
        }
    }

    fn document_mutation_started(&mut self) {
        if !matches!(
            self.fallback,
            Some(DocumentPublicationEvent::AbortBeforeDocumentWrite)
        ) {
            return;
        }
        if let Some(hook) = self.hook {
            hook(DocumentPublicationEvent::DocumentMutationStarted);
        }
        self.fallback = Some(DocumentPublicationEvent::LeavePending);
    }

    fn document_write_started(&mut self, path: PathBuf) {
        self.document_mutation_started();
        if let Some(hook) = self.hook {
            hook(DocumentPublicationEvent::DocumentWriteStarted(path));
        }
    }

    fn document_write_completed(&self, path: PathBuf) {
        if let Some(hook) = self.hook {
            hook(DocumentPublicationEvent::DocumentWriteCompleted(path));
        }
    }

    fn document_paths_removed(&self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if let Some(hook) = self.hook {
            hook(DocumentPublicationEvent::DocumentPathsRemoved(paths));
        }
    }

    fn commit(&mut self, event: DocumentPublicationEvent) {
        self.fallback = None;
        if let Some(hook) = self.hook {
            hook(event);
        }
    }
}

impl Drop for DocumentPublicationTransaction<'_> {
    fn drop(&mut self) {
        if let (Some(hook), Some(event)) = (self.hook, self.fallback.take()) {
            hook(event);
        }
    }
}

// ── Internal write/dedup component ──────────────────────────────────────────

/// Result of a single [`WriteCache::write_page`] call.
///
/// Returned to the tick loop so it can update `BuildOutcome` and
/// populate `live_dests` / `prune_candidates` without reaching into the
/// cache internals.
struct WritePageOutcome {
    /// `true` when the page's bytes differed from the cached copy and
    /// `atomic_write` was called successfully.
    written: bool,
    /// The stale output-path for this page (from a previous tick that
    /// mapped the same page id to a different destination), if any.
    /// The caller collects these and runs the deferred-prune step after
    /// the write loop completes.
    stale_path: Option<PathBuf>,
}

/// Result of a guarded request-time write
/// ([`DevAssetPipeline::request_write_guarded`] /
/// [`RequestWriter::request_write_guarded`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardedWriteOutcome {
    /// Revalidation passed under the exclusion lock; the normal
    /// validate → byte-dedup → atomic-write → commit sequence ran.
    Written(RequestWriteOutcome),
    /// Revalidation failed — the write was skipped wholesale: no path
    /// validation, no dedup commit, disk untouched. Whatever the
    /// interleaving tick wrote stays authoritative.
    Skipped,
}

/// Result of a request-time write ([`DevAssetPipeline::request_write`] /
/// [`RequestWriter::request_write`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestWriteOutcome {
    /// `true` when the bytes differed from the dedup cache's copy and the
    /// file was (re-)written via `atomic_write`; `false` when the write
    /// was skipped as byte-identical.
    ///
    /// Informational only (logging). Request-time writes never feed
    /// [`super::BuildOutcome::pages_written`] and never emit SSE reload
    /// events — the browser that triggered the request is about to
    /// receive these very bytes as its response.
    pub written: bool,
}

/// Shared dedup/bookkeeping component that owns the per-tick-persistent
/// write state for [`DevAssetPipeline`].
///
/// Invariants enforced here (also validated by the existing regression
/// tests #727, #804, #956, #958):
///
/// - `last_bytes` is updated **only after** a successful `atomic_write`.
///   A transient I/O failure must not poison the cache — an identical
///   retry on the next tick must re-write.
/// - `last_output_path` is committed **only after** the write succeeds
///   (same reason). A failed write leaves the old path mapping intact so
///   the stale artifact remains prunable on the next tick.
#[derive(Debug, Default)]
struct WriteCache {
    // Last-known bytes per output path, used to skip writes when the
    // renderer is deterministic and re-emits the same HTML. Wrapped in
    // a Mutex so &self methods can mutate it; the lock is held briefly
    // (check then commit), not across the atomic_write I/O.
    last_bytes: Mutex<HashMap<PathBuf, Vec<u8>>>,

    // Last-known absolute output path per page id. Used to detect
    // stale-output transitions: when a page's `output_path` changes
    // between builds (typically because its frontmatter `extension`
    // flipped — e.g. `xml` → `rss`), the previous artifact is
    // deleted so dist/ doesn't accumulate orphaned files.
    //
    // Stored as the absolute joined path (`<dist_root>/<output_path>`)
    // so the prune step doesn't need access to the dist root.
    last_output_path: Mutex<HashMap<PageId, PathBuf>>,
}

impl WriteCache {
    /// Core write sequence for a single rendered page:
    /// dedup vs `last_bytes` → `atomic_write` (if changed) →
    /// commit `last_bytes` and `last_output_path` strictly after the
    /// write succeeds.
    ///
    /// `dest` must already be validated (via `validate_output_path`)
    /// by the caller.  The returned [`WritePageOutcome`] carries the
    /// `written` flag and any stale path the caller should queue for
    /// deferred pruning.
    fn write_page(
        &self,
        page: &PageId,
        dest: &PathBuf,
        new_bytes: Vec<u8>,
    ) -> Result<WritePageOutcome> {
        // Collect any stale-output prune candidate for this page.
        // The actual delete is deferred to after the write loop so we
        // can cross-check against live_dests first (issue #727).
        // We still defer updating `last_output_path` until the write
        // succeeds: a transient write failure aborts the tick (via `?`)
        // before the deferred prune runs, so the previous path mapping
        // is preserved for the next tick.
        let stale_path = {
            let last_out = self.last_output_path.lock().unwrap_or_else(|p| {
                tracing::warn!(
                    site = "WriteCache.last_output_path",
                    "mutex poisoned, recovering"
                );
                p.into_inner()
            });
            last_out.get(page).and_then(|prev| {
                if prev != dest {
                    Some(prev.clone())
                } else {
                    None
                }
            })
        };

        let changed = self.write_bytes(dest, new_bytes)?;

        // Only after a successful write do we record the new output
        // path. If the write above failed we abort the tick and the
        // previous mapping is preserved, so the stale file remains
        // prunable on the next rebuild.
        self.last_output_path
            .lock()
            .unwrap_or_else(|p| {
                tracing::warn!(
                    site = "WriteCache.last_output_path (commit)",
                    "mutex poisoned, recovering"
                );
                p.into_inner()
            })
            .insert(page.clone(), dest.clone());

        Ok(WritePageOutcome {
            written: changed,
            stale_path,
        })
    }

    /// Byte-dedup → `atomic_write` → commit `last_bytes`, for one dest.
    ///
    /// The shared core of the tick path ([`Self::write_page`]) and the
    /// request path ([`DevAssetPipeline::request_write`]), so both
    /// observe the identical dedup + commit-after-success discipline.
    /// Returns `true` when the bytes differed from the cached copy and
    /// the write ran.
    ///
    /// `dest` must already be validated (via `validate_output_path`).
    fn write_bytes(&self, dest: &PathBuf, new_bytes: Vec<u8>) -> Result<bool> {
        let changed = {
            let cache = self.last_bytes.lock().unwrap_or_else(|p| {
                tracing::warn!(site = "WriteCache.last_bytes", "mutex poisoned, recovering");
                p.into_inner()
            });
            !matches!(cache.get(dest), Some(prev) if prev == &new_bytes)
        };

        // After a prune the new path is by definition different from
        // anything we've written before, so we always (re-)emit.
        // `changed` already flips true via the cache miss above. We
        // still want to write before we delete the previous artifact.

        if changed {
            // Pass the raw bytes through — `atomic_write` is the
            // canonical write helper. Going through
            // `from_utf8(...).unwrap_or("")` previously turned any
            // encoding hiccup into a silent blank file.
            atomic_write(dest, &new_bytes)?;
            // Only after the write succeeds do we record the new bytes
            // in the dedup cache. If we updated the cache before the
            // write a transient I/O failure would poison it: the next
            // tick's identical bytes would be deemed "unchanged" and the
            // file would silently never make it to disk.
            self.last_bytes
                .lock()
                .unwrap_or_else(|p| {
                    tracing::warn!(
                        site = "WriteCache.last_bytes (commit)",
                        "mutex poisoned, recovering"
                    );
                    p.into_inner()
                })
                .insert(dest.clone(), new_bytes);
        }

        Ok(changed)
    }

    /// Evict `path` from both caches (last_bytes and last_output_path).
    ///
    /// Called during the deferred-prune step after a path has been
    /// confirmed not live (not in `live_dests`) and `fs::remove_file`
    /// has been attempted. Eviction ensures the path is never revisited
    /// as a prune candidate on the next tick.
    fn evict_path(&self, path: &PathBuf) {
        self.last_bytes
            .lock()
            .unwrap_or_else(|p| {
                tracing::warn!(
                    site = "WriteCache.last_bytes (evict)",
                    "mutex poisoned, recovering"
                );
                p.into_inner()
            })
            .remove(path);

        self.last_output_path
            .lock()
            .unwrap_or_else(|p| {
                tracing::warn!(
                    site = "WriteCache.last_output_path (evict)",
                    "mutex poisoned, recovering"
                );
                p.into_inner()
            })
            .retain(|_, v| v != path);
    }

    /// Clear both caches. Useful when the dist root is wiped from
    /// outside the orchestrator and the caller wants the next rebuild
    /// to re-emit every page.
    fn clear(&self) {
        self.last_bytes
            .lock()
            .unwrap_or_else(|p| {
                tracing::warn!(
                    site = "WriteCache.last_bytes (clear)",
                    "mutex poisoned, recovered"
                );
                p.into_inner()
            })
            .clear();
        self.last_output_path
            .lock()
            .unwrap_or_else(|p| {
                tracing::warn!(
                    site = "WriteCache.last_output_path (clear)",
                    "mutex poisoned, recovered"
                );
                p.into_inner()
            })
            .clear();
    }
}

// ── Shared write state + tick-vs-request exclusion ──────────────────────────

/// The write state shared between the tick path and the request path:
/// the [`WriteCache`] plus the pipeline-wide exclusion lock.
///
/// ## Exclusion discipline (issues #727 / #804 / #1024)
///
/// One mutex, held **exclusively** by both sides:
///
/// - [`DevAssetPipeline::apply`] holds it for the **whole tick** —
///   renderer reload, render fan-out, write loop, and the deferred-prune
///   window included.
/// - A request-time write ([`DevAssetPipeline::request_write`] /
///   [`RequestWriter::request_write`]) holds it for one
///   validate → dedup → atomic-write → commit sequence.
///
/// So a request-time write completes entirely before or entirely after
/// an `apply()`; it is never partially interleaved. Why this is
/// load-bearing:
///
/// - **#727 (deferred prune + `live_dests`):** the per-tick `live_dests`
///   set is only coherent inside one `apply()`. A request-time write
///   landing inside the tick's prune window could write a path the tick
///   is about to prune — the fresh artifact would be deleted right after
///   the write (write-after-prune orphan) — or re-materialise a path the
///   tick has already declared vanished.
/// - **#804 (vanish eviction):** when a route vanishes, the tick deletes
///   the file AND evicts its `last_bytes` entry as one logical step. A
///   write slipping between the delete and the evict would commit cache
///   bytes for a file the evict then forgets — the next identical write
///   would be wrongly deduped against a file that is gone from disk.
/// - **request-vs-request:** two concurrent writes to the same dest
///   could commit `last_bytes` in the opposite order of their on-disk
///   renames, leaving the dedup cache claiming bytes that are not what
///   is on disk.
///
/// Lock ordering: `exclusion` is always the outermost lock; the
/// `WriteCache` mutexes are leaf-level and never held across a call that
/// takes `exclusion`, so no inversion is possible.
#[derive(Debug, Default)]
struct WriteShared {
    cache: WriteCache,
    exclusion: Mutex<()>,
}

impl WriteShared {
    /// Acquire the tick-vs-request exclusion lock (poison-recovering, in
    /// line with the `WriteCache` mutexes — a panicked tick must not
    /// wedge the dev server's request path forever).
    fn lock_exclusion(&self, site: &'static str) -> std::sync::MutexGuard<'_, ()> {
        self.exclusion.lock().unwrap_or_else(|p| {
            tracing::warn!(site, "exclusion mutex poisoned, recovering");
            p.into_inner()
        })
    }

    /// Request-time write sequence: exclusion → `validate_output_path`
    /// → byte-dedup → `atomic_write` → commit `last_bytes`.
    fn request_write(
        &self,
        dist_root: &Path,
        output_path: &Path,
        bytes: Vec<u8>,
    ) -> Result<RequestWriteOutcome> {
        let _exclusion = self.lock_exclusion("WriteShared.exclusion (request_write)");
        self.validate_and_write(dist_root, output_path, bytes)
    }

    /// Guarded request-time write (issue #1027 lazy race): exclusion →
    /// `revalidate()` → (only if it passed) the same validate → dedup →
    /// write → commit sequence as [`Self::request_write`].
    ///
    /// The revalidation closure runs strictly UNDER the exclusion lock
    /// and strictly BEFORE any other work, so a `false` answer skips the
    /// write wholesale — see [`RequestWriter::request_write_guarded`]
    /// for why that ordering is load-bearing.
    fn request_write_guarded(
        &self,
        dist_root: &Path,
        output_path: &Path,
        bytes: Vec<u8>,
        revalidate: impl FnOnce() -> bool,
    ) -> Result<GuardedWriteOutcome> {
        let _exclusion = self.lock_exclusion("WriteShared.exclusion (request_write_guarded)");
        if !revalidate() {
            return Ok(GuardedWriteOutcome::Skipped);
        }
        Ok(GuardedWriteOutcome::Written(self.validate_and_write(
            dist_root,
            output_path,
            bytes,
        )?))
    }

    fn request_write_guarded_then(
        &self,
        dist_root: &Path,
        output_path: &Path,
        bytes: Vec<u8>,
        revalidate: impl FnOnce() -> bool,
        after_write: impl FnOnce(),
    ) -> Result<GuardedWriteOutcome> {
        let _exclusion = self.lock_exclusion("WriteShared.exclusion (request_write_guarded_then)");
        if !revalidate() {
            return Ok(GuardedWriteOutcome::Skipped);
        }
        let outcome = self.validate_and_write(dist_root, output_path, bytes)?;
        after_write();
        Ok(GuardedWriteOutcome::Written(outcome))
    }

    /// The exclusion-agnostic core shared by [`Self::request_write`] and
    /// [`Self::request_write_guarded`]. Callers MUST hold the exclusion
    /// lock.
    fn validate_and_write(
        &self,
        dist_root: &Path,
        output_path: &Path,
        bytes: Vec<u8>,
    ) -> Result<RequestWriteOutcome> {
        // Same write-boundary validation as the tick path: reject any
        // output path that escapes dist_root via `..`, absolute roots,
        // or a planted symlink.
        let dest = validate_output_path(dist_root, output_path)
            .with_context(|| format!("while request-writing {}", output_path.display()))?;

        let written = self.cache.write_bytes(&dest, bytes)?;
        Ok(RequestWriteOutcome { written })
    }
}

// ── Public pipeline ──────────────────────────────────────────────────────────

/// The default `zfb dev`-mode asset pipeline.
#[derive(Default)]
pub struct DevAssetPipeline {
    shared: Arc<WriteShared>,
    /// Issue #1025 (lazy dev render): optional probe draining the routes
    /// the renderer marked stale this tick into
    /// [`BuildOutcome::pages_stale`]. `None` (tests, production-shaped
    /// callers) leaves the field empty.
    stale_probe: Option<StaleProbe>,
    /// Issue #1826 (Dev Self Heal epic #1999): optional probe draining
    /// the "SSR routes were published this tick" one-shot flag into
    /// [`BuildOutcome::ssr_routes_published`]. `None` (tests,
    /// production-shaped callers) leaves the field `false`.
    ssr_publish_probe: Option<SsrPublishProbe>,
    /// Issue #2097 (MDX Reload Fix epic #2092): optional probe draining
    /// the "dynamic injected routes were re-staled this tick" one-shot
    /// flag into [`BuildOutcome::dynamic_injected_restaled`]. `None`
    /// (tests, production-shaped callers) leaves the field `false`.
    dynamic_injected_probe: Option<DynamicInjectedProbe>,
    lazy_fallback_probe: Option<LazyFallbackProbe>,
    document_publication_hook: Option<DocumentPublicationHook>,
}

impl std::fmt::Debug for DevAssetPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DevAssetPipeline")
            .field("shared", &self.shared)
            .field(
                "stale_probe",
                &self.stale_probe.as_ref().map(|_| "<callback>"),
            )
            .field(
                "ssr_publish_probe",
                &self.ssr_publish_probe.as_ref().map(|_| "<callback>"),
            )
            .field(
                "dynamic_injected_probe",
                &self.dynamic_injected_probe.as_ref().map(|_| "<callback>"),
            )
            .field(
                "lazy_fallback_probe",
                &self.lazy_fallback_probe.as_ref().map(|_| "<callback>"),
            )
            .field(
                "document_publication_hook",
                &self
                    .document_publication_hook
                    .as_ref()
                    .map(|_| "<callback>"),
            )
            .finish()
    }
}

impl DevAssetPipeline {
    /// Construct a fresh pipeline with no last-bytes cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a pipeline that fills [`BuildOutcome::pages_stale`]
    /// from `probe` after every tick (issue #1025 — lazy dev render).
    pub fn with_stale_probe(probe: StaleProbe) -> Self {
        Self {
            shared: Arc::default(),
            stale_probe: Some(probe),
            ssr_publish_probe: None,
            dynamic_injected_probe: None,
            lazy_fallback_probe: None,
            document_publication_hook: None,
        }
    }

    /// Attach the probe that fills
    /// [`BuildOutcome::ssr_routes_published`] after every tick (issue
    /// #1826 — SSR-only reload self-heal). Chainable onto
    /// [`Self::with_stale_probe`]; the two probes are independent
    /// signals drained side by side.
    pub fn with_ssr_publish_probe(mut self, probe: SsrPublishProbe) -> Self {
        self.ssr_publish_probe = Some(probe);
        self
    }

    /// Attach the probe that fills
    /// [`BuildOutcome::dynamic_injected_restaled`] after every tick
    /// (issue #2097 — dynamic-injected reload self-heal). Chainable onto
    /// [`Self::with_stale_probe`] beside
    /// [`Self::with_ssr_publish_probe`]; all three probes are
    /// independent signals drained side by side.
    pub fn with_dynamic_injected_probe(mut self, probe: DynamicInjectedProbe) -> Self {
        self.dynamic_injected_probe = Some(probe);
        self
    }

    /// Attach the complete stale-document snapshot used immediately before
    /// an ordinary hydration publication commit. The probe runs under the
    /// same tick/request exclusion as the commit and request finalizers.
    pub fn with_lazy_fallback_probe(mut self, probe: LazyFallbackProbe) -> Self {
        self.lazy_fallback_probe = Some(probe);
        self
    }

    /// Attach the one-lock document/entry publication transaction bridge.
    pub fn with_document_publication_hook(mut self, hook: DocumentPublicationHook) -> Self {
        self.document_publication_hook = Some(hook);
        self
    }

    /// Forget the last-bytes and last-output caches. Useful when the
    /// dist root is wiped from outside the orchestrator and the caller
    /// wants the next rebuild to re-emit every page.
    ///
    /// Takes the tick-vs-request exclusion lock: a reset interleaving
    /// with a tick's write loop would clear some pages' dedup entries
    /// mid-tick (re-emitting only a suffix of the tick's pages on the
    /// next rebuild) and could slip inside the deferred-prune window.
    pub fn reset_cache(&self) {
        let _exclusion = self
            .shared
            .lock_exclusion("WriteShared.exclusion (reset_cache)");
        self.shared.cache.clear();
    }

    /// Write `bytes` to `<dist_root>/<output_path>` at **request time**
    /// (i.e. from the dev server's HTTP path, outside any watcher tick),
    /// with the same validate → byte-dedup → atomic-write → commit
    /// discipline the tick's write loop uses.
    ///
    /// Returns [`RequestWriteOutcome::written`] = `true` when the bytes
    /// differed from the dedup cache and the file was written. The flag
    /// is informational only — request-time writes never feed
    /// [`super::BuildOutcome::pages_written`] and never emit SSE reload
    /// events.
    ///
    /// ## Exclusion vs `apply()`
    ///
    /// This call takes the pipeline-wide exclusion lock and therefore
    /// **blocks while a tick is in flight**: it completes entirely
    /// before or entirely after an [`AssetPipeline::apply`], never
    /// inside a tick's deferred-prune window. See [`WriteShared`] for
    /// why interleaving would corrupt the #727 prune coherence and the
    /// #804 vanish-eviction step.
    ///
    /// ## Dedup coherence
    ///
    /// The dedup cache is shared with the tick path: a tick whose
    /// renderer re-emits exactly the bytes a request wrote skips the
    /// write (no spurious rewrite), and a request re-sending the bytes a
    /// tick wrote is skipped likewise. After a prune evicts a path, the
    /// next write to it always runs (cache miss by construction).
    pub fn request_write(
        &self,
        dist_root: &Path,
        output_path: &Path,
        bytes: Vec<u8>,
    ) -> Result<RequestWriteOutcome> {
        self.shared.request_write(dist_root, output_path, bytes)
    }

    /// See [`RequestWriter::request_write_guarded`] — the same guarded
    /// write through the pipeline handle.
    pub fn request_write_guarded(
        &self,
        dist_root: &Path,
        output_path: &Path,
        bytes: Vec<u8>,
        revalidate: impl FnOnce() -> bool,
    ) -> Result<GuardedWriteOutcome> {
        self.shared
            .request_write_guarded(dist_root, output_path, bytes, revalidate)
    }

    /// A cheap clonable handle onto this pipeline's write state, for
    /// callers that need to perform [`Self::request_write`]s after the
    /// pipeline itself has been moved into the
    /// [`crate::BuildOrchestrator`] (which takes it by value). Clone the
    /// handle out first, then hand the pipeline over.
    pub fn request_writer(&self) -> RequestWriter {
        RequestWriter {
            shared: Arc::clone(&self.shared),
        }
    }
}

/// Clonable, `Send + Sync` handle for request-time writes against a
/// [`DevAssetPipeline`]'s shared write state.
///
/// Obtained via [`DevAssetPipeline::request_writer`]. All clones share
/// the same dedup cache and the same tick-vs-request exclusion lock as
/// the pipeline they came from (see [`DevAssetPipeline::request_write`]
/// for the full contract).
#[derive(Debug, Clone)]
pub struct RequestWriter {
    shared: Arc<WriteShared>,
}

impl RequestWriter {
    /// See [`DevAssetPipeline::request_write`].
    pub fn request_write(
        &self,
        dist_root: &Path,
        output_path: &Path,
        bytes: Vec<u8>,
    ) -> Result<RequestWriteOutcome> {
        self.shared.request_write(dist_root, output_path, bytes)
    }

    /// [`Self::request_write`] with a freshness recheck under the
    /// exclusion lock (issue #1027 lazy race).
    ///
    /// The request-time lazy render releases the renderer mutex after
    /// rendering and only THEN takes this pipeline's exclusion lock. A
    /// watcher tick completing in that gap can eagerly re-render and
    /// write the SAME route with fresher bytes (evicting its stale
    /// entry), or re-stale it at a newer generation. An unguarded
    /// `request_write` would then overwrite the newer HTML with bytes
    /// rendered against the older host — a silent stale serve until the
    /// next edit.
    ///
    /// `revalidate` closes that gap: it runs strictly under the
    /// exclusion lock (so no tick is in flight while it answers) and
    /// strictly before any validation or disk work. Callers pass a
    /// closure re-checking that their claim on the route is still
    /// exactly current; on `false` the write is skipped wholesale and
    /// [`GuardedWriteOutcome::Skipped`] is returned — the interleaving
    /// tick's bytes stay authoritative on disk and in the dedup cache.
    ///
    /// Lock ordering: identical to `request_write` — the exclusion lock
    /// only. `revalidate` may take leaf locks of its own (e.g. the dev
    /// session's stale map, which ticks also take INSIDE their
    /// exclusion window) but must never acquire the renderer mutex
    /// (that would invert against a tick's exclusion → renderer
    /// nesting).
    pub fn request_write_guarded(
        &self,
        dist_root: &Path,
        output_path: &Path,
        bytes: Vec<u8>,
        revalidate: impl FnOnce() -> bool,
    ) -> Result<GuardedWriteOutcome> {
        self.shared
            .request_write_guarded(dist_root, output_path, bytes, revalidate)
    }

    /// Guarded request write plus an indivisible publication finalizer.
    pub fn request_write_guarded_then(
        &self,
        dist_root: &Path,
        output_path: &Path,
        bytes: Vec<u8>,
        revalidate: impl FnOnce() -> bool,
        after_write: impl FnOnce(),
    ) -> Result<GuardedWriteOutcome> {
        self.shared.request_write_guarded_then(
            dist_root,
            output_path,
            bytes,
            revalidate,
            after_write,
        )
    }
}

impl AssetPipeline for DevAssetPipeline {
    fn apply(&self, plan: &RebuildPlan, ctx: &BuildContext) -> Result<BuildOutcome> {
        let has_document_writes = match &plan.pages {
            PageSelection::Specific(pages) => !pages.is_empty(),
            PageSelection::All => true,
        };
        let publication_relevant = has_document_writes
            || plan.rerun_islands
            || plan.rerun_client_scripts
            || plan.ssr_reload_needed;
        // Tick-vs-request exclusion (#727 / #804 / #1024): hold the
        // pipeline-wide lock for the WHOLE tick so no request-time write
        // can interleave with the render fan-out or the deferred-prune
        // window. See [`WriteShared`] for the discipline.
        let _exclusion = self.shared.lock_exclusion("WriteShared.exclusion (apply)");
        // Publication Begin belongs to the same serialization domain as
        // request-time recovery commits. Otherwise a request could finish its
        // write, a tick could Begin and block here, and the request could then
        // accidentally commit that not-yet-rendered tick.
        let mut publication = DocumentPublicationTransaction::begin(
            self.document_publication_hook.as_ref(),
            publication_relevant,
        );

        let mut outcome = BuildOutcome::default();

        // 1. Pages.
        let pages: Vec<PageId> = match &plan.pages {
            PageSelection::All => {
                // The orchestrator should have resolved this. If we still
                // see All here it means the caller bypassed the
                // orchestrator — surface it as an error rather than
                // silently no-op'ing.
                return Err(anyhow::anyhow!(
                    "DevAssetPipeline: PageSelection::All must be resolved to a concrete page list \
                     by the orchestrator before reaching the pipeline"
                ));
            }
            PageSelection::Specific(s) => s.iter().cloned().collect(),
        };

        // Client scripts publish before the renderer refresh and page writes.
        // This closes the addition side of the publication window: newly
        // rendered HTML cannot name an entry that has not been written yet.
        // The runner owns the other side of the invariant by retaining the
        // previous active entry/worker generation until after this tick's HTML
        // is published; see the dev command's client-output pruning contract.
        // Its successful return also publishes the current-only URL set into
        // `DevPublicationState`, so the timing marker remains an honest
        // publication boundary rather than merely a bundle-start signal.
        if plan.rerun_client_scripts {
            outcome.client_scripts_rerun = true;
            if let Some(run) = &ctx.run_client_scripts {
                outcome.client_scripts_changed = run()?;
                if dev_timing_enabled() {
                    eprintln!("{}", format_tick_client_scripts_published_line());
                }
            }
        }

        // Islands publish before renderer refresh and page writes for the same
        // addition-side reason as client scripts. The command-layer publisher
        // stages the already-written entry in the one-lock state, so a page
        // request racing the later write loop can inject the candidate entry
        // while readiness remains false. Removal keeps the committed entry and
        // companions until the document transaction commits.
        if plan.rerun_islands {
            outcome.islands_rerun = true;
            if let Some(run) = &ctx.run_islands {
                if let Some(info) = run()? {
                    outcome.islands_changed = info.changed;
                    outcome.islands_bundle = Some(info);
                    if dev_timing_enabled() {
                        eprintln!("{}", format_tick_islands_published_line());
                    }
                }
            }
        }

        // Trigger the renderer reload for SSR-only projects (issue #807): when
        // every page is `prerender = false`, the plan's SSG page set is always
        // empty, so the normal pages-non-empty guard would never fire. But the
        // V8 host still needs a fresh bundle after each relevant edit so SSR
        // requests serve the updated code. Fire the reload here when:
        //   - the tick is SSR-relevant (Content/Page/Module/Data change), AND
        //   - the plan has no SSG pages (would be missed by the loop below), AND
        //   - the renderer was not already refreshed this tick.
        // For mixed projects (some SSG pages) this block is skipped — the pages
        // loop below fires the reload as before, so there is no double-reload.
        if pages.is_empty() && plan.ssr_reload_needed && !plan.renderer_fresh {
            if let Some(reload) = &ctx.reload_renderer {
                // Vanished paths from an SSR-only reload are appended to
                // prune_paths below; the pages loop's prune infrastructure
                // is bypassed here so we handle them in the plan-prune block.
                // A `Skipped` outcome (issue #956) means the live host and
                // route tables were left untouched — nothing vanished,
                // nothing to prune.
                let vanished = match reload()? {
                    RefreshOutcome::Skipped => Vec::new(),
                    RefreshOutcome::Refreshed { vanished, .. } => vanished,
                };
                if !vanished.is_empty() {
                    let mut removed = Vec::new();
                    outcome.pages_pruned.extend(vanished.iter().cloned());
                    for prev in &vanished {
                        publication.document_write_started(prev.clone());
                        match std::fs::remove_file(prev) {
                            Ok(()) => removed.push(prev.clone()),
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                                removed.push(prev.clone());
                            }
                            Err(_) => {}
                        }
                        self.shared.cache.evict_path(prev);
                    }
                    publication.document_paths_removed(removed);
                }
            }
        }

        // Vanished route output paths consumed by the deferred-prune loop
        // inside the render block: plan-carried paths (issue #804 P2) plus
        // whatever the reloader reports. Hoisted out of the render block so
        // the Skipped path (issue #956) leaves them to the plan-prune block
        // (1b) below instead.
        let mut route_vanished: Vec<PathBuf> = plan.prune_paths.clone();

        // True when the reloader reported [`RefreshOutcome::Skipped`]
        // (issue #956): byte-identical bundle + unchanged route universe.
        // The V8 host boot + `paths()` re-expansion inside the reloader is
        // bypassed (the #940/#956 win) — but the render fan-out itself is
        // NOT (issue #1301): the skip-key carries no page-selection info, so
        // bypassing render would silently lose a later tick's route staleness
        // when its selection differs from an earlier byte-identical tick.
        // Instead only the ROUTE-VANISHED prune half is skipped here (issue
        // #1317) — on a skip the route table did not move, so nothing vanished
        // there; the per-page stale-output prune still runs, since a skipped
        // tick can still move a page's `output_path`. The render half still
        // runs; redundant re-renders against an identical bundle re-emit
        // identical HTML and are absorbed by the `last_bytes` byte-dedup
        // cache (no write, no SSE). CSS, islands, and plan prune paths run.
        let mut renderer_refresh_skipped = false;

        // Content-narrowing hint for the render callback (issues #958 /
        // #1058). G5/G6 historically dropped the hint to `None` because,
        // under the old contract, a hint meant "narrow the eager fan-out" —
        // unsafe when discovery refreshed the renderer (G6) or the route
        // structure moved (G5), since narrowing could orphan a brand-new
        // URL. The hint now ALSO carries the lazy eager basis (the edited
        // entries' own routes), which is ALWAYS safe: it only ADDS eager
        // renders for known routes and never restricts the fan-out. So
        // instead of dropping the hint, clear only its `fan_out_safe` flag —
        // the eager path falls back to the full fan-out (it gates on
        // `fan_out_safe`) while the lazy path still eager-renders the edited
        // entry's own route even when a co-changed Created file refreshed the
        // renderer this tick (the #1058 mixed-tick failure).
        //
        // G6: a discovery-refreshed renderer (`renderer_fresh`) implies a
        // created file in this tick → the fan-out is no longer safe to narrow.
        let mut narrowing: Option<crate::ContentNarrowing> = plan.content_narrowing.clone();
        if plan.renderer_fresh {
            if let Some(n) = narrowing.as_mut() {
                n.fan_out_safe = false;
            }
        }

        if !pages.is_empty() {
            // Reload the SSR renderer (embedded V8 host) before rendering
            // pages whenever the dirty set is non-empty — UNLESS the
            // renderer bundle was already refreshed this tick (boot's
            // eager initial render, or a watch-ADD discovery re-bundle;
            // see `RebuildPlan::renderer_fresh`). Errors abort the
            // tick — the watcher stays alive and the previous renderer
            // state is preserved by the orchestrator.
            //
            // The reloader also returns the globally-vanished absolute
            // output paths (routes that existed before this tick's
            // route-table rebuild but are absent from every source's new
            // entry set). These are fed into the prune loop below.
            //
            // When the renderer was already refreshed this tick (e.g. by
            // the discovery hook), reload_renderer is skipped — but the
            // plan may still carry vanished paths from that same discovery
            // refresh via RebuildPlan::prune_paths (issue #804 P2).
            if !plan.renderer_fresh {
                if let Some(reload) = &ctx.reload_renderer {
                    match reload()? {
                        RefreshOutcome::Skipped => renderer_refresh_skipped = true,
                        RefreshOutcome::Refreshed {
                            vanished,
                            changed_sources,
                        } => {
                            // Fallback G5 (issue #958): the refresh ran
                            // BEFORE the render, so the route table the
                            // narrowing would match against is the
                            // post-edit one — but if the refresh reports
                            // that any source's route-entry set changed
                            // (or any output path vanished), the route
                            // structure moved this tick and narrowing the
                            // eager FAN-OUT could orphan a brand-new URL.
                            // Clear `fan_out_safe` so the eager path renders
                            // the full selected set, while the lazy eager
                            // basis (own-route renders only) survives (#1058).
                            if !vanished.is_empty() || !changed_sources.is_empty() {
                                if let Some(n) = narrowing.as_mut() {
                                    n.fan_out_safe = false;
                                }
                            }
                            route_vanished.extend(vanished);
                        }
                    }
                }
            }
        }

        // Render + write is decoupled from prune (issue #1301): a skipped
        // tick (issue #956) still re-renders its SELECTED pages so a later
        // tick whose page-selection differs from an earlier byte-identical
        // tick does not silently serve stale bytes. The skip-key carries no
        // page-selection info, so the bypass could drop the second tick's
        // route staleness entirely. Redundant re-renders against an
        // identical bundle are absorbed by `WriteCache.last_bytes` byte-dedup
        // (already-fresh pages produce no write, no SSE), the same
        // determinism assumption the skip originally relied on.
        //
        // The render + write half runs unconditionally when `pages` is
        // non-empty. Pruning below is split by trigger (issue #1317): the
        // per-page stale-output half runs too (a skipped tick can still move a
        // page's `output_path`), while the route-vanished half stays gated on
        // `!renderer_refresh_skipped` — on a skip the route table did not
        // move, so nothing vanished there.
        let mut prune_candidates: Vec<PathBuf> = Vec::new();
        let mut live_dests: HashSet<PathBuf> = HashSet::new();
        if !pages.is_empty() {
            publication.document_mutation_started();
            let rendered = (ctx.render_pages)(&pages, narrowing.as_ref())?;
            outcome.pages_rendered = rendered.len();

            // Collect prune candidates and the live dest set during the
            // write loop; the actual deletes are deferred to after the
            // loop (and gated on `!renderer_refresh_skipped`). This prevents
            // a page's prune from deleting a path that a sibling page in the
            // same tick has already written (or will write) to — i.e. the
            // two-page output-path swap scenario described in issue #727.
            for r in rendered {
                // `RelDistPath` is lexically contained, so record its stable
                // absolute identity before filesystem/symlink validation.
                // A validation failure is itself an unresolved document
                // attempt and must survive unrelated narrowed ticks.
                let publication_path = ctx.dist_root.join(r.output_path.as_path());
                publication.document_write_started(publication_path.clone());
                // Reject any output_path that escapes dist_root via
                // `..` or absolute roots before we touch the
                // filesystem. Paths come from the renderer/router but
                // we still validate at the write boundary.
                let dest = validate_output_path(&ctx.dist_root, r.output_path.as_path())
                    .with_context(|| format!("while building page {:?}", r.page))?;
                let new_bytes = r.html.into_bytes();

                // Every dest produced in this tick is "live" regardless
                // of whether its bytes changed. Record it before any
                // conditional logic so unchanged pages still protect
                // their path from a sibling's deferred prune.
                live_dests.insert(dest.clone());

                let wo = self.shared.cache.write_page(&r.page, &dest, new_bytes)?;
                publication.document_write_completed(publication_path);
                if let Some(stale) = wo.stale_path {
                    prune_candidates.push(stale);
                }
                if wo.written {
                    outcome.pages_written.push(r.page.clone());
                }
            }
            if dev_timing_enabled() {
                eprintln!("{}", format_tick_page_write_complete_line());
            }
        }

        // Deferred prune, split by trigger (issue #1317). The two prune
        // sources fire on DIFFERENT conditions and must NOT share one gate:
        //
        //   (a) `prune_candidates` — per-page stale outputs collected by
        //       `write_page` when a page's SAME `PageId` moved to a new
        //       `output_path` this tick. Produced by the render loop that
        //       JUST RAN, so they must be pruned whenever that loop ran —
        //       INCLUDING a skipped tick (issue #956): the render+write half
        //       still runs on a skip (issue #1301), so `last_output_path`
        //       already advanced to the new path. Gating these on
        //       `!renderer_refresh_skipped` (the pre-#1317 bug) dropped the
        //       candidate while the cache pointed at the new path, leaking the
        //       old artifact in dist/ and its `last_bytes` entry forever
        //       (self-heals only on restart / next real bundle change). So:
        //       prune candidates whenever `!pages.is_empty()`, skip or not.
        //
        //   (b) `route_vanished` — route-table-level vanished output paths
        //       (globally-vanished routes returned by reload_renderer, plus
        //       the `plan.prune_paths` clone). These describe route-table
        //       MOVEMENT, which on a skip did NOT happen — so they stay gated
        //       on `!renderer_refresh_skipped`. When that gate is off (skip),
        //       `plan.prune_paths` is left to the plan-prune block (1b) below,
        //       which consumes it exactly once.
        //
        // Both halves keep the #727 `live_dests` guard: skip any path a
        // sibling page in this same tick now owns, so deleting it would remove
        // that sibling's freshly-written artifact. Both also `evict_path` the
        // pruned path (fixes the #1317 `last_bytes` cache-leak half on the
        // skip path too). NOTE: "exact complement of 1b" (below) applies to
        // the (b) route-vanished gate specifically, NOT to (a) — the next
        // reader must not re-derive it for all pruning.
        if !pages.is_empty() {
            let mut removed = Vec::new();
            // (a) Per-page stale outputs — pruned whenever the render loop
            // ran, skip or not (issue #1317 / #1301).
            for prev in prune_candidates {
                if live_dests.contains(&prev) {
                    continue; // another page now owns this path — skip
                }
                publication.document_write_started(prev.clone());
                match std::fs::remove_file(&prev) {
                    Ok(()) => removed.push(prev.clone()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        removed.push(prev.clone());
                    }
                    Err(_) => {}
                }
                self.shared.cache.evict_path(&prev);
                outcome.pages_pruned.push(prev);
            }

            // (b) Globally-vanished route output paths — only when the route
            // table actually moved (not on a skip; issue #956). E.g. a content
            // file was deleted or a dynamic route's paths() output shrank. Same
            // live_dests guard: if route A lost /x while route B simultaneously
            // gained /x, /x must NOT be deleted. `route_vanished` carries
            // `plan.prune_paths`; when this half is gated off (skip), those
            // paths are handled once by the plan-prune block (1b) below.
            if !renderer_refresh_skipped {
                for prev in route_vanished {
                    if live_dests.contains(&prev) {
                        continue; // another page now owns this path — skip
                    }
                    publication.document_write_started(prev.clone());
                    match std::fs::remove_file(&prev) {
                        Ok(()) => removed.push(prev.clone()),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            removed.push(prev.clone());
                        }
                        Err(_) => {}
                    }
                    self.shared.cache.evict_path(&prev);
                    outcome.pages_pruned.push(prev);
                }
            }
            publication.document_paths_removed(removed);
        }

        // 1b. Prune paths from the plan (issue #804 P2): paths explicitly
        // supplied by the orchestrator from a discovery refresh that already
        // set renderer_fresh (so reload_renderer was skipped and these paths
        // could not be returned through the normal vanished-routes channel).
        //
        // Exactly-once (issue #1317): `plan.prune_paths` is carried into
        // `route_vanished` and consumed only by the (b) route-vanished half
        // above, whose gate is `!pages.is_empty() && !renderer_refresh_skipped`.
        // This block's gate is the EXACT COMPLEMENT of THAT (b) gate
        // specifically — NOT of all pruning: the (a) per-page half now also
        // runs on a skip, but it never touches `route_vanished`/
        // `plan.prune_paths`. So `plan.prune_paths` runs here only when pages
        // are empty or the renderer refresh was skipped, and is processed
        // EXACTLY ONCE.
        //
        // 1b `live_dests` gap (issue #1317, re-verified): this block consumes
        // `plan.prune_paths` WITHOUT a live_dests guard. On the
        // `renderer_refresh_skipped && !pages.is_empty()` path `live_dests` is
        // non-empty while this runs unguarded — but that combination cannot
        // occur in production: `plan.prune_paths` only comes from a discovery
        // refresh, which sets `renderer_fresh`, which skips the reload_renderer
        // call entirely (so `renderer_refresh_skipped` stays false). The
        // #1317 split did not change this: the (a) per-page half runs on a
        // skip but leaves `plan.prune_paths` to this block. Argument still
        // holds → no guard needed here.
        //
        // Note the render+write half (issue #1301) may still have run above on
        // a skipped tick — that half does not consume `route_vanished`, only
        // the (b) route-vanished half does.
        if (pages.is_empty() || renderer_refresh_skipped) && !plan.prune_paths.is_empty() {
            let mut removed = Vec::new();
            for prev in &plan.prune_paths {
                publication.document_write_started(prev.clone());
                match std::fs::remove_file(prev) {
                    Ok(()) => removed.push(prev.clone()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        removed.push(prev.clone());
                    }
                    Err(_) => {}
                }
                self.shared.cache.evict_path(prev);
                outcome.pages_pruned.push(prev.clone());
            }
            publication.document_paths_removed(removed);
        }

        // This is the hydration publication boundary: all selected document
        // writes and both prune halves have completed, while unrelated CSS and
        // outcome probes have not started. A later CSS/probe failure must not
        // roll back a document/entry generation that is already observable.
        if publication_relevant {
            if let (Some(probe), Some(hook)) =
                (&self.lazy_fallback_probe, &self.document_publication_hook)
            {
                hook(DocumentPublicationEvent::SnapshotLazyFallbacks(probe()));
            }
            let event = if has_document_writes {
                DocumentPublicationEvent::CommitPublished
            } else {
                DocumentPublicationEvent::CommitEntriesOnly
            };
            publication.commit(event);
        }

        // 2. CSS.
        if plan.rerun_css {
            outcome.css_rerun = true;
            if let Some(run) = &ctx.run_css {
                outcome.css_changed = run()?;
            }
        }

        // 3. Stale routes (issue #1025 — lazy dev render). Drain the
        // renderer's per-tick stale buffer into the outcome. The render
        // callback (and the reloader's route-table/stale-state updates)
        // completed above, so by construction the staleness map already
        // describes the new world by the time this outcome reaches
        // `on_outcome`. Empty on every tick while the lazy switch is off.
        if let Some(probe) = &self.stale_probe {
            outcome.pages_stale = probe();
        }

        // 4. SSR route publication (issue #1826 — Dev Self Heal epic
        // #1999). Drained beside the stale buffer for the same reason and
        // at the same point: an SSR route has no `dist/` output path, so
        // it can never show up in `pages_stale`, and without this bit an
        // SSR-only project's outcome is indistinguishable from an empty
        // tick. `false` on every tick that did not publish SSR routes, and
        // on every caller that wired no probe.
        if let Some(probe) = &self.ssr_publish_probe {
            outcome.ssr_routes_published = probe();
        }

        // 5. Dynamic injected route re-staling (issue #2097 — MDX Reload
        // Fix epic #2092). Drained beside the two probes above, for the
        // same structural reason: a dynamic injected route is re-staled
        // through a channel that performs no `tick_stale` push, so it can
        // never show up in `pages_stale`, and without this bit an
        // all-dynamic-injected project's outcome is indistinguishable
        // from an empty tick. `false` on every tick that re-staled no
        // dynamic injected route, and on every caller that wired no
        // probe.
        if let Some(probe) = &self.dynamic_injected_probe {
            outcome.dynamic_injected_restaled = probe();
        }

        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{RelDistPath, RenderedPage};
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tempfile::tempdir;

    fn pid(s: &str) -> PageId {
        PageId::new(PathBuf::from(s))
    }

    fn ctx_with_renderer(
        dist_root: PathBuf,
        rendered: Vec<RenderedPage>,
        invocations: Arc<AtomicUsize>,
    ) -> BuildContext {
        BuildContext {
            dist_root,
            render_pages: Arc::new(
                move |_pages: &[PageId], _: Option<&crate::ContentNarrowing>| {
                    invocations.fetch_add(1, Ordering::SeqCst);
                    Ok(rendered.clone())
                },
            ),
            run_css: None,
            run_islands: None,
            run_client_scripts: None,
            reload_renderer: None,
        }
    }

    #[test]
    fn writes_rendered_pages_atomically() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let ctx = ctx_with_renderer(
            dir.path().to_path_buf(),
            vec![RenderedPage {
                page: pid("/p/a.tsx"),
                output_path: RelDistPath::new("a/index.html").unwrap(),
                html: "<h1>A</h1>".into(),
                content_type: None,
            }],
            calls.clone(),
        );

        let mut sel = BTreeSet::new();
        sel.insert(pid("/p/a.tsx"));
        let plan = RebuildPlan {
            pages: PageSelection::Specific(sel),
            rerun_css: false,
            rerun_islands: false,
            rerun_client_scripts: false,
            renderer_fresh: false,
            ssr_reload_needed: false,
            prune_paths: vec![],
            triggers: vec![],
            content_narrowing: None,
        };

        let outcome = pipeline.apply(&plan, &ctx).unwrap();
        assert_eq!(outcome.pages_rendered, 1);
        assert_eq!(outcome.pages_written, vec![pid("/p/a.tsx")]);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a/index.html")).unwrap(),
            "<h1>A</h1>"
        );
    }

    #[test]
    fn publication_timing_line_format_is_stable() {
        assert_eq!(
            format_tick_page_write_complete_line(),
            "[zfb-timing] tick: page write complete"
        );
        assert_eq!(
            format_tick_islands_published_line(),
            "[zfb-timing] tick: islands published"
        );
        assert_eq!(
            format_tick_client_scripts_published_line(),
            "[zfb-timing] tick: client scripts published"
        );
    }

    fn client_script_page_plan() -> RebuildPlan {
        let mut pages = BTreeSet::new();
        pages.insert(pid("/pages/index.tsx"));
        RebuildPlan {
            pages: PageSelection::Specific(pages),
            rerun_css: false,
            rerun_islands: false,
            rerun_client_scripts: true,
            renderer_fresh: true,
            ssr_reload_needed: false,
            prune_paths: vec![],
            triggers: vec![],
            content_narrowing: None,
        }
    }

    #[test]
    fn client_script_addition_publishes_entry_before_declaring_html() {
        let dir = tempdir().unwrap();
        let entry = dir.path().join("assets/client/new.js");
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let run_events = Arc::clone(&events);
        let run_entry = entry.clone();
        let render_events = Arc::clone(&events);
        let render_entry = entry.clone();
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            run_client_scripts: Some(Arc::new(move || {
                std::fs::create_dir_all(run_entry.parent().unwrap())?;
                std::fs::write(&run_entry, "new entry")?;
                run_events.lock().unwrap().push("client-published");
                Ok(true)
            })),
            render_pages: Arc::new(move |_, _| {
                assert!(
                    render_entry.exists(),
                    "new entry must exist before HTML can declare it"
                );
                render_events.lock().unwrap().push("page-rendered");
                Ok(vec![RenderedPage {
                    page: pid("/pages/index.tsx"),
                    output_path: RelDistPath::new("index.html").unwrap(),
                    html: r#"<script src="/assets/client/new.js"></script>"#.into(),
                    content_type: None,
                }])
            }),
            run_css: None,
            run_islands: None,
            reload_renderer: None,
        };

        DevAssetPipeline::new()
            .apply(&client_script_page_plan(), &ctx)
            .unwrap();

        assert_eq!(
            *events.lock().unwrap(),
            vec!["client-published", "page-rendered"]
        );
    }

    #[test]
    fn islands_and_clients_precede_pages_and_commit_precedes_css_failure() {
        let dir = tempdir().unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let hook_events = Arc::clone(&events);
        let client_events = Arc::clone(&events);
        let islands_events = Arc::clone(&events);
        let render_events = Arc::clone(&events);
        let css_events = Arc::clone(&events);
        let pipeline =
            DevAssetPipeline::new().with_document_publication_hook(Arc::new(move |event| {
                hook_events.lock().unwrap().push(match event {
                    DocumentPublicationEvent::Begin => "begin",
                    DocumentPublicationEvent::SnapshotLazyFallbacks(_) => "snapshot",
                    DocumentPublicationEvent::DocumentMutationStarted => "mutation",
                    DocumentPublicationEvent::DocumentWriteStarted(_) => "write-start",
                    DocumentPublicationEvent::DocumentWriteCompleted(_) => "write-complete",
                    DocumentPublicationEvent::DocumentPathsRemoved(_) => "paths-removed",
                    DocumentPublicationEvent::CommitPublished => "commit",
                    DocumentPublicationEvent::CommitReadyOnRequest
                    | DocumentPublicationEvent::CommitLazyRepair
                    | DocumentPublicationEvent::CommitNotExpected
                    | DocumentPublicationEvent::CommitEntriesOnly => "other-commit",
                    DocumentPublicationEvent::AbortBeforeDocumentWrite => "abort",
                    DocumentPublicationEvent::LeavePending => "pending",
                });
            }));
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            run_client_scripts: Some(Arc::new(move || {
                client_events.lock().unwrap().push("client");
                Ok(true)
            })),
            run_islands: Some(Arc::new(move || {
                islands_events.lock().unwrap().push("islands");
                Ok(Some(crate::pipeline::IslandsBundleInfo {
                    changed: true,
                    bundle_url: "/assets/islands.js".into(),
                    components: Vec::new(),
                }))
            })),
            render_pages: Arc::new(move |_, _| {
                render_events.lock().unwrap().push("render");
                Ok(vec![RenderedPage {
                    page: pid("/pages/index.tsx"),
                    output_path: RelDistPath::new("index.html").unwrap(),
                    html: "<main>ready</main>".into(),
                    content_type: None,
                }])
            }),
            run_css: Some(Arc::new(move || {
                css_events.lock().unwrap().push("css");
                anyhow::bail!("css failed after document publication")
            })),
            reload_renderer: None,
        };
        let mut plan = client_script_page_plan();
        plan.rerun_islands = true;
        plan.rerun_css = true;

        assert!(pipeline.apply(&plan, &ctx).is_err());
        assert_eq!(
            *events.lock().unwrap(),
            vec![
                "begin",
                "client",
                "islands",
                "mutation",
                "render",
                "write-start",
                "write-complete",
                "commit",
                "css"
            ]
        );
    }

    #[test]
    fn complete_lazy_fallback_snapshot_precedes_ordinary_commit() {
        let dir = tempdir().unwrap();
        let fallback = dir.path().join("injected/existing.html");
        let events = Arc::new(Mutex::new(Vec::new()));
        let probe_events = Arc::clone(&events);
        let hook_events = Arc::clone(&events);
        let fallback_for_probe = fallback.clone();
        let pipeline = DevAssetPipeline::new()
            .with_lazy_fallback_probe(Arc::new(move || {
                probe_events.lock().unwrap().push("probe");
                vec![fallback_for_probe.clone()]
            }))
            .with_document_publication_hook(Arc::new(move |event| {
                let label = match event {
                    DocumentPublicationEvent::Begin => "begin",
                    DocumentPublicationEvent::SnapshotLazyFallbacks(paths) => {
                        assert_eq!(paths, vec![fallback.clone()]);
                        "snapshot"
                    }
                    DocumentPublicationEvent::CommitEntriesOnly => "commit",
                    _ => "unexpected",
                };
                hook_events.lock().unwrap().push(label);
            }));
        let mut plan = RebuildPlan::empty();
        plan.rerun_client_scripts = true;
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(|_, _| Ok(Vec::new())),
            run_css: None,
            run_islands: None,
            run_client_scripts: None,
            reload_renderer: None,
        };

        pipeline.apply(&plan, &ctx).unwrap();

        assert_eq!(
            *events.lock().unwrap(),
            vec!["begin", "probe", "snapshot", "commit"]
        );
    }

    #[test]
    fn render_failure_leaves_document_transaction_pending() {
        let dir = tempdir().unwrap();
        let events = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let hook_events = Arc::clone(&events);
        let render_events = Arc::clone(&events);
        let pipeline =
            DevAssetPipeline::new().with_document_publication_hook(Arc::new(move |event| {
                hook_events.lock().unwrap().push(match event {
                    DocumentPublicationEvent::Begin => "begin",
                    DocumentPublicationEvent::DocumentMutationStarted => "mutation",
                    DocumentPublicationEvent::LeavePending => "pending",
                    _ => "unexpected",
                });
            }));
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(move |_, _| {
                render_events.lock().unwrap().push("render");
                anyhow::bail!("render failed")
            }),
            run_css: None,
            run_islands: None,
            run_client_scripts: None,
            reload_renderer: None,
        };

        assert!(pipeline.apply(&client_script_page_plan(), &ctx).is_err());
        assert_eq!(
            *events.lock().unwrap(),
            vec!["begin", "mutation", "render", "pending"]
        );
    }

    #[test]
    fn document_write_failure_records_exact_path_and_leaves_pending() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("index.html");
        std::fs::create_dir_all(&dest).unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let hook_events = Arc::clone(&events);
        let pipeline =
            DevAssetPipeline::new().with_document_publication_hook(Arc::new(move |event| {
                hook_events.lock().unwrap().push(event);
            }));
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(move |_, _| {
                Ok(vec![RenderedPage {
                    page: pid("/pages/index.tsx"),
                    output_path: RelDistPath::new("index.html").unwrap(),
                    html: "<main>cannot replace directory</main>".into(),
                    content_type: None,
                }])
            }),
            run_css: None,
            run_islands: None,
            run_client_scripts: None,
            reload_renderer: None,
        };

        assert!(pipeline.apply(&client_script_page_plan(), &ctx).is_err());
        assert_eq!(
            *events.lock().unwrap(),
            vec![
                DocumentPublicationEvent::Begin,
                DocumentPublicationEvent::DocumentMutationStarted,
                DocumentPublicationEvent::DocumentWriteStarted(dest),
                DocumentPublicationEvent::LeavePending,
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn document_validation_failure_records_lexical_path_and_leaves_pending() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        symlink(outside.path(), dir.path().join("escape")).unwrap();
        let attempted = dir.path().join("escape/index.html");
        let events = Arc::new(Mutex::new(Vec::new()));
        let hook_events = Arc::clone(&events);
        let pipeline =
            DevAssetPipeline::new().with_document_publication_hook(Arc::new(move |event| {
                hook_events.lock().unwrap().push(event);
            }));
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(move |_, _| {
                Ok(vec![RenderedPage {
                    page: pid("/pages/index.tsx"),
                    output_path: RelDistPath::new("escape/index.html").unwrap(),
                    html: "<main>escape rejected</main>".into(),
                    content_type: None,
                }])
            }),
            run_css: None,
            run_islands: None,
            run_client_scripts: None,
            reload_renderer: None,
        };

        assert!(pipeline.apply(&client_script_page_plan(), &ctx).is_err());
        assert_eq!(
            *events.lock().unwrap(),
            vec![
                DocumentPublicationEvent::Begin,
                DocumentPublicationEvent::DocumentMutationStarted,
                DocumentPublicationEvent::DocumentWriteStarted(attempted),
                DocumentPublicationEvent::LeavePending,
            ]
        );
    }

    #[test]
    fn request_repair_finalizer_serializes_before_next_publication_begin() {
        use std::sync::mpsc;
        use std::time::Duration;

        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let (begin_tx, begin_rx) = mpsc::channel();
        let pipeline = Arc::new(
            DevAssetPipeline::new().with_document_publication_hook(Arc::new(move |event| {
                if event == DocumentPublicationEvent::Begin {
                    begin_tx.send(()).unwrap();
                }
            })),
        );
        let writer = pipeline.request_writer();
        let (finalizer_entered_tx, finalizer_entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let write_root = root.clone();
        let request = std::thread::spawn(move || {
            writer
                .request_write_guarded_then(
                    &write_root,
                    Path::new("repair.html"),
                    b"repaired".to_vec(),
                    || true,
                    || {
                        finalizer_entered_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                    },
                )
                .unwrap()
        });
        finalizer_entered_rx.recv().unwrap();

        let tick_pipeline = Arc::clone(&pipeline);
        let tick = std::thread::spawn(move || {
            let mut plan = RebuildPlan::empty();
            plan.rerun_client_scripts = true;
            let ctx = BuildContext {
                dist_root: root,
                render_pages: Arc::new(|_, _| Ok(Vec::new())),
                run_css: None,
                run_islands: None,
                run_client_scripts: None,
                reload_renderer: None,
            };
            tick_pipeline.apply(&plan, &ctx).unwrap();
        });

        assert!(
            begin_rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "tick Begin must wait for the request recovery finalizer",
        );
        release_tx.send(()).unwrap();
        assert!(matches!(
            request.join().unwrap(),
            GuardedWriteOutcome::Written(_)
        ));
        begin_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("tick Begin after request finalizer");
        tick.join().unwrap();
    }

    #[test]
    fn islands_failure_before_document_render_aborts_transaction() {
        let dir = tempdir().unwrap();
        let events = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let hook_events = Arc::clone(&events);
        let islands_events = Arc::clone(&events);
        let render_events = Arc::clone(&events);
        let pipeline =
            DevAssetPipeline::new().with_document_publication_hook(Arc::new(move |event| {
                hook_events.lock().unwrap().push(match event {
                    DocumentPublicationEvent::Begin => "begin",
                    DocumentPublicationEvent::AbortBeforeDocumentWrite => "abort",
                    _ => "unexpected",
                });
            }));
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(move |_, _| {
                render_events.lock().unwrap().push("render");
                Ok(Vec::new())
            }),
            run_css: None,
            run_islands: Some(Arc::new(move || {
                islands_events.lock().unwrap().push("islands");
                anyhow::bail!("islands failed")
            })),
            run_client_scripts: None,
            reload_renderer: None,
        };
        let mut plan = client_script_page_plan();
        plan.rerun_client_scripts = false;
        plan.rerun_islands = true;

        assert!(pipeline.apply(&plan, &ctx).is_err());
        assert_eq!(*events.lock().unwrap(), vec!["begin", "islands", "abort"]);
    }

    #[test]
    fn client_script_removal_keeps_previous_entry_through_html_publication() {
        let dir = tempdir().unwrap();
        let old_entry = dir.path().join("assets/client/old.js");
        std::fs::create_dir_all(old_entry.parent().unwrap()).unwrap();
        std::fs::write(&old_entry, "old entry").unwrap();
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let run_events = Arc::clone(&events);
        let run_entry = old_entry.clone();
        let render_events = Arc::clone(&events);
        let render_entry = old_entry.clone();
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            run_client_scripts: Some(Arc::new(move || {
                assert!(
                    run_entry.exists(),
                    "the previous entry must be retained by the client publisher"
                );
                run_events.lock().unwrap().push("client-published");
                Ok(false)
            })),
            render_pages: Arc::new(move |_, _| {
                assert!(
                    render_entry.exists(),
                    "previously served HTML can still name the old entry until this write"
                );
                render_events.lock().unwrap().push("page-rendered");
                Ok(vec![RenderedPage {
                    page: pid("/pages/index.tsx"),
                    output_path: RelDistPath::new("index.html").unwrap(),
                    html: "<main>client entry removed</main>".into(),
                    content_type: None,
                }])
            }),
            run_css: None,
            run_islands: None,
            reload_renderer: None,
        };

        DevAssetPipeline::new()
            .apply(&client_script_page_plan(), &ctx)
            .unwrap();

        assert!(old_entry.exists(), "old entry survives the transition tick");
        assert_eq!(
            *events.lock().unwrap(),
            vec!["client-published", "page-rendered"]
        );
    }

    #[test]
    fn skips_write_when_html_unchanged() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let rendered = vec![RenderedPage {
            page: pid("/p/a.tsx"),
            output_path: RelDistPath::new("a.html").unwrap(),
            html: "<p>same</p>".into(),
            content_type: None,
        }];
        let ctx = ctx_with_renderer(dir.path().to_path_buf(), rendered, calls);

        let mut sel = BTreeSet::new();
        sel.insert(pid("/p/a.tsx"));
        let plan = RebuildPlan {
            pages: PageSelection::Specific(sel),
            rerun_css: false,
            rerun_islands: false,
            rerun_client_scripts: false,
            renderer_fresh: false,
            ssr_reload_needed: false,
            prune_paths: vec![],
            triggers: vec![],
            content_narrowing: None,
        };

        let first = pipeline.apply(&plan, &ctx).unwrap();
        assert_eq!(first.pages_written.len(), 1);

        let second = pipeline.apply(&plan, &ctx).unwrap();
        // Bytes are the same — nothing new to write.
        assert_eq!(second.pages_written.len(), 0);
        assert_eq!(second.pages_rendered, 1, "renderer still ran");
    }

    #[test]
    fn css_rerun_invokes_callback() {
        let pipeline = DevAssetPipeline::new();
        let dir = tempdir().unwrap();
        let css_calls = Arc::new(AtomicUsize::new(0));
        let css_calls_cb = css_calls.clone();
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(|_, _| Ok(vec![])),
            run_css: Some(Arc::new(move || {
                css_calls_cb.fetch_add(1, Ordering::SeqCst);
                Ok(true)
            })),
            run_islands: None,
            run_client_scripts: None,
            reload_renderer: None,
        };

        let plan = RebuildPlan {
            pages: PageSelection::none(),
            rerun_css: true,
            rerun_islands: false,
            rerun_client_scripts: false,
            renderer_fresh: false,
            ssr_reload_needed: false,
            prune_paths: vec![],
            triggers: vec![],
            content_narrowing: None,
        };

        let outcome = pipeline.apply(&plan, &ctx).unwrap();
        assert!(outcome.css_rerun);
        assert!(outcome.css_changed);
        assert_eq!(css_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn stale_output_is_pruned_when_extension_changes() {
        // Sub 49 acceptance criterion: when a page's output_path
        // changes between builds (e.g. frontmatter extension flipped
        // from "xml" to "rss"), the previous artifact must be
        // deleted so dist/ doesn't accumulate orphaned files.
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();

        // First build: emit dist/sitemap.xml.
        let first_render = vec![RenderedPage {
            page: pid("/p/sitemap.xml.tsx"),
            output_path: RelDistPath::new("sitemap.xml").unwrap(),
            html: "<urlset/>".into(),
            content_type: Some("application/xml".into()),
        }];
        let calls_a = Arc::new(AtomicUsize::new(0));
        let ctx_a = ctx_with_renderer(dir.path().to_path_buf(), first_render, calls_a);
        let mut sel = BTreeSet::new();
        sel.insert(pid("/p/sitemap.xml.tsx"));
        let plan = RebuildPlan {
            pages: PageSelection::Specific(sel.clone()),
            rerun_css: false,
            rerun_islands: false,
            rerun_client_scripts: false,
            renderer_fresh: false,
            ssr_reload_needed: false,
            prune_paths: vec![],
            triggers: vec![],
            content_narrowing: None,
        };
        let first = pipeline.apply(&plan, &ctx_a).unwrap();
        assert_eq!(first.pages_written.len(), 1);
        assert!(
            first.pages_pruned.is_empty(),
            "first build has nothing to prune"
        );
        assert!(dir.path().join("sitemap.xml").exists());

        // Second build: same page, but output_path flipped to
        // sitemap.rss. The pipeline must delete the stale .xml.
        let second_render = vec![RenderedPage {
            page: pid("/p/sitemap.xml.tsx"),
            output_path: RelDistPath::new("sitemap.rss").unwrap(),
            html: "<rss/>".into(),
            content_type: Some("application/rss+xml".into()),
        }];
        let calls_b = Arc::new(AtomicUsize::new(0));
        let ctx_b = ctx_with_renderer(dir.path().to_path_buf(), second_render, calls_b);
        let second = pipeline.apply(&plan, &ctx_b).unwrap();

        assert_eq!(second.pages_written.len(), 1, "new artifact written");
        assert_eq!(
            second.pages_pruned.len(),
            1,
            "old artifact reported as pruned"
        );
        assert_eq!(second.pages_pruned[0], dir.path().join("sitemap.xml"));
        assert!(
            !dir.path().join("sitemap.xml").exists(),
            "stale sitemap.xml must be removed",
        );
        assert!(
            dir.path().join("sitemap.rss").exists(),
            "fresh sitemap.rss must exist",
        );
    }

    #[test]
    fn unchanged_output_path_does_not_prune() {
        // Re-rendering the same page to the same path is the common
        // case and must not produce any prune entries.
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let render = vec![RenderedPage {
            page: pid("/p/a.tsx"),
            output_path: RelDistPath::new("a/index.html").unwrap(),
            html: "<h1>A</h1>".into(),
            content_type: None,
        }];
        let ctx = ctx_with_renderer(
            dir.path().to_path_buf(),
            render.clone(),
            Arc::new(AtomicUsize::new(0)),
        );
        let mut sel = BTreeSet::new();
        sel.insert(pid("/p/a.tsx"));
        let plan = RebuildPlan {
            pages: PageSelection::Specific(sel),
            rerun_css: false,
            rerun_islands: false,
            rerun_client_scripts: false,
            renderer_fresh: false,
            ssr_reload_needed: false,
            prune_paths: vec![],
            triggers: vec![],
            content_narrowing: None,
        };
        let first = pipeline.apply(&plan, &ctx).unwrap();
        let second = pipeline.apply(&plan, &ctx).unwrap();
        assert!(first.pages_pruned.is_empty());
        assert!(second.pages_pruned.is_empty());
    }

    #[test]
    fn stale_output_prune_skipped_when_sibling_claims_path() {
        // Regression test for #727: when two pages swap output paths in
        // a single tick the deferred prune must not delete the path that
        // a sibling just claimed.
        //
        // Topology:
        //   Tick 1: A → shared.html, B → b.html
        //   Tick 2: A → a.html,      B → shared.html  (B claims A's old path)
        //
        // Expected after tick 2:
        //   - shared.html still exists with B's content (not deleted)
        //   - a.html exists with A's content
        //   - b.html is gone (legitimately pruned — nothing claims it)
        //   - pages_pruned does NOT contain shared.html
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();

        // -- Tick 1 ----------------------------------------------------------
        let tick1 = vec![
            RenderedPage {
                page: pid("/p/a.tsx"),
                output_path: RelDistPath::new("shared.html").unwrap(),
                html: "<p>A tick1</p>".into(),
                content_type: None,
            },
            RenderedPage {
                page: pid("/p/b.tsx"),
                output_path: RelDistPath::new("b.html").unwrap(),
                html: "<p>B tick1</p>".into(),
                content_type: None,
            },
        ];
        let ctx1 = ctx_with_renderer(
            dir.path().to_path_buf(),
            tick1,
            Arc::new(AtomicUsize::new(0)),
        );
        let mut sel = BTreeSet::new();
        sel.insert(pid("/p/a.tsx"));
        sel.insert(pid("/p/b.tsx"));
        let plan = RebuildPlan {
            pages: PageSelection::Specific(sel),
            rerun_css: false,
            rerun_islands: false,
            rerun_client_scripts: false,
            renderer_fresh: false,
            ssr_reload_needed: false,
            prune_paths: vec![],
            triggers: vec![],
            content_narrowing: None,
        };
        let first = pipeline.apply(&plan, &ctx1).unwrap();
        assert_eq!(first.pages_written.len(), 2);
        assert!(first.pages_pruned.is_empty(), "tick 1 has nothing to prune");
        assert!(dir.path().join("shared.html").exists());
        assert!(dir.path().join("b.html").exists());

        // -- Tick 2: B claims A's previous path ------------------------------
        let tick2 = vec![
            RenderedPage {
                page: pid("/p/a.tsx"),
                output_path: RelDistPath::new("a.html").unwrap(),
                html: "<p>A tick2</p>".into(),
                content_type: None,
            },
            RenderedPage {
                page: pid("/p/b.tsx"),
                output_path: RelDistPath::new("shared.html").unwrap(),
                html: "<p>B tick2</p>".into(),
                content_type: None,
            },
        ];
        let ctx2 = ctx_with_renderer(
            dir.path().to_path_buf(),
            tick2,
            Arc::new(AtomicUsize::new(0)),
        );
        let second = pipeline.apply(&plan, &ctx2).unwrap();

        // shared.html is claimed by B — must NOT be in pages_pruned.
        assert!(
            !second
                .pages_pruned
                .contains(&dir.path().join("shared.html")),
            "shared.html is a live dest for B — must not be pruned; pages_pruned={:?}",
            second.pages_pruned,
        );
        // shared.html must still exist with B's tick-2 content.
        assert!(
            dir.path().join("shared.html").exists(),
            "shared.html must still exist after tick 2",
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("shared.html")).unwrap(),
            "<p>B tick2</p>",
        );
        // a.html must exist with A's tick-2 content.
        assert!(
            dir.path().join("a.html").exists(),
            "a.html must exist after A moves to it",
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.html")).unwrap(),
            "<p>A tick2</p>",
        );
        // b.html was abandoned by B with nothing claiming it — it should
        // be pruned (this exercises the normal stale-prune path).
        assert!(
            !dir.path().join("b.html").exists(),
            "b.html was abandoned by B and should be pruned",
        );
        assert!(
            second.pages_pruned.contains(&dir.path().join("b.html")),
            "b.html must appear in pages_pruned",
        );
    }

    #[test]
    fn all_must_be_resolved_before_reaching_pipeline() {
        let pipeline = DevAssetPipeline::new();
        let dir = tempdir().unwrap();
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(|_, _| Ok(vec![])),
            run_css: None,
            run_islands: None,
            run_client_scripts: None,
            reload_renderer: None,
        };
        let plan = RebuildPlan {
            pages: PageSelection::All,
            rerun_css: false,
            rerun_islands: false,
            rerun_client_scripts: false,
            renderer_fresh: false,
            ssr_reload_needed: false,
            prune_paths: vec![],
            triggers: vec![],
            content_narrowing: None,
        };
        assert!(pipeline.apply(&plan, &ctx).is_err());
    }

    #[test]
    fn dev_pipeline_does_not_emit_hashed_asset_urls() {
        // Contract: DevAssetPipeline never populates
        // BuildOutcome::hashed_asset_urls. That field is reserved for
        // ProductionAssetPipeline so callers can distinguish "did the
        // production hashing run?" from "no hashing happened".
        let pipeline = DevAssetPipeline::new();
        let dir = tempdir().unwrap();
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(|_, _| Ok(vec![])),
            run_css: Some(Arc::new(|| Ok(true))),
            run_islands: Some(Arc::new(|| {
                Ok(Some(crate::pipeline::IslandsBundleInfo {
                    changed: true,
                    bundle_url: "/assets/islands-test.js".into(),
                    components: vec![],
                }))
            })),
            run_client_scripts: None,
            reload_renderer: None,
        };
        let plan = RebuildPlan {
            pages: PageSelection::none(),
            rerun_css: true,
            rerun_islands: true,
            rerun_client_scripts: false,
            renderer_fresh: false,
            ssr_reload_needed: false,
            prune_paths: vec![],
            triggers: vec![],
            content_narrowing: None,
        };
        let outcome = pipeline.apply(&plan, &ctx).unwrap();
        assert!(
            outcome.hashed_asset_urls.is_empty(),
            "dev pipeline must leave hashed_asset_urls empty; got {:?}",
            outcome.hashed_asset_urls,
        );
    }

    // ── RefreshOutcome::Skipped — render-fan-out bypass (issue #956) ─────

    /// Context whose reload reports the given outcome on every tick and
    /// counts render invocations.
    fn ctx_with_reload_outcome(
        dist_root: PathBuf,
        rendered: Vec<RenderedPage>,
        render_calls: Arc<AtomicUsize>,
        outcome: RefreshOutcome,
    ) -> BuildContext {
        BuildContext {
            dist_root,
            render_pages: Arc::new(
                move |_pages: &[PageId], _: Option<&crate::ContentNarrowing>| {
                    render_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(rendered.clone())
                },
            ),
            run_css: None,
            run_islands: None,
            run_client_scripts: None,
            reload_renderer: Some(Arc::new(move || Ok(outcome.clone()))),
        }
    }

    fn single_page_plan() -> RebuildPlan {
        let mut sel = BTreeSet::new();
        sel.insert(pid("/p/a.tsx"));
        RebuildPlan {
            pages: PageSelection::Specific(sel),
            rerun_css: false,
            rerun_islands: false,
            rerun_client_scripts: false,
            renderer_fresh: false,
            ssr_reload_needed: false,
            prune_paths: vec![],
            triggers: vec![],
            content_narrowing: None,
        }
    }

    /// A `Skipped` reload still renders its selected pages (issue #1301 —
    /// the skip-key carries no page selection, so bypassing render would
    /// drop a later tick's route staleness). Here the page re-renders to the
    /// SAME `output_path`, so there is no per-page stale candidate: the only
    /// pruning that could fire is the route-vanished half, which is gated off
    /// on a skip — so nothing is pruned (an unrendered/unmoved URL must not be
    /// mistaken for a vanished one). Re-rendering the identical bundle
    /// re-emits identical HTML, absorbed by the `last_bytes` byte-dedup (no
    /// write on tick 2).
    ///
    /// Narrowed claim (issue #1317): "a skip prunes nothing" is only true for
    /// the route-vanished half AND when no page's `output_path` moved this
    /// tick. A same-`PageId` output-path MOVE on a skip IS now pruned — see
    /// `skipped_tick_prunes_moved_per_page_artifact_and_evicts_cache`.
    #[test]
    fn skipped_reload_renders_selection_but_prunes_no_vanished_routes() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let rendered = vec![RenderedPage {
            page: pid("/p/a.tsx"),
            output_path: RelDistPath::new("a/index.html").unwrap(),
            html: "<h1>A</h1>".into(),
            content_type: None,
        }];
        let plan = single_page_plan();

        // Tick 1: a real refresh renders and writes as usual.
        let calls1 = Arc::new(AtomicUsize::new(0));
        let ctx1 = ctx_with_reload_outcome(
            dir.path().to_path_buf(),
            rendered.clone(),
            calls1.clone(),
            RefreshOutcome::Refreshed {
                vanished: vec![],
                changed_sources: vec![],
            },
        );
        let first = pipeline.apply(&plan, &ctx1).unwrap();
        assert_eq!(first.pages_rendered, 1);
        assert!(dir.path().join("a/index.html").exists());

        // Tick 2: byte-identical bundle → Skipped. The render half still
        // runs (issue #1301), but the identical HTML is deduped (no write)
        // and nothing is pruned; the existing HTML stays on disk.
        let calls2 = Arc::new(AtomicUsize::new(0));
        let ctx2 = ctx_with_reload_outcome(
            dir.path().to_path_buf(),
            rendered,
            calls2.clone(),
            RefreshOutcome::Skipped,
        );
        let second = pipeline.apply(&plan, &ctx2).unwrap();
        assert_eq!(
            calls2.load(Ordering::SeqCst),
            1,
            "Skipped tick still renders its selected pages (issue #1301)"
        );
        assert_eq!(second.pages_rendered, 1, "selected page rendered on skip");
        assert!(
            second.pages_written.is_empty(),
            "identical HTML is byte-deduped — no write on skip"
        );
        assert!(
            second.pages_pruned.is_empty(),
            "no per-page move + route-vanished half gated off on skip → \
             nothing pruned (issue #1317); pruned={:?}",
            second.pages_pruned,
        );
        assert!(
            dir.path().join("a/index.html").exists(),
            "existing HTML must survive a skipped tick"
        );
    }

    /// Regression for issue #1317: a same-`PageId` output-path MOVE on a
    /// `RefreshOutcome::Skipped` tick must still prune the old artifact and
    /// evict its cache entry. The render+write half runs on a skip (#1301),
    /// so `write_page` advances `last_output_path` to the new path and returns
    /// the old one as a per-page stale candidate. Before the #1317 fix that
    /// candidate was dropped (the whole deferred-prune block was gated on
    /// `!renderer_refresh_skipped`), leaking the old file in dist/ and its
    /// `last_bytes` entry forever. The per-page prune half now runs on a skip.
    #[test]
    fn skipped_tick_prunes_moved_per_page_artifact_and_evicts_cache() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let x = dir.path().join("x/index.html");
        let y = dir.path().join("y/index.html");

        // Tick 1: real refresh, page P → X.
        let calls1 = Arc::new(AtomicUsize::new(0));
        let ctx1 = ctx_with_reload_outcome(
            dir.path().to_path_buf(),
            vec![RenderedPage {
                page: pid("/p/a.tsx"),
                output_path: RelDistPath::new("x/index.html").unwrap(),
                html: "<h1>X</h1>".into(),
                content_type: None,
            }],
            calls1,
            RefreshOutcome::Refreshed {
                vanished: vec![],
                changed_sources: vec![],
            },
        );
        let first = pipeline.apply(&single_page_plan(), &ctx1).unwrap();
        assert_eq!(first.pages_written, vec![pid("/p/a.tsx")]);
        assert!(x.exists(), "X written on tick 1");

        // Tick 2: byte-identical bundle → Skipped, but the SAME page P now
        // renders to Y (output_path moved while the skip-key stayed fixed).
        let calls2 = Arc::new(AtomicUsize::new(0));
        let ctx2 = ctx_with_reload_outcome(
            dir.path().to_path_buf(),
            vec![RenderedPage {
                page: pid("/p/a.tsx"),
                output_path: RelDistPath::new("y/index.html").unwrap(),
                html: "<h1>Y</h1>".into(),
                content_type: None,
            }],
            calls2,
            RefreshOutcome::Skipped,
        );
        let second = pipeline.apply(&single_page_plan(), &ctx2).unwrap();

        // Y is freshly written; X is pruned even though this was a skip.
        assert!(
            second.pages_written.contains(&pid("/p/a.tsx")),
            "moved page must be written to its new path Y; written={:?}",
            second.pages_written,
        );
        assert!(
            y.exists(),
            "new artifact Y must exist after the skipped move"
        );
        assert!(
            second.pages_pruned.contains(&x),
            "old artifact X must be pruned on the skipped move (issue #1317); \
             pruned={:?}",
            second.pages_pruned,
        );
        assert!(!x.exists(), "stale X must be removed from disk");

        // The #1317 cache-leak half: X must no longer sit in either cache.
        assert!(
            !pipeline
                .shared
                .cache
                .last_bytes
                .lock()
                .unwrap()
                .contains_key(&x),
            "pruned X must be evicted from last_bytes",
        );
        assert!(
            !pipeline
                .shared
                .cache
                .last_output_path
                .lock()
                .unwrap()
                .values()
                .any(|v| v == &x),
            "no page may still map to pruned X in last_output_path",
        );
    }

    /// A `Skipped` reload still runs the render half (issue #1301) as well
    /// as any CSS and islands requested by the plan.
    #[test]
    fn skipped_reload_still_runs_css_and_islands() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let render_calls = Arc::new(AtomicUsize::new(0));
        let render_calls_cb = render_calls.clone();
        let css_calls = Arc::new(AtomicUsize::new(0));
        let css_calls_cb = css_calls.clone();
        let islands_calls = Arc::new(AtomicUsize::new(0));
        let islands_calls_cb = islands_calls.clone();
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(
                move |_pages: &[PageId], _: Option<&crate::ContentNarrowing>| {
                    render_calls_cb.fetch_add(1, Ordering::SeqCst);
                    Ok(vec![])
                },
            ),
            run_css: Some(Arc::new(move || {
                css_calls_cb.fetch_add(1, Ordering::SeqCst);
                Ok(true)
            })),
            run_islands: Some(Arc::new(move || {
                islands_calls_cb.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            })),
            run_client_scripts: None,
            reload_renderer: Some(Arc::new(|| Ok(RefreshOutcome::Skipped))),
        };

        let mut plan = single_page_plan();
        plan.rerun_css = true;
        plan.rerun_islands = true;

        let outcome = pipeline.apply(&plan, &ctx).unwrap();
        assert_eq!(
            render_calls.load(Ordering::SeqCst),
            1,
            "render half still runs on a skipped tick (issue #1301)"
        );
        assert!(outcome.css_rerun, "CSS still reruns on a skipped tick");
        assert_eq!(css_calls.load(Ordering::SeqCst), 1);
        assert!(
            outcome.islands_rerun,
            "islands still rerun on a skipped tick"
        );
        assert_eq!(islands_calls.load(Ordering::SeqCst), 1);
    }

    /// Straddling-tick regression (issue #1301, fixes #1301): tick A
    /// produces a bundle selecting only route R1; tick B produces a
    /// BYTE-IDENTICAL bundle (`RefreshOutcome::Skipped`) but the incoming
    /// plan selects route R2 (the differing selection comes from the plan's
    /// `pages` — driven by the dirty set — NOT from the route table). R2
    /// must be rendered and written on tick B; before the fix the skip
    /// short-circuited the whole render fan-out and R2 stayed silently
    /// stale until the next real bundle change.
    #[test]
    fn skipped_tick_still_renders_differing_page_selection() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();

        // The render callback maps each selected PageId to its own output
        // path so the plan's selection genuinely drives what is rendered —
        // exactly the coupling the skip-key discards.
        fn render_by_selection(calls: Arc<AtomicUsize>) -> crate::pipeline::PageRenderer {
            Arc::new(move |pages: &[PageId], _| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(pages
                    .iter()
                    .map(|p| {
                        let name = p.path().file_stem().unwrap().to_string_lossy().into_owned();
                        RenderedPage {
                            page: p.clone(),
                            output_path: RelDistPath::new(format!("{name}/index.html")).unwrap(),
                            html: format!("<h1>{name}</h1>"),
                            content_type: None,
                        }
                    })
                    .collect())
            })
        }

        fn plan_selecting(page: &str) -> RebuildPlan {
            let mut sel = BTreeSet::new();
            sel.insert(pid(page));
            RebuildPlan {
                pages: PageSelection::Specific(sel),
                rerun_css: false,
                rerun_islands: false,
                rerun_client_scripts: false,
                renderer_fresh: false,
                ssr_reload_needed: false,
                prune_paths: vec![],
                triggers: vec![],
                content_narrowing: None,
            }
        }

        // Tick A: real refresh, selection = R1 only.
        let calls_a = Arc::new(AtomicUsize::new(0));
        let ctx_a = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: render_by_selection(calls_a.clone()),
            run_css: None,
            run_islands: None,
            run_client_scripts: None,
            reload_renderer: Some(Arc::new(|| {
                Ok(RefreshOutcome::Refreshed {
                    vanished: vec![],
                    changed_sources: vec![],
                })
            })),
        };
        let plan_a = plan_selecting("/p/r1.tsx");
        let a = pipeline.apply(&plan_a, &ctx_a).unwrap();
        assert_eq!(a.pages_rendered, 1);
        assert!(dir.path().join("r1/index.html").exists());
        assert!(
            !dir.path().join("r2/index.html").exists(),
            "R2 was not selected on tick A"
        );

        // Tick B: byte-identical bundle → Skipped; selection = R2 only.
        let calls_b = Arc::new(AtomicUsize::new(0));
        let ctx_b = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: render_by_selection(calls_b.clone()),
            run_css: None,
            run_islands: None,
            run_client_scripts: None,
            reload_renderer: Some(Arc::new(|| Ok(RefreshOutcome::Skipped))),
        };
        let plan_b = plan_selecting("/p/r2.tsx");
        let b = pipeline.apply(&plan_b, &ctx_b).unwrap();

        assert_eq!(
            calls_b.load(Ordering::SeqCst),
            1,
            "a skipped tick must still render its (differing) selection"
        );
        assert_eq!(b.pages_rendered, 1, "R2 rendered on the skipped tick");
        assert!(
            b.pages_written.contains(&pid("/p/r2.tsx")),
            "R2 must be freshly written on tick B, not left stale; written={:?}",
            b.pages_written,
        );
        assert!(
            dir.path().join("r2/index.html").exists(),
            "R2 output must exist after the skipped tick (issue #1301)"
        );
        // Prune stays gated off on a skip — R1 must survive untouched.
        assert!(b.pages_pruned.is_empty(), "skip prunes nothing");
        assert!(
            dir.path().join("r1/index.html").exists(),
            "R1 must survive the skipped tick"
        );
    }

    /// Plan-carried prune paths (issue #804 P2) are still processed when
    /// the reload reports `Skipped`. Defensive: a discovery refresh sets
    /// `renderer_fresh` so the combination cannot occur in production
    /// wiring, but the pipeline contract is "prune paths always honoured".
    #[test]
    fn skipped_reload_still_processes_plan_prune_paths() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let stale = dir.path().join("stale.html");
        std::fs::write(&stale, "<p>stale</p>").unwrap();

        let calls = Arc::new(AtomicUsize::new(0));
        let ctx = ctx_with_reload_outcome(
            dir.path().to_path_buf(),
            vec![],
            calls.clone(),
            RefreshOutcome::Skipped,
        );
        let mut plan = single_page_plan();
        plan.prune_paths = vec![stale.clone()];

        let outcome = pipeline.apply(&plan, &ctx).unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "render half still runs on a skipped tick (issue #1301)"
        );
        assert!(!stale.exists(), "plan prune path must still be deleted");
        // Processed exactly once: the deferred-prune loop is gated off on a
        // skip, so the plan-prune block (1b) is the sole handler here.
        assert_eq!(
            outcome.pages_pruned.iter().filter(|p| **p == stale).count(),
            1,
            "plan prune path must be reported exactly once; got {:?}",
            outcome.pages_pruned,
        );
    }

    /// SSR-only tick (no SSG pages, `ssr_reload_needed`): a `Skipped`
    /// reload leaves the dist tree untouched — nothing vanished because
    /// the route table did not move.
    #[test]
    fn ssr_only_skipped_reload_prunes_nothing() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let existing = dir.path().join("page.html");
        std::fs::write(&existing, "<p>live</p>").unwrap();

        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(|_, _| Ok(vec![])),
            run_css: None,
            run_islands: None,
            run_client_scripts: None,
            reload_renderer: Some(Arc::new(|| Ok(RefreshOutcome::Skipped))),
        };
        let mut plan = single_page_plan();
        plan.pages = PageSelection::none();
        plan.ssr_reload_needed = true;

        let outcome = pipeline.apply(&plan, &ctx).unwrap();
        assert!(outcome.pages_pruned.is_empty(), "nothing vanished on skip");
        assert!(existing.exists(), "existing output must survive");
    }

    /// A `Refreshed` reload (even with nothing vanished) renders as
    /// before — the skip bypass only fires on the explicit `Skipped`
    /// state.
    #[test]
    fn refreshed_reload_renders_as_before() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let ctx = ctx_with_reload_outcome(
            dir.path().to_path_buf(),
            vec![RenderedPage {
                page: pid("/p/a.tsx"),
                output_path: RelDistPath::new("a/index.html").unwrap(),
                html: "<h1>A</h1>".into(),
                content_type: None,
            }],
            calls.clone(),
            RefreshOutcome::Refreshed {
                vanished: vec![],
                changed_sources: vec![],
            },
        );
        let outcome = pipeline.apply(&single_page_plan(), &ctx).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "render ran");
        assert_eq!(outcome.pages_rendered, 1);
        assert!(dir.path().join("a/index.html").exists());
    }

    // ── Content-narrowing hint forwarding (issue #958) ───────────────────

    /// Context whose render callback records the narrowing hint it
    /// received and whose reloader reports the given outcome.
    fn ctx_recording_narrowing(
        dist_root: PathBuf,
        outcome: RefreshOutcome,
        seen: Arc<Mutex<Vec<Option<crate::ContentNarrowing>>>>,
    ) -> BuildContext {
        BuildContext {
            dist_root,
            render_pages: Arc::new(
                move |_pages: &[PageId], narrowing: Option<&crate::ContentNarrowing>| {
                    seen.lock().unwrap().push(narrowing.cloned());
                    Ok(vec![])
                },
            ),
            run_css: None,
            run_islands: None,
            run_client_scripts: None,
            reload_renderer: Some(Arc::new(move || Ok(outcome.clone()))),
        }
    }

    fn narrowed_plan() -> RebuildPlan {
        let mut plan = single_page_plan();
        plan.content_narrowing = Some(crate::ContentNarrowing {
            changed_content: vec![PathBuf::from("/proj/content/post.md")],
            fan_out_safe: true,
        });
        plan
    }

    /// Happy path: a plan-carried narrowing hint reaches the render
    /// callback verbatim when the refresh reports no structural change.
    #[test]
    fn narrowing_hint_reaches_render_callback() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let ctx = ctx_recording_narrowing(
            dir.path().to_path_buf(),
            RefreshOutcome::Refreshed {
                vanished: vec![],
                changed_sources: vec![],
            },
            seen.clone(),
        );
        pipeline.apply(&narrowed_plan(), &ctx).unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "render callback invoked once");
        assert_eq!(
            seen[0],
            Some(crate::ContentNarrowing {
                changed_content: vec![PathBuf::from("/proj/content/post.md")],
                fan_out_safe: true,
            }),
            "the hint must be forwarded when the route table did not move"
        );
    }

    /// Helper: the narrowed hint with `fan_out_safe` cleared — what G5/G6
    /// now forward (the eager fan-out is disabled, the lazy eager basis
    /// survives). See issue #1058.
    fn hint_with_fan_out_cleared() -> Option<crate::ContentNarrowing> {
        Some(crate::ContentNarrowing {
            changed_content: vec![PathBuf::from("/proj/content/post.md")],
            fan_out_safe: false,
        })
    }

    /// Fallback G5 (issues #958 / #1058): a refresh that reports changed
    /// source route sets clears `fan_out_safe` (the eager path renders the
    /// full fan-out) but PRESERVES the hint so the lazy path still
    /// eager-renders the edited entry's own route.
    #[test]
    fn refresh_changed_sources_clears_fan_out_safe() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let ctx = ctx_recording_narrowing(
            dir.path().to_path_buf(),
            RefreshOutcome::Refreshed {
                vanished: vec![],
                changed_sources: vec![pid("/p/blog/[slug].tsx")],
            },
            seen.clone(),
        );
        pipeline.apply(&narrowed_plan(), &ctx).unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            seen[0],
            hint_with_fan_out_cleared(),
            "non-empty changed_sources clears fan_out_safe but keeps the lazy basis (G5)"
        );
    }

    /// Fallback G5, vanished arm: globally-vanished routes also mean the
    /// route structure moved — clear `fan_out_safe`, keep the lazy basis.
    #[test]
    fn refresh_vanished_routes_clears_fan_out_safe() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let ctx = ctx_recording_narrowing(
            dir.path().to_path_buf(),
            RefreshOutcome::Refreshed {
                vanished: vec![dir.path().join("gone/index.html")],
                changed_sources: vec![],
            },
            seen.clone(),
        );
        pipeline.apply(&narrowed_plan(), &ctx).unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            seen[0],
            hint_with_fan_out_cleared(),
            "non-empty vanished clears fan_out_safe but keeps the lazy basis (G5)"
        );
    }

    /// Fallback G6 (issues #958 / #1058): a renderer-fresh tick (discovery
    /// regime) clears `fan_out_safe` — the eager path renders the full
    /// fan-out — but the lazy eager basis survives so a co-changed in-place
    /// content edit still eager-renders its own route.
    #[test]
    fn renderer_fresh_tick_clears_fan_out_safe() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let ctx = ctx_recording_narrowing(
            dir.path().to_path_buf(),
            RefreshOutcome::Refreshed {
                vanished: vec![],
                changed_sources: vec![],
            },
            seen.clone(),
        );
        let mut plan = narrowed_plan();
        plan.renderer_fresh = true;
        pipeline.apply(&plan, &ctx).unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            seen[0],
            hint_with_fan_out_cleared(),
            "renderer_fresh clears fan_out_safe but keeps the lazy basis (G6)"
        );
    }

    /// Prune-safety on a narrowed tick (issue #958 §7 / #727): when the
    /// render callback returns only a SUBSET of the selected pages (the
    /// narrowed set), the sibling pages' on-disk HTML and pipeline
    /// bookkeeping (`last_bytes` / `last_output_path`) must survive
    /// untouched — an unrendered sibling URL is never treated as
    /// vanished, and a later full tick still byte-dedups it.
    #[test]
    fn narrowed_tick_leaves_sibling_outputs_and_bookkeeping_intact() {
        let dir = tempdir().unwrap();
        let pipeline = DevAssetPipeline::new();

        let page_a = RenderedPage {
            page: pid("blog/a/index.html"),
            output_path: RelDistPath::new("blog/a/index.html").unwrap(),
            html: "<p>A v1</p>".into(),
            content_type: None,
        };
        let page_b = RenderedPage {
            page: pid("blog/b/index.html"),
            output_path: RelDistPath::new("blog/b/index.html").unwrap(),
            html: "<p>B v1</p>".into(),
            content_type: None,
        };

        let mut sel = BTreeSet::new();
        sel.insert(pid("/p/blog/[slug].tsx"));
        let mut plan = RebuildPlan::empty();
        plan.pages = PageSelection::Specific(sel);

        // Tick 1: full fan-out renders A and B.
        let full = vec![page_a.clone(), page_b.clone()];
        let ctx1 = ctx_with_renderer(
            dir.path().to_path_buf(),
            full,
            Arc::new(AtomicUsize::new(0)),
        );
        let first = pipeline.apply(&plan, &ctx1).unwrap();
        assert_eq!(first.pages_written.len(), 2);
        assert!(dir.path().join("blog/b/index.html").exists());

        // Tick 2: narrowed — the callback returns ONLY A (new bytes).
        let narrowed = vec![RenderedPage {
            html: "<p>A v2</p>".into(),
            ..page_a.clone()
        }];
        let ctx2 = ctx_with_renderer(
            dir.path().to_path_buf(),
            narrowed,
            Arc::new(AtomicUsize::new(0)),
        );
        let second = pipeline.apply(&plan, &ctx2).unwrap();
        assert_eq!(second.pages_written, vec![page_a.page.clone()]);
        assert!(
            second.pages_pruned.is_empty(),
            "an unrendered sibling must never be treated as vanished; pruned={:?}",
            second.pages_pruned,
        );
        assert!(
            dir.path().join("blog/b/index.html").exists(),
            "sibling HTML must survive a narrowed tick"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("blog/b/index.html")).unwrap(),
            "<p>B v1</p>",
        );

        // Tick 3: full fan-out again with byte-identical content — the
        // sibling's `last_bytes` slot survived the narrowed tick, so B
        // byte-dedups (not re-written); A's v2 bytes also dedup.
        let full_again = vec![
            RenderedPage {
                html: "<p>A v2</p>".into(),
                ..page_a
            },
            page_b,
        ];
        let ctx3 = ctx_with_renderer(
            dir.path().to_path_buf(),
            full_again,
            Arc::new(AtomicUsize::new(0)),
        );
        let third = pipeline.apply(&plan, &ctx3).unwrap();
        assert!(
            third.pages_written.is_empty(),
            "bookkeeping intact: a later full tick byte-skips unchanged \
             siblings; written={:?}",
            third.pages_written,
        );
        assert!(third.pages_pruned.is_empty());
    }
}
