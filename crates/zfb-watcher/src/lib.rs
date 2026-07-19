//! `zfb-watcher` — dev-mode filesystem watcher for the zfb dev server.
//!
//! Wraps the [`notify`] crate's recommended watcher, normalises its many
//! event kinds into a small [`ChangeKind`] enum, and emits debounced
//! [`Change`] values on a `tokio::sync::mpsc` channel.
//!
//! ## Why this crate exists
//!
//! Editor "save" actions almost never produce a single FS event. Common
//! patterns:
//!
//! - `vim` saves via "write to swap → rename over original" → multiple
//!   `Create`/`Modify`/`Remove` events on the *same* path within ~5ms.
//! - `vscode` and friends emit several `Modify(Metadata)` /
//!   `Modify(Data(Content))` events back-to-back.
//! - Bulk operations like `git checkout` produce hundreds of events at once.
//!
//! Downstream consumers (the dev server's rebuild loop) want **one logical
//! change per path per burst**. So we coalesce events within a short
//! debounce window (default 50ms) and only emit once the path has been
//! quiet for that window.
//!
//! ## Design
//!
//! - One background `notify` watcher thread (started by `notify` itself).
//! - One async debouncer task: receives raw events on an unbounded
//!   `std::sync::mpsc` (sync, because that is what `notify` hands us) and
//!   forwards normalised `Change` values on a `tokio::sync::mpsc`.
//! - Per-path "last seen" timestamp + sleep-until-quiet logic gives us
//!   "burst → one event" semantics with a single small loop.
//! - Watching a non-existent path is allowed: notify rejects it, we log
//!   and move on. (We do NOT poll for re-appearance — the higher-level
//!   dev server is expected to restart the watcher when major project
//!   shape changes happen. Re-appearance of a sub-directory under an
//!   already-watched parent is handled automatically by recursive
//!   watching.)

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as NotifyWatcher};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

mod self_check;
pub use self_check::{check_liveness, LivenessOutcome, SelfCheckOpts};

/// Default debounce window. Chosen short enough to feel instant in dev
/// (~one frame at 60fps + a bit) but long enough to coalesce typical
/// editor save bursts on Linux/macOS/Windows.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(50);

/// Normalised filesystem change kind.
///
/// Collapses notify's many `EventKind` variants into the three things
/// downstream rebuild logic actually cares about. Anything we cannot
/// classify is treated as `Modified` — we'd rather rebuild unnecessarily
/// than miss a real change. Pure-read [`notify::event::EventKind::Access`]
/// variants (open, close-after-read) are dropped entirely; see `classify`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Created,
    Modified,
    Removed,
}

/// A debounced filesystem change.
#[derive(Debug, Clone)]
pub struct Change {
    pub path: PathBuf,
    pub kind: ChangeKind,
}

/// The watcher handle.
///
/// Holds the underlying `notify` watcher (dropping stops the OS watch)
/// and the JoinHandle for the debouncer task. Dropping the handle sends
/// a graceful shutdown signal to the debouncer so any pending events
/// flush to the receiver before the task exits. Drop detaches the
/// JoinHandle without aborting; the task exits on its own once it
/// sees the shutdown signal or the closed bridge channel.
///
/// Construct via [`Watcher::start`], which returns this handle plus the
/// receiver end of the `Change` channel.
pub struct Watcher {
    // Kept alive: dropping the notify watcher stops the OS-level watch.
    _notify: RecommendedWatcher,
    // Some(_) until Drop fires the shutdown signal.
    shutdown: Option<oneshot::Sender<()>>,
    // Detached on drop (JoinHandle dropped without abort); the task
    // exits itself after seeing the shutdown signal / closed bridge.
    debouncer: Option<JoinHandle<()>>,
    /// Recursive roots registered at boot. Dynamic file watches below can
    /// skip any parent already covered by one of these roots.
    watched_recursive_roots: BTreeSet<PathBuf>,
    /// Exact directories registered non-recursively for live dependencies
    /// discovered after boot.
    watched_dependency_dirs: BTreeSet<PathBuf>,
}

fn watch_aliases(path: &Path) -> impl Iterator<Item = PathBuf> {
    let canonical = path.canonicalize().ok();
    std::iter::once(path.to_path_buf()).chain(canonical)
}

impl Watcher {
    /// Start a watcher rooted at `project_root`, watching every existing
    /// path in `relative_paths` recursively, with the default 50ms debounce.
    ///
    /// Missing paths are skipped with a warn-level log — this is normal
    /// (e.g. `data/` may not exist on a fresh project).
    ///
    /// Returns `(handle, receiver)`. Drop the handle to stop watching.
    pub fn start<P, I, S>(
        project_root: P,
        relative_paths: I,
    ) -> notify::Result<(Self, mpsc::Receiver<Change>)>
    where
        P: AsRef<Path>,
        I: IntoIterator<Item = S>,
        S: AsRef<Path>,
    {
        Self::start_with_debounce(project_root, relative_paths, DEFAULT_DEBOUNCE)
    }

    /// Same as [`Watcher::start`] but with a configurable debounce window
    /// (mainly for tests).
    pub fn start_with_debounce<P, I, S>(
        project_root: P,
        relative_paths: I,
        debounce: Duration,
    ) -> notify::Result<(Self, mpsc::Receiver<Change>)>
    where
        P: AsRef<Path>,
        I: IntoIterator<Item = S>,
        S: AsRef<Path>,
    {
        Self::start_with_extras(
            project_root,
            relative_paths,
            std::iter::empty::<&Path>(),
            debounce,
        )
    }

    /// Variant of [`Watcher::start_with_debounce`] that additionally
    /// registers each path in `extra_absolute_paths` verbatim (no join
    /// against `project_root`).
    ///
    /// Each extra path:
    ///
    /// - MUST be absolute. The caller is responsible for verifying this
    ///   (the config loader does so at parse time; the dev command
    ///   layer canonicalises before reaching this fn). A non-absolute
    ///   path here is a programming error — `notify` will still try to
    ///   watch it relative to the process cwd, but events will be hard
    ///   to interpret downstream.
    /// - is watched recursively.
    /// - skipped-with-warning if it does not exist at boot. The watcher
    ///   does NOT poll for the path appearing later (consistent with
    ///   how the in-tree relative paths are handled — see the
    ///   module-level docs).
    pub fn start_with_extras<P, I, S, J, T>(
        project_root: P,
        relative_paths: I,
        extra_absolute_paths: J,
        debounce: Duration,
    ) -> notify::Result<(Self, mpsc::Receiver<Change>)>
    where
        P: AsRef<Path>,
        I: IntoIterator<Item = S>,
        S: AsRef<Path>,
        J: IntoIterator<Item = T>,
        T: AsRef<Path>,
    {
        let root = project_root.as_ref();

        // notify hands us events on a sync channel from a thread it owns.
        let (raw_tx, raw_rx) = std_mpsc::channel::<notify::Result<Event>>();
        let mut notify_watcher = notify::recommended_watcher(move |res| {
            // Best-effort send; if the receiver is gone the watcher is
            // shutting down and we should quietly stop pushing.
            let _ = raw_tx.send(res);
        })?;
        let mut watched_recursive_roots = BTreeSet::new();

        for rel in relative_paths {
            let full = root.join(rel.as_ref());
            if !full.exists() {
                warn!(path = %full.display(), "watch target missing; skipping");
                continue;
            }
            if let Err(e) = notify_watcher.watch(&full, RecursiveMode::Recursive) {
                warn!(path = %full.display(), error = %e, "failed to watch path");
            } else {
                watched_recursive_roots.extend(watch_aliases(&full));
                debug!(path = %full.display(), "watching");
            }
        }

        // Extra absolute paths — registered as-is, no `project_root.join(...)`.
        // The caller (dev command layer) canonicalises before handing
        // paths in here, so `notify` and downstream classifiers see the
        // same form across boot + every later event.
        for extra in extra_absolute_paths {
            let extra = extra.as_ref();
            if !extra.exists() {
                warn!(
                    path = %extra.display(),
                    "extra watch target missing at boot; skipping (will not be re-watched if it appears later)"
                );
                continue;
            }
            if let Err(e) = notify_watcher.watch(extra, RecursiveMode::Recursive) {
                warn!(path = %extra.display(), error = %e, "failed to watch extra path");
            } else {
                watched_recursive_roots.extend(watch_aliases(extra));
                debug!(path = %extra.display(), "watching extra path");
            }
        }

        // Outbound channel: bounded but generous. 256 should comfortably
        // absorb a `git checkout` burst that survived debouncing.
        let (out_tx, out_rx) = mpsc::channel::<Change>(256);

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let debouncer = tokio::spawn(debouncer_task(raw_rx, out_tx, debounce, shutdown_rx));

        Ok((
            Self {
                _notify: notify_watcher,
                shutdown: Some(shutdown_tx),
                debouncer: Some(debouncer),
                watched_recursive_roots,
                watched_dependency_dirs: BTreeSet::new(),
            },
            out_rx,
        ))
    }

    /// Add non-recursive watches for the parent directories of absolute
    /// dependency files discovered after the watcher started, returning the
    /// parent directories NEWLY watched by this call (deduped; empty when
    /// every parent was already covered).
    ///
    /// Watching the parent instead of the file preserves delete/recreate
    /// visibility. Non-recursive mode avoids broadening a root-level
    /// dependency into a recursive watch of the entire project (including
    /// generated output) — and, because a first-party dependency file's
    /// parent is never a `node_modules` directory, it is the `node_modules`-
    /// safe way to follow a workspace-sibling source directory (issue #1678):
    /// the returned directories are exactly the out-of-recursive-root
    /// dependency parents (client-script `?raw`/module-worker sibling targets
    /// among them) that the caller may surface as an observable signal.
    /// Existing boot roots and previously-added parents are deduplicated.
    /// Per-path failures are warned and skipped, matching the boot
    /// registration policy.
    pub fn watch_additional_files<I, P>(&mut self, paths: I) -> Vec<PathBuf>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut newly_watched = Vec::new();
        for path in paths {
            let path = path.as_ref();
            if !path.is_absolute() {
                warn!(path = %path.display(), "dynamic watch dependency is not absolute; skipping");
                continue;
            }
            let Some(parent) = path.parent() else {
                continue;
            };
            if !parent.exists() {
                warn!(path = %parent.display(), "dynamic watch dependency parent missing; skipping");
                continue;
            }

            let aliases: Vec<PathBuf> = watch_aliases(parent).collect();
            let recursively_covered = aliases.iter().any(|alias| {
                self.watched_recursive_roots
                    .iter()
                    .any(|root| alias.starts_with(root))
            });
            if recursively_covered
                || aliases
                    .iter()
                    .any(|alias| self.watched_dependency_dirs.contains(alias))
            {
                continue;
            }

            if let Err(error) = self._notify.watch(parent, RecursiveMode::NonRecursive) {
                warn!(path = %parent.display(), %error, "failed to watch dynamic dependency parent");
            } else {
                self.watched_dependency_dirs.extend(aliases);
                debug!(path = %parent.display(), "watching dynamic dependency parent");
                newly_watched.push(parent.to_path_buf());
            }
        }
        newly_watched
    }
}

impl Watcher {
    /// Shut down the watcher gracefully and wait for the debouncer task
    /// to finish flushing any pending events.
    ///
    /// This is the preferred shutdown path. It sends the shutdown signal
    /// to the debouncer, waits for the debouncer to flush all pending
    /// events and exit, then returns. The OS-level `notify` watcher is
    /// stopped when the `Watcher` value is dropped at the call-site.
    ///
    /// Callers that cannot await (e.g. in a `Drop` impl) may simply drop
    /// the `Watcher` value instead — the `Drop` impl sends the shutdown
    /// signal as a best-effort fallback, but cannot await the flush.
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.debouncer.take() {
            // Ignore join errors (task already exited or was cancelled).
            let _ = handle.await;
        }
        // `_notify` is dropped here, stopping the OS-level watch.
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        // Best-effort graceful shutdown: signal the debouncer so it can
        // run its `flush_all` branch. We deliberately do NOT call
        // `abort()` here — that would race the shutdown signal and
        // cancel the task before it could flush. Instead, we drop the
        // JoinHandle which detaches the task; once shutdown fires (or
        // the bridge channel closes when `_notify` is dropped right
        // after this in field-drop order), the task will flush and exit
        // on its own. If the runtime itself is shutting down, the task
        // will be cancelled by the runtime; that's the unavoidable case
        // where `flush_all` cannot run.
        //
        // Prefer the async [`Watcher::shutdown`] method over relying on
        // `Drop` — `Drop` cannot await the flush so pending events may
        // be lost when the Tokio runtime is shutting down.
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        // Detach the task by dropping its JoinHandle without aborting.
        // The task will see the shutdown signal (or the closed bridge
        // channel) and run `flush_all` before exiting.
        let _ = self.debouncer.take();
    }
}

/// Map a notify `EventKind` to our small enum. Unknown / "Any" / "Other"
/// kinds collapse to `Modified` so we never silently drop a real change.
///
/// Returns `None` for pure-read access events (open, close-after-read) that
/// carry no mutation signal. On Linux inotify the [`notify`] crate maps
/// `IN_OPEN` and `IN_CLOSE_NOWRITE` to [`EventKind::Access`] variants; if we
/// treated those as `Modified`, every file read inside a tick would schedule
/// another tick — an infinite churn loop that also poisons B3 content-edit
/// narrowing by flooding `changed_content` with unrelated files (issue #B4).
///
/// `Access(Close(Write))` (inotify `IN_CLOSE_WRITE`) IS a write-completion
/// event and is therefore kept as `Modified`.  Every other `Access` variant
/// (Open, Close(Read), Read, Any, Other) is a pure-read; we drop them.
fn classify(kind: &EventKind) -> Option<ChangeKind> {
    use notify::event::{AccessKind, AccessMode};
    match kind {
        EventKind::Create(_) => Some(ChangeKind::Created),
        EventKind::Remove(_) => Some(ChangeKind::Removed),
        EventKind::Modify(_) | EventKind::Any | EventKind::Other => Some(ChangeKind::Modified),
        // Write-close = a mutation completed; treat like Modify.
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => Some(ChangeKind::Modified),
        // Pure-read accesses: open, close-after-read, raw read, etc.
        // Drop them to prevent the inotify Access→tick churn loop.
        EventKind::Access(_) => None,
    }
}

/// Per-path pending state used by the debouncer.
struct Pending {
    kind: ChangeKind,
    last_seen: Instant,
}

/// Merge an incoming [`ChangeKind`] with whatever kind is already pending
/// for the same path within the debounce window.
///
/// Truth table (rows = pending, cols = incoming):
///
/// | pending \ incoming | Created | Modified | Removed |
/// |--------------------|---------|----------|---------|
/// | None               | Created | Modified | Removed |
/// | Created            | Created | Created  | Removed |
/// | Modified           | Created | Modified | Removed |
/// | Removed            | Created | Modified | Removed |
///
/// Rules:
///   - An incoming `Removed` always wins: the file is gone, so any preceding
///     Create/Modify is moot.
///   - `Created` followed by `Modified` stays `Created`: on macOS FSEvents
///     (and some Linux inotify setups) `fs::write` to a brand-new path fires
///     Create then one or more Modify events within the window. Collapsing to
///     `Modified` would drop the `Created` signal the watch-ADD discovery hook
///     relies on (`tick_with_kinds` only calls the hook for `Created`).
///   - A pending `Removed` followed by `Created`/`Modified` takes the incoming
///     kind: git's restore path (`git checkout --`, `git pull`, `git stash
///     pop`) unlinks then recreates a tracked file, so a Remove immediately
///     followed by a Create/Modify on the same path must surface as a real
///     change — NOT a stale `Removed` that `tick_with_kinds` would treat as a
///     deletion and skip from rebuild planning (issue #823). `Removed` is no
///     longer sticky.
///   - Otherwise take the incoming kind.
fn merge_kind(existing: Option<ChangeKind>, incoming: ChangeKind) -> ChangeKind {
    match existing {
        Some(ChangeKind::Created) if incoming == ChangeKind::Modified => ChangeKind::Created,
        _ => incoming,
    }
}

/// Resolve the kind to actually emit for a path at flush time, reconciling
/// the coalesced kind against the path's actual on-disk state.
///
/// On-disk existence is authoritative for the **Removed boundary** (it never
/// touches the Created vs Modified distinction — that is `merge_kind`'s job,
/// where the #660 discovery-hook fix lives):
///
/// - Pending `Removed` but the path is present → the file was restored, not
///   deleted: emit `Modified`. Covers platforms (e.g. macOS FSEvents under
///   load) that deliver a bare `Remove` for a git restore with no paired
///   `Create`, so the coalescer never got a Create/Modify to override it
///   (issue #823).
/// - Pending `Created`/`Modified` but the path is absent → it really is gone:
///   emit `Removed`. A backend can deliver `Remove` then a trailing
///   `Modify`-like event (name/metadata flags) for an already-deleted path,
///   which `merge_kind` coalesces to `Modified`; without this branch the
///   deletion would be lost and `tick_with_kinds` would skip its prune path.
/// - Otherwise pass the kind through unchanged.
///
/// The existence probe is authoritative only for a **definitive** answer.
/// A transient `Err` from `try_exists()` — e.g. `EIO`/`ENFILE` while an
/// arm64 macOS host is under heavy IO load (issue #1058) — proves nothing
/// about the file's state, so it must not be allowed to rewrite a write
/// event into a deletion. The reconciliation is split into the pure
/// [`reconcile_emit_kind`] so that transient-error branch is unit-testable
/// without provoking real filesystem IO errors.
fn resolve_emit_kind(path: &Path, kind: ChangeKind) -> ChangeKind {
    reconcile_emit_kind(kind, path.try_exists())
}

/// Pure existence-reconciliation step (see [`resolve_emit_kind`]). `exists`
/// is the raw `try_exists()` outcome: `Ok(true)` present, `Ok(false)`
/// genuinely absent, `Err` an IO error that tells us nothing.
///
/// - Incoming `Removed`: keep biasing to `Removed` on `Err` — we can least
///   afford to *drop* a delete, so an unknown state still surfaces as a
///   deletion (unchanged from the original behaviour). A definitive
///   `Ok(true)` means the file was restored → upgrade to `Modified` (#823).
/// - Incoming `Created`/`Modified`: a definitive `Ok(false)` still emits
///   `Removed` (a backend can deliver a trailing `Modify` for an
///   already-deleted path; without this the prune path is skipped). But on
///   a transient `Err`, PASS THE KIND THROUGH rather than downgrading to
///   `Removed` (issue #1058): the event source already told us the file was
///   written, and a phantom deletion would route a live content edit to the
///   prune regime and skip its eager re-render.
fn reconcile_emit_kind(kind: ChangeKind, exists: std::io::Result<bool>) -> ChangeKind {
    match (kind, exists) {
        // Removed boundary — a definitive on-disk answer is authoritative.
        // Accepted trade-off: a bare-Remove from a git-restore of a
        // brand-new route upgrades to Modified (never Created), so the
        // orchestrator's watch-ADD discovery hook does not fire for that
        // restored route on this tick.
        (ChangeKind::Removed, Ok(true)) => ChangeKind::Modified,
        (ChangeKind::Removed, Ok(false)) => ChangeKind::Removed,
        (ChangeKind::Removed, Err(_)) => ChangeKind::Removed,
        // Non-removed (Created/Modified): definitive "absent" → the file
        // really is gone, emit Removed so the prune path runs.
        (other, Ok(true)) => other,
        (_, Ok(false)) => ChangeKind::Removed,
        // Transient stat miss on a write/create event → do NOT downgrade.
        (other, Err(_)) => other,
    }
}

/// Force a flush once `pending` reaches this many distinct paths, even if
/// the `tick` arm has not had a chance to run. Under a sustained high-rate
/// event stream (e.g. `git checkout` of a huge tree, or a build tool
/// rewriting files in a loop) the `biased` `select!` keeps servicing the
/// `recv` arm and STARVES the `tick` arm, so `pending` would otherwise grow
/// without bound and never drain until the burst stops. The outbound
/// channel is bounded at 256; a threshold an order of magnitude below that
/// keeps memory bounded while still coalescing normal editor-save bursts
/// (which are a handful of paths) untouched.
const MAX_PENDING_BEFORE_FORCED_FLUSH: usize = 1024;

/// The debouncer loop. Runs on a tokio task.
///
/// Strategy: bridge the sync `notify` channel onto a tokio channel via
/// `spawn_blocking`, then loop with a small "wake every debounce/2"
/// timer that flushes any path whose last event is older than the
/// debounce window. This avoids per-path timer tasks (cheap) and gives
/// O(1) flush per tick.
///
/// Starvation / livelock guard: the `select!` is `biased`, so under a
/// continuous high-rate event stream the `recv` arm fires forever and the
/// `tick` arm never runs — `pending` grows unbounded and a continuously
/// touched "hot" file (whose `last_seen` is bumped on every event) never
/// satisfies the quiet-window drain condition (livelock). To bound both,
/// the `recv` arm forces an inline flush whenever `pending` exceeds
/// [`MAX_PENDING_BEFORE_FORCED_FLUSH`] OR the wall-time since the last
/// drain exceeds the debounce window. Neither check depends on the `tick`
/// arm getting scheduled, so a sustained burst always makes progress.
async fn debouncer_task(
    raw_rx: std_mpsc::Receiver<notify::Result<Event>>,
    out_tx: mpsc::Sender<Change>,
    debounce: Duration,
    mut shutdown: oneshot::Receiver<()>,
) {
    // Bridge sync→async. A small bounded buffer is fine: notify will
    // back up briefly under bursts but we drain quickly.
    let (bridge_tx, mut bridge_rx) = mpsc::channel::<notify::Result<Event>>(1024);
    let bridge = tokio::task::spawn_blocking(move || {
        while let Ok(msg) = raw_rx.recv() {
            // blocking_send is fine: we're on a blocking thread.
            if bridge_tx.blocking_send(msg).is_err() {
                break;
            }
        }
    });

    let mut pending: HashMap<PathBuf, Pending> = HashMap::new();
    // Wall-clock of the last time we drained ready entries (via the `tick`
    // arm or the recv-arm forced flush). Used by the recv arm to force a
    // flush when the `tick` arm is being starved by a continuous stream.
    let mut last_drain = Instant::now();
    // Wake at half the debounce window so worst-case extra latency is
    // ~debounce + debounce/2. With the default 50ms that's ~75ms.
    let mut tick = tokio::time::interval(debounce / 2);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            biased;

            // Shutdown signal from Drop (or the async shutdown() method):
            // flush whatever we have and exit. Drop only signals — it no
            // longer aborts the task — so this is the graceful-exit path.
            _ = &mut shutdown => {
                flush_all(&mut pending, &out_tx).await;
                // Do NOT fall through to `let _ = bridge.await;` below: the
                // bridge thread is parked in the synchronous `raw_rx.recv()`,
                // which closes only after `_notify` drops — and `_notify`
                // drops only when `Watcher::shutdown` RETURNS. Awaiting the
                // bridge here is a circular wait (issue #708). abort() cannot
                // cancel the parked thread, but it makes the JoinHandle return
                // immediately; the detached thread then exits once `_notify`
                // drops at the end of `shutdown()` and closes `raw_tx`.
                bridge.abort();
                return;
            }

            maybe_evt = bridge_rx.recv() => {
                let Some(res) = maybe_evt else {
                    // Bridge closed (notify watcher dropped). Flush
                    // anything still pending, then exit.
                    flush_all(&mut pending, &out_tx).await;
                    break;
                };
                match res {
                    Ok(evt) => {
                        let Some(kind) = classify(&evt.kind) else {
                            // Pure-read access event (e.g. IN_OPEN, IN_CLOSE_NOWRITE on
                            // Linux inotify): drop silently to avoid the churn loop.
                            continue;
                        };
                        let now = Instant::now();
                        for path in evt.paths {
                            // Coalesce the incoming kind with any already-pending
                            // kind for this path across the burst window. See
                            // `merge_kind` for the full truth table and rationale.
                            let merged_kind = merge_kind(pending.get(&path).map(|p| p.kind), kind);
                            pending.insert(path, Pending { kind: merged_kind, last_seen: now });
                        }
                        // Starvation / livelock guard (see fn docs). The `biased`
                        // `select!` lets this arm fire forever under a continuous
                        // stream, so we cannot rely on the `tick` arm to drain. Force
                        // a flush of ALL pending entries — not just the quiet ones —
                        // when either the map has grown too large OR a full debounce
                        // window has elapsed since the last drain. Draining all (not
                        // just `now - last_seen >= debounce`) is what breaks the
                        // hot-file livelock: a continuously touched path is never
                        // "quiet", so a quiet-only filter would never release it.
                        if pending.len() >= MAX_PENDING_BEFORE_FORCED_FLUSH
                            || now.duration_since(last_drain) >= debounce
                        {
                            last_drain = now;
                            if drain_all(&mut pending, &out_tx).await.is_err() {
                                // Receiver dropped; bail out of the loop.
                                bridge.abort();
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "notify watcher error");
                    }
                }
            }

            _ = tick.tick() => {
                let now = Instant::now();
                // Drain entries whose last event is older than `debounce`.
                let ready: Vec<(PathBuf, ChangeKind)> = pending
                    .iter()
                    .filter(|(_, p)| now.duration_since(p.last_seen) >= debounce)
                    .map(|(k, p)| (k.clone(), p.kind))
                    .collect();
                for (path, _) in &ready {
                    pending.remove(path);
                }
                last_drain = now;
                for (path, kind) in ready {
                    let kind = resolve_emit_kind(&path, kind);
                    if out_tx.send(Change { path, kind }).await.is_err() {
                        // Receiver dropped; bail out of the loop.
                        bridge.abort();
                        return;
                    }
                }
            }
        }
    }

    // bridge task exits naturally when raw_rx closes.
    let _ = bridge.await;
}

async fn flush_all(pending: &mut HashMap<PathBuf, Pending>, out_tx: &mpsc::Sender<Change>) {
    let _ = drain_all(pending, out_tx).await;
}

/// Drain and emit **every** pending entry, regardless of how recently it was
/// last seen. Used by the recv-arm starvation/livelock guard (and, via
/// [`flush_all`], by the shutdown / bridge-closed paths). Returns `Err(())`
/// if the outbound receiver was dropped mid-drain so the caller can bail.
async fn drain_all(
    pending: &mut HashMap<PathBuf, Pending>,
    out_tx: &mpsc::Sender<Change>,
) -> Result<(), ()> {
    for (path, p) in pending.drain() {
        let kind = resolve_emit_kind(&path, p.kind);
        if out_tx.send(Change { path, kind }).await.is_err() {
            return Err(());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn classify_create_is_created() {
        use notify::event::{CreateKind, EventKind};
        assert_eq!(
            classify(&EventKind::Create(CreateKind::File)),
            Some(ChangeKind::Created)
        );
    }

    #[test]
    fn classify_remove_is_removed() {
        use notify::event::{EventKind, RemoveKind};
        assert_eq!(
            classify(&EventKind::Remove(RemoveKind::File)),
            Some(ChangeKind::Removed)
        );
    }

    #[test]
    fn classify_unknown_is_modified() {
        use notify::event::EventKind;
        assert_eq!(classify(&EventKind::Any), Some(ChangeKind::Modified));
        assert_eq!(classify(&EventKind::Other), Some(ChangeKind::Modified));
    }

    #[test]
    fn classify_access_close_write_is_modified() {
        use notify::event::{AccessKind, AccessMode, EventKind};
        // IN_CLOSE_WRITE: a file was written and closed — treat as Modified.
        assert_eq!(
            classify(&EventKind::Access(AccessKind::Close(AccessMode::Write))),
            Some(ChangeKind::Modified),
            "IN_CLOSE_WRITE must surface as Modified",
        );
    }

    #[test]
    fn classify_pure_read_access_is_none() {
        use notify::event::{AccessKind, AccessMode, EventKind};
        // IN_OPEN: file opened for reading — must be dropped to prevent churn loop.
        assert_eq!(
            classify(&EventKind::Access(AccessKind::Open(AccessMode::Any))),
            None,
            "IN_OPEN must be dropped (pure read)",
        );
        // IN_CLOSE_NOWRITE: file closed after read-only access — must be dropped.
        assert_eq!(
            classify(&EventKind::Access(AccessKind::Close(AccessMode::Read))),
            None,
            "IN_CLOSE_NOWRITE must be dropped (pure read)",
        );
        // AccessKind::Read, Any, Other — pure read variants; also dropped.
        assert_eq!(classify(&EventKind::Access(AccessKind::Any)), None);
        assert_eq!(classify(&EventKind::Access(AccessKind::Other)), None);
    }

    // ---------------------------------------------------------------------------
    // Burst-coalescing merge tests for `merge_kind` (the production helper).
    //
    // Two coalescing rules are exercised here:
    //   - The "sticky Created" fix (issue #660): `fs::write` to a brand-new
    //     path fires Create then one or more Modify events within the debounce
    //     window. Collapsing the burst to `Modified` would drop the `Created`
    //     signal the watch-ADD discovery hook relies on, so `(Created,
    //     Modified) → Created`.
    //   - The git-restore fix (issue #823): git's restore path unlinks then
    //     recreates a tracked file. A pending `Removed` must NOT be sticky —
    //     a subsequent Create/Modify on the same path overrides it, so the
    //     restored file surfaces as a real change rather than a stale deletion.
    // ---------------------------------------------------------------------------

    #[test]
    fn created_then_modified_stays_created() {
        // A new file write on macOS fires Create→Modify; the downstream
        // discovery hook requires Created.
        assert_eq!(
            merge_kind(Some(ChangeKind::Created), ChangeKind::Modified),
            ChangeKind::Created,
            "Created must survive a subsequent Modified in the same burst",
        );
    }

    #[test]
    fn created_then_removed_becomes_removed() {
        // Created then immediately Removed: file is gone; Removed wins.
        assert_eq!(
            merge_kind(Some(ChangeKind::Created), ChangeKind::Removed),
            ChangeKind::Removed,
        );
    }

    #[test]
    fn modified_then_removed_becomes_removed() {
        assert_eq!(
            merge_kind(Some(ChangeKind::Modified), ChangeKind::Removed),
            ChangeKind::Removed,
        );
    }

    #[test]
    fn removed_then_recreate_overrides() {
        // Issue #823: a git restore (unlink + recreate) lands as Remove→Create
        // (or Remove→Modify) on the same path within the debounce window. The
        // recreate MUST override the pending Removed so the page rebuilds
        // instead of being treated as a deletion. A repeated Removed stays
        // Removed (a genuine delete).
        assert_eq!(
            merge_kind(Some(ChangeKind::Removed), ChangeKind::Created),
            ChangeKind::Created,
            "a Create after a pending Removed must override it (git restore)",
        );
        assert_eq!(
            merge_kind(Some(ChangeKind::Removed), ChangeKind::Modified),
            ChangeKind::Modified,
            "a Modify after a pending Removed must override it (git restore)",
        );
        assert_eq!(
            merge_kind(Some(ChangeKind::Removed), ChangeKind::Removed),
            ChangeKind::Removed,
        );
    }

    #[test]
    fn no_prior_takes_incoming_kind() {
        assert_eq!(merge_kind(None, ChangeKind::Created), ChangeKind::Created);
        assert_eq!(merge_kind(None, ChangeKind::Modified), ChangeKind::Modified);
        assert_eq!(merge_kind(None, ChangeKind::Removed), ChangeKind::Removed);
    }

    #[test]
    fn modified_then_created_upgrades_to_created() {
        // Unusual but possible: Modify event arrives before Create (OS ordering).
        // The incoming Created must win over the prior Modified.
        assert_eq!(
            merge_kind(Some(ChangeKind::Modified), ChangeKind::Created),
            ChangeKind::Created,
        );
    }

    // ---------------------------------------------------------------------------
    // Flush-time existence reconciliation (`resolve_emit_kind`) — issue #823
    // defense-in-depth. If a path's pending kind is Removed but the file is
    // present on disk at flush time, it was restored (e.g. git checkout that
    // emitted only a bare Remove on this platform): emit Modified, not Removed.
    // ---------------------------------------------------------------------------

    #[test]
    fn resolve_emit_kind_removed_but_present_becomes_modified() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("restored.txt");
        std::fs::write(&file, b"hello").expect("write");
        assert_eq!(
            resolve_emit_kind(&file, ChangeKind::Removed),
            ChangeKind::Modified,
            "Removed for an existing path means it was restored",
        );
    }

    #[test]
    fn resolve_emit_kind_removed_and_absent_stays_removed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("gone.txt");
        assert_eq!(
            resolve_emit_kind(&file, ChangeKind::Removed),
            ChangeKind::Removed,
            "Removed for a missing path is a genuine deletion",
        );
    }

    #[test]
    fn resolve_emit_kind_present_non_removed_is_passthrough() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("any.txt");
        std::fs::write(&file, b"x").expect("write");
        assert_eq!(
            resolve_emit_kind(&file, ChangeKind::Created),
            ChangeKind::Created
        );
        assert_eq!(
            resolve_emit_kind(&file, ChangeKind::Modified),
            ChangeKind::Modified
        );
    }

    #[test]
    fn resolve_emit_kind_absent_non_removed_becomes_removed() {
        // A real deletion can land as Remove→Modify (trailing name/metadata
        // event) for an already-gone path; merge_kind coalesces that to
        // Modified, so flush-time existence must downgrade it back to Removed
        // or the orchestrator skips its prune path.
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("gone.txt");
        assert_eq!(
            resolve_emit_kind(&file, ChangeKind::Modified),
            ChangeKind::Removed,
            "Modified for a missing path is a genuine deletion",
        );
        assert_eq!(
            resolve_emit_kind(&file, ChangeKind::Created),
            ChangeKind::Removed,
            "Created for a missing path is a genuine deletion",
        );
    }

    // ---------------------------------------------------------------------------
    // Transient stat-miss reconciliation (`reconcile_emit_kind`) — issue #1058.
    // Under heavy IO load on arm64 macOS, `try_exists()` can return an `Err`
    // (transient EIO/ENFILE) for a file that was just written. That unknown
    // state must NOT downgrade a live write event to a phantom deletion, or the
    // edit is routed to the prune regime and never eagerly re-rendered.
    // ---------------------------------------------------------------------------

    fn transient_err() -> std::io::Result<bool> {
        Err(std::io::Error::other("transient stat miss"))
    }

    #[test]
    fn reconcile_emit_kind_write_event_with_io_error_passes_through() {
        assert_eq!(
            reconcile_emit_kind(ChangeKind::Modified, transient_err()),
            ChangeKind::Modified,
            "a transient stat miss must not turn a Modified into a deletion (#1058)",
        );
        assert_eq!(
            reconcile_emit_kind(ChangeKind::Created, transient_err()),
            ChangeKind::Created,
            "a transient stat miss must not turn a Created into a deletion (#1058)",
        );
    }

    #[test]
    fn reconcile_emit_kind_removed_with_io_error_stays_removed() {
        // A delete is the change we can least afford to drop, so an unknown
        // state for an incoming Removed still surfaces as Removed.
        assert_eq!(
            reconcile_emit_kind(ChangeKind::Removed, transient_err()),
            ChangeKind::Removed,
        );
    }

    #[test]
    fn reconcile_emit_kind_definitive_answers_match_legacy_matrix() {
        // Every definitive (Ok) answer preserves the pre-#1058 behaviour.
        assert_eq!(
            reconcile_emit_kind(ChangeKind::Removed, Ok(true)),
            ChangeKind::Modified,
        );
        assert_eq!(
            reconcile_emit_kind(ChangeKind::Removed, Ok(false)),
            ChangeKind::Removed,
        );
        assert_eq!(
            reconcile_emit_kind(ChangeKind::Modified, Ok(true)),
            ChangeKind::Modified
        );
        assert_eq!(
            reconcile_emit_kind(ChangeKind::Created, Ok(true)),
            ChangeKind::Created
        );
        assert_eq!(
            reconcile_emit_kind(ChangeKind::Modified, Ok(false)),
            ChangeKind::Removed
        );
        assert_eq!(
            reconcile_emit_kind(ChangeKind::Created, Ok(false)),
            ChangeKind::Removed
        );
    }

    /// Regression test for the `Watcher::shutdown()` circular-wait deadlock
    /// (issue #708 / sub-issue #759). Before the fix, the shutdown-signal
    /// branch flushed then fell through to `let _ = bridge.await;`, but the
    /// bridge `spawn_blocking` task is parked in `raw_rx.recv()`, which only
    /// closes when `_notify` drops — and `_notify` drops only when `shutdown`
    /// RETURNS. So `shutdown().await` could never complete. The timeout here
    /// is the guard: it fails (rather than hangs the whole suite) if the
    /// deadlock ever returns.
    #[tokio::test]
    async fn shutdown_returns_within_timeout() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (watcher, _rx) =
            Watcher::start(tmp.path(), std::iter::once(".")).expect("watcher start");

        tokio::time::timeout(Duration::from_secs(3), watcher.shutdown())
            .await
            .expect("Watcher::shutdown() must complete (circular-wait deadlock regression)");
    }

    // ---------------------------------------------------------------------------
    // Real-watcher integration tests for issue #823 (git-restore not picked up).
    //
    // These drive a live `notify` watcher in a tempdir and assert that a
    // git-restore-shaped mutation — `remove_file(target)` then re-creating the
    // same path — surfaces as a NON-`Removed` change. We assert "not Removed"
    // (rather than an exact kind) because FSEvents/inotify may coalesce the
    // remove+recreate into a single Create, deliver Remove→Create, or (under
    // load) emit only a bare Remove that the `resolve_emit_kind` existence
    // check then upgrades to Modified. All three paths must reach the rebuild
    // loop, so the only wrong outcome is a surviving `Removed`.
    //
    // Timing is deliberately generous (200ms debounce, multi-second drain
    // deadlines) so the tests stay reliable on a busy CI runner.
    // ---------------------------------------------------------------------------

    /// Collect changes touching `target` until `deadline`, returning the last
    /// kind seen for that path (or `None` if no event arrived in time).
    async fn last_kind_for(
        rx: &mut mpsc::Receiver<Change>,
        target: &Path,
        deadline: Duration,
    ) -> Option<ChangeKind> {
        let mut seen = None;
        let _ = tokio::time::timeout(deadline, async {
            while let Some(change) = rx.recv().await {
                if change.path == target {
                    seen = Some(change.kind);
                }
            }
        })
        .await;
        seen
    }

    async fn next_kind_for(
        rx: &mut mpsc::Receiver<Change>,
        target: &Path,
        deadline: Duration,
    ) -> Option<ChangeKind> {
        tokio::time::timeout(deadline, async {
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dynamic_dependency_parent_observes_edit_delete_and_recreate_outside_boot_roots() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().canonicalize().expect("canonicalize tempdir");
        std::fs::create_dir_all(root.join("pages")).expect("pages dir");
        std::fs::create_dir_all(root.join("lib")).expect("lib dir");
        let dependency = root.join("lib/worker-helper.ts");
        std::fs::write(&dependency, "export const marker = 'one';\n").expect("seed helper");

        // `lib/` is deliberately absent from the boot roots. The worker graph
        // is discovered during the boot islands pass and registers this file
        // dynamically by watching its parent directory.
        let (mut watcher, mut rx) = Watcher::start_with_debounce(
            &root,
            std::iter::once("pages"),
            Duration::from_millis(50),
        )
        .expect("watcher start");
        watcher.watch_additional_files([&dependency]);
        tokio::time::sleep(Duration::from_millis(100)).await;
        while rx.try_recv().is_ok() {}

        std::fs::write(&dependency, "export const marker = 'two';\n").expect("edit helper");
        assert!(
            matches!(
                next_kind_for(&mut rx, &dependency, Duration::from_secs(3)).await,
                Some(ChangeKind::Created) | Some(ChangeKind::Modified)
            ),
            "outside-root worker helper edit must reach the watcher"
        );

        std::fs::remove_file(&dependency).expect("delete helper");
        assert_eq!(
            next_kind_for(&mut rx, &dependency, Duration::from_secs(3)).await,
            Some(ChangeKind::Removed),
            "watching the parent must preserve delete visibility"
        );

        std::fs::write(&dependency, "export const marker = 'three';\n").expect("recreate helper");
        assert!(
            matches!(
                next_kind_for(&mut rx, &dependency, Duration::from_secs(3)).await,
                Some(ChangeKind::Created) | Some(ChangeKind::Modified)
            ),
            "watching the parent must preserve recreate visibility"
        );

        watcher.shutdown().await;
    }

    /// `watch_additional_files` returns exactly the parent directories NEWLY
    /// watched by the call — the observable the dev orchestrator surfaces as the
    /// #1678 `watch-extra registered:` signal. A dependency whose parent is
    /// already covered by a recursive boot root (the in-project / non-workspace
    /// case) yields NOTHING, so a project without workspace siblings registers
    /// no new directory and emits no signal (issue #1678 acceptance: non-
    /// workspace projects' watch set is unchanged).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watch_additional_files_returns_only_newly_watched_out_of_root_parents() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().canonicalize().expect("canonicalize tempdir");
        std::fs::create_dir_all(root.join("src")).expect("src dir");
        std::fs::create_dir_all(root.join("sibling")).expect("sibling dir");
        let in_root_dep = root.join("src/in-root.ts");
        let sibling_dep = root.join("sibling/sib.frag");
        std::fs::write(&in_root_dep, "in-root\n").expect("seed in-root dep");
        std::fs::write(&sibling_dep, "sib\n").expect("seed sibling dep");

        // `src` is a recursive boot root; `sibling` is not.
        let (mut watcher, _rx) =
            Watcher::start_with_debounce(&root, std::iter::once("src"), Duration::from_millis(50))
                .expect("watcher start");

        // The in-root dependency's parent is already recursively covered, so it
        // registers nothing; only the out-of-root sibling parent is newly
        // watched and returned.
        let newly = watcher.watch_additional_files([in_root_dep.clone(), sibling_dep.clone()]);
        assert_eq!(
            newly,
            vec![root.join("sibling")],
            "only the out-of-root sibling parent is newly watched"
        );

        // Re-registering an already-watched parent is idempotent: nothing new.
        let again = watcher.watch_additional_files([sibling_dep]);
        assert!(
            again.is_empty(),
            "re-registering an already-watched sibling parent must be a no-op, got {again:?}"
        );

        watcher.shutdown().await;
    }

    /// `git checkout -- <file>` unlinks then recreates the tracked file. The
    /// watcher must report a non-`Removed` change so the dev server re-renders
    /// instead of serving the pre-restore page forever (issue #823).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn git_restore_remove_then_write_is_reported() {
        // FSEvents reports canonical paths (/var/folders → /private/...), so
        // canonicalize the root before watching to match received paths.
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().canonicalize().expect("canonicalize tempdir");
        let target = root.join("hello.txt");
        std::fs::write(&target, b"original\n").expect("seed file");

        let debounce = Duration::from_millis(200);
        let (watcher, mut rx) = Watcher::start_with_debounce(&root, std::iter::once("."), debounce)
            .expect("watcher start");

        // Let the OS watch settle and drain the seed/create noise.
        let _ = last_kind_for(&mut rx, &target, Duration::from_millis(500)).await;

        // Simulate git's write_entry: unlink, then recreate with new bytes.
        std::fs::remove_file(&target).expect("remove");
        std::fs::write(&target, b"restored\n").expect("recreate");

        let kind = last_kind_for(&mut rx, &target, Duration::from_secs(3)).await;
        assert!(
            matches!(kind, Some(ChangeKind::Created) | Some(ChangeKind::Modified)),
            "git restore (remove+write) must surface a non-Removed change, got {kind:?}",
        );

        watcher.shutdown().await;
    }

    /// The other git-restore shape: unlink the target, then rename a temp file
    /// over it (atomic replace, new inode). Must also report non-`Removed`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn git_restore_remove_then_rename_over_is_reported() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().canonicalize().expect("canonicalize tempdir");
        let target = root.join("hello.txt");
        std::fs::write(&target, b"original\n").expect("seed file");

        let debounce = Duration::from_millis(200);
        let (watcher, mut rx) = Watcher::start_with_debounce(&root, std::iter::once("."), debounce)
            .expect("watcher start");

        let _ = last_kind_for(&mut rx, &target, Duration::from_millis(500)).await;

        std::fs::remove_file(&target).expect("remove");
        let staged = root.join("hello.txt.tmp");
        std::fs::write(&staged, b"restored\n").expect("stage");
        std::fs::rename(&staged, &target).expect("rename over");

        let kind = last_kind_for(&mut rx, &target, Duration::from_secs(3)).await;
        assert!(
            matches!(kind, Some(ChangeKind::Created) | Some(ChangeKind::Modified)),
            "git restore (remove+rename-over) must surface a non-Removed change, got {kind:?}",
        );

        watcher.shutdown().await;
    }

    // ---------------------------------------------------------------------------
    // Starvation / livelock regression (sub #1138 ← source #1113).
    //
    // Under a *continuous* high-rate event stream the `biased` `select!` in
    // `debouncer_task` services the `recv` arm forever and STARVES the `tick`
    // arm, so the quiet-window drain never runs while the burst is in flight.
    // Worse, a single continuously rewritten "hot" file has its `last_seen`
    // bumped on every event, so it is never "quiet" — a quiet-only filter would
    // livelock and never release it until the stream stops.
    //
    // We drive `debouncer_task` DIRECTLY with a synthetic saturating stream
    // rather than through a live `notify` watcher: the kernel's inotify layer
    // coalesces rapid writes to one path down to a handful of events, so a
    // real-FS writer cannot reliably reproduce the saturating stream the bug
    // needs. Feeding the sync `raw_rx` channel ourselves gives a deterministic,
    // platform-independent reproduction of the exact `select!` pathology.
    //
    // Mid-burst proof, BY CONSTRUCTION (issue #1357 ← source #1355): the two
    // tests below used to prove "flushed while still streaming" with a
    // timing ratio — a fixed 2s producer vs. a 1s consumer deadline. Under
    // CPU pressure that 1s deadline is a bare-timing-wait flake (deflaking
    // root cause #1 in CLAUDE.md's Testing section) even though the
    // debouncer behaved correctly. Nothing in the product contract (the
    // starvation guard at lines 507-525 above) promises a 1s max latency —
    // that number was test convenience, not spec.
    //
    // Instead, each producer streams with no sleep (load-bearing: keeping an
    // event always buffered is what makes the `biased` recv arm win every
    // `select!` and starve `tick`, per the comment above) until the CONSUMER
    // sets a shared `done` flag once it has observed its per-test condition
    // (see each test below), bounded by `PRODUCER_FAILSAFE_CAP` as a
    // give-up-only backstop. Because the producer only stops when told to
    // (or the cap is hit), any condition the consumer observes was
    // necessarily satisfied WHILE the producer was still streaming — no
    // timing ratio needed. Every remaining timeout below (the consumer's
    // `tokio::time::timeout` and the producer's cap) is a give-up failsafe
    // that bounds worst-case test runtime on a regression, never a proof
    // deadline — same framing as the `drive()` helper's 5s timeout below and
    // `zfb_test_utils::watcher_live_handshake`'s deadline
    // (crates/zfb-test-utils/src/watcher_handshake.rs:135-198).
    // ---------------------------------------------------------------------------

    /// Give-up failsafe for the consumer's `tokio::time::timeout` in the two
    /// sustained-burst tests below — bounds worst-case test runtime if a
    /// regression starves the debouncer forever. NOT a proof deadline: the
    /// mid-burst proof comes from the `done`-flag construction described
    /// above, not from this number. Kept strictly less than
    /// `PRODUCER_FAILSAFE_CAP` so a regressed debouncer times out the
    /// consumer (a clean test failure) instead of the producer thread
    /// outliving it.
    const CONSUMER_FAILSAFE: Duration = Duration::from_secs(30);

    /// Give-up cap on the producer thread's streaming loop in the two
    /// sustained-burst tests below. Normally the loop exits promptly once
    /// the consumer sets `done`; this cap only matters if that never
    /// happens (i.e. the same regression `CONSUMER_FAILSAFE` guards
    /// against), so it just needs to stay strictly greater than
    /// `CONSUMER_FAILSAFE`.
    const PRODUCER_FAILSAFE_CAP: Duration = Duration::from_secs(60);

    /// Bound on the producers' raw channel in the two sustained-burst tests
    /// below (`std_mpsc::sync_channel`, which hands `debouncer_task` the same
    /// `Receiver` type as the unbounded `channel` production uses). Without a
    /// bound, a regressed/stalled consumer would let the no-sleep producer
    /// enqueue up to `PRODUCER_FAILSAFE_CAP` worth of events — memory
    /// exhaustion instead of a clean timeout. Backpressure does NOT weaken
    /// the saturation property: a full channel means events are always
    /// buffered ahead of the bridge, which is exactly what keeps the
    /// `biased` recv arm winning. If the whole pipeline stalls, the
    /// producer blocks in `send` (without re-checking its failsafe cap)
    /// rather than allocating; the consumer failsafe then fails the test,
    /// and the unwind closes the channel, which errors the blocked `send`
    /// and lets the producer thread exit.
    const PRODUCER_CHANNEL_BOUND: usize = 4096;

    /// Build a synthetic `notify` modify event for a single path (mirrors what
    /// the recommended watcher hands us for a content edit).
    fn modify_event(path: &Path) -> notify::Result<Event> {
        use notify::event::{DataChange, EventKind, ModifyKind};
        Ok(Event {
            kind: EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            paths: vec![path.to_path_buf()],
            attrs: Default::default(),
        })
    }

    /// A continuous single-hot-file stream must flush MID-STREAM. Against the
    /// pre-fix loop the `recv` arm starves `tick` and the hot file never goes
    /// quiet, so nothing is emitted until the producer stops. The condition
    /// here — the first flush for `hot`, observed while the producer is
    /// still streaming (by construction, see the section header above) —
    /// makes that failure observable without any timing ratio.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sustained_hot_file_stream_flushes_mid_burst() {
        let hot = PathBuf::from("/synthetic/hot.txt");

        // Bounded: see `PRODUCER_CHANNEL_BOUND` — caps producer backlog
        // without weakening recv-arm saturation.
        let (raw_tx, raw_rx) =
            std_mpsc::sync_channel::<notify::Result<Event>>(PRODUCER_CHANNEL_BOUND);
        let (out_tx, mut out_rx) = mpsc::channel::<Change>(256);
        let (_shutdown_tx, shutdown_rx) = oneshot::channel();

        let debounce = Duration::from_millis(100);
        let task = tokio::spawn(debouncer_task(raw_rx, out_tx, debounce, shutdown_rx));

        // Producer: hammer the SAME path as fast as the bounded bridge will
        // accept, until the consumer below sets `done` (its mid-burst proof
        // landed) or `PRODUCER_FAILSAFE_CAP` gives up. No sleep — keeping an
        // event always buffered is what makes the `biased` recv arm win
        // every `select!` and starve `tick`; that starvation IS the product
        // path under test (see the section header above), so do not add a
        // sleep here. `raw_tx` is moved in and dropped when the loop ends,
        // which closes the channel and lets the task exit cleanly.
        let done = Arc::new(AtomicBool::new(false));
        let producer_done = Arc::clone(&done);
        let producer_path = hot.clone();
        let producer = std::thread::spawn(move || {
            let started = Instant::now();
            while !producer_done.load(Ordering::Relaxed)
                && started.elapsed() < PRODUCER_FAILSAFE_CAP
            {
                if raw_tx.send(modify_event(&producer_path)).is_err() {
                    break; // debouncer task gone
                }
            }
        });

        // Must observe a flush for the hot file WHILE the producer is still
        // running — it only stops once we set `done` below (or the failsafe
        // cap is hit), so a pass here proves the forced flush fired
        // mid-stream BY CONSTRUCTION, not via a timing ratio.
        // `CONSUMER_FAILSAFE` is a give-up failsafe, not a proof deadline.
        let flushed = tokio::time::timeout(CONSUMER_FAILSAFE, async {
            while let Some(change) = out_rx.recv().await {
                if change.path == hot {
                    done.store(true, Ordering::Relaxed);
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false);

        assert!(
            flushed,
            "a continuously streamed hot file must flush mid-burst; the tick \
             arm was starved and/or the hot file livelocked (sub #1138)",
        );

        // Drop the receiver so the debouncer's next `out_tx.send` errors and the
        // task returns (which drops `raw_rx`, ending the producer's sends). Doing
        // this BEFORE awaiting avoids a deadlock where the task blocks on a full
        // outbound channel that nobody is draining.
        drop(out_rx);
        let _ = producer.join();
        let _ = task.await;
    }

    /// A continuous stream across MANY distinct paths must also drain mid-burst
    /// — exercising the `pending.len() >= MAX_PENDING_BEFORE_FORCED_FLUSH`
    /// branch (a `git checkout` of a huge tree), not just the elapsed-window
    /// branch. We keep the STRONGER condition here (≥50 distinct paths
    /// drained while the producer is still running, by construction) so this
    /// still clearly proves the forced-flush-on-size branch, not just any
    /// single flush.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sustained_many_paths_stream_drains_mid_burst() {
        // Bounded: see `PRODUCER_CHANNEL_BOUND` — caps producer backlog
        // without weakening recv-arm saturation.
        let (raw_tx, raw_rx) =
            std_mpsc::sync_channel::<notify::Result<Event>>(PRODUCER_CHANNEL_BOUND);
        let (out_tx, mut out_rx) = mpsc::channel::<Change>(256);
        let (_shutdown_tx, shutdown_rx) = oneshot::channel();

        let debounce = Duration::from_millis(100);
        let task = tokio::spawn(debouncer_task(raw_rx, out_tx, debounce, shutdown_rx));

        // Producer: a flood of DISTINCT paths (like a bulk checkout) until the
        // consumer below sets `done` (its 50-path mid-burst proof landed) or
        // `PRODUCER_FAILSAFE_CAP` gives up. See the hot-file test above for
        // why the loop has no sleep.
        let done = Arc::new(AtomicBool::new(false));
        let producer_done = Arc::clone(&done);
        let producer = std::thread::spawn(move || {
            let started = Instant::now();
            let mut n: u64 = 0;
            while !producer_done.load(Ordering::Relaxed)
                && started.elapsed() < PRODUCER_FAILSAFE_CAP
            {
                let p = PathBuf::from(format!("/synthetic/tree/file_{n}.txt"));
                if raw_tx.send(modify_event(&p)).is_err() {
                    break;
                }
                n += 1;
            }
        });

        // Expect >=50 distinct emitted changes WHILE the producer is still
        // running — it only stops once we set `done` below (or the failsafe
        // cap is hit), so reaching 50 here proves `pending` is draining
        // under a continuous stream BY CONSTRUCTION, not via a timing ratio.
        // `CONSUMER_FAILSAFE` is a give-up failsafe, not a proof deadline.
        let drained = tokio::time::timeout(CONSUMER_FAILSAFE, async {
            let mut count = 0usize;
            while let Some(_change) = out_rx.recv().await {
                count += 1;
                if count >= 50 {
                    done.store(true, Ordering::Relaxed);
                    return count;
                }
            }
            count
        })
        .await
        .unwrap_or(0);

        assert!(
            drained >= 50,
            "a continuous many-path stream must drain mid-burst (got {drained} \
             changes); pending was starving `tick` (sub #1138)",
        );

        // See the note in the hot-file test: drop the receiver before awaiting
        // so the task can unwind instead of blocking on a full outbound channel.
        drop(out_rx);
        let _ = producer.join();
        let _ = task.await;
    }

    // =========================================================================
    // Part A (issue #1348) — deterministic coalescing / dedup / dispatch tests.
    //
    // These drive `debouncer_task` DIRECTLY through the same synthetic `raw_rx`
    // seam the two starvation tests above already use. No real `notify` watcher
    // and no dependence on OS event timing: we feed a fixed list of synthetic
    // events, then CLOSE the raw channel to force the deterministic `flush_all`
    // path (bridge closes → `bridge_rx.recv()` returns `None` → every pending
    // entry drains regardless of the quiet window).
    //
    // Determinism guarantee: the debounce is set to an hour, so NEITHER the
    // `tick` arm NOR the recv-arm forced flush (`now - last_drain >= debounce`
    // or `pending.len() >= 1024`) can fire within a test. The interval's
    // immediate first `tick` drains nothing (no entry is an hour old). So the
    // ONLY non-empty emission path is the channel-close `flush_all`, making the
    // emitted set a pure function of the fed events — exactly the Level-1 seam
    // #1348 asks for: coalescing/dedup proven in sub-second unit tests instead
    // of a 200s e2e harness. No refactor of production code was needed; this
    // seam already existed.
    // =========================================================================

    fn create_event(path: &Path) -> notify::Result<Event> {
        use notify::event::{CreateKind, EventKind};
        Ok(Event {
            kind: EventKind::Create(CreateKind::File),
            paths: vec![path.to_path_buf()],
            attrs: Default::default(),
        })
    }

    fn remove_event(path: &Path) -> notify::Result<Event> {
        use notify::event::{EventKind, RemoveKind};
        Ok(Event {
            kind: EventKind::Remove(RemoveKind::File),
            paths: vec![path.to_path_buf()],
            attrs: Default::default(),
        })
    }

    fn access_open_event(path: &Path) -> notify::Result<Event> {
        use notify::event::{AccessKind, AccessMode, EventKind};
        Ok(Event {
            kind: EventKind::Access(AccessKind::Open(AccessMode::Any)),
            paths: vec![path.to_path_buf()],
            attrs: Default::default(),
        })
    }

    fn access_close_write_event(path: &Path) -> notify::Result<Event> {
        use notify::event::{AccessKind, AccessMode, EventKind};
        Ok(Event {
            kind: EventKind::Access(AccessKind::Close(AccessMode::Write)),
            paths: vec![path.to_path_buf()],
            attrs: Default::default(),
        })
    }

    /// A single synthetic modify event carrying an arbitrary set of paths
    /// (notify delivers multi-path events for renames; an empty vec models the
    /// pathless events some backends emit).
    fn modify_event_with_paths(paths: Vec<PathBuf>) -> notify::Result<Event> {
        use notify::event::{EventKind, ModifyKind};
        Ok(Event {
            kind: EventKind::Modify(ModifyKind::Any),
            paths,
            attrs: Default::default(),
        })
    }

    /// Drive `debouncer_task` with a fixed list of synthetic raw events, then
    /// close the raw channel to force the deterministic `flush_all`, and
    /// collect every emitted `Change`. See the section header for why this is
    /// timing-independent. The 5s timeout is a failsafe against a regression
    /// that fails to flush/close — the normal path completes in microseconds
    /// once the channel is closed; it is NOT a fixed warmup wait.
    async fn drive(events: Vec<notify::Result<Event>>) -> Vec<Change> {
        let (raw_tx, raw_rx) = std_mpsc::channel::<notify::Result<Event>>();
        let (out_tx, mut out_rx) = mpsc::channel::<Change>(256);
        let (_shutdown_tx, shutdown_rx) = oneshot::channel();

        // One hour: no time-based flush can fire mid-test (see header).
        let debounce = Duration::from_secs(3600);
        let task = tokio::spawn(debouncer_task(raw_rx, out_tx, debounce, shutdown_rx));

        for evt in events {
            raw_tx.send(evt).expect("feed synthetic event");
        }
        // Closing the raw channel is the flush trigger.
        drop(raw_tx);

        let mut collected = Vec::new();
        let drained = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(change) = out_rx.recv().await {
                collected.push(change);
            }
        })
        .await;
        assert!(
            drained.is_ok(),
            "debouncer_task must flush and close its output channel promptly \
             after the raw channel closes",
        );
        let _ = task.await;
        collected
    }

    /// Return the single collected change for `path`, asserting exactly one exists.
    fn one_for<'a>(changes: &'a [Change], path: &Path) -> &'a Change {
        let matches: Vec<&Change> = changes
            .iter()
            .filter(|c| c.path.as_path() == path)
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "expected exactly one emitted change for {path:?}, got {changes:#?}",
        );
        matches[0]
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn coalesce_five_events_same_path_emits_once() {
        // The core debounce coalescing contract: N rapid events on ONE path
        // collapse to a single emission. A real file backs the path so the
        // flush-time existence reconciliation passes the kind through unchanged.
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("hot.txt");
        std::fs::write(&p, b"x").expect("seed");
        let changes = drive(vec![
            modify_event(&p),
            modify_event(&p),
            modify_event(&p),
            modify_event(&p),
            modify_event(&p),
        ])
        .await;
        assert_eq!(changes.len(), 1, "5 events on one path must coalesce to 1");
        assert_eq!(one_for(&changes, &p).kind, ChangeKind::Modified);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn three_distinct_paths_each_emit_once() {
        // Distinct paths in one window batch out separately (one emit each).
        let tmp = tempfile::tempdir().expect("tempdir");
        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.txt");
        let c = tmp.path().join("c.txt");
        for p in [&a, &b, &c] {
            std::fs::write(p, b"x").expect("seed");
        }
        let changes = drive(vec![modify_event(&a), modify_event(&b), modify_event(&c)]).await;
        assert_eq!(changes.len(), 3, "three distinct paths must each emit once");
        for p in [&a, &b, &c] {
            assert_eq!(one_for(&changes, p).kind, ChangeKind::Modified);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn interleaved_mixed_burst_coalesces_per_path() {
        // a×3, b×2, c×1 interleaved → exactly 3 emissions (one per path). Proves
        // per-path coalescing holds across an interleaved multi-path burst.
        let tmp = tempfile::tempdir().expect("tempdir");
        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.txt");
        let c = tmp.path().join("c.txt");
        for p in [&a, &b, &c] {
            std::fs::write(p, b"x").expect("seed");
        }
        let changes = drive(vec![
            modify_event(&a),
            modify_event(&b),
            modify_event(&a),
            modify_event(&c),
            modify_event(&b),
            modify_event(&a),
        ])
        .await;
        assert_eq!(changes.len(), 3, "interleaved burst must coalesce per path");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn created_then_modified_same_path_emits_created() {
        // merge_kind's sticky-Created (#660) threaded through the task: a Create
        // followed by a Modify in one burst surfaces as Created so the watch-ADD
        // discovery hook fires. Proves the wiring, not just the pure helper.
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("new.txt");
        std::fs::write(&p, b"x").expect("seed");
        let changes = drive(vec![create_event(&p), modify_event(&p)]).await;
        assert_eq!(changes.len(), 1);
        assert_eq!(one_for(&changes, &p).kind, ChangeKind::Created);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remove_then_create_same_path_emits_non_removed() {
        // git-restore shape (#823): Remove then Create on the same path must NOT
        // stay Removed. File exists at flush so reconciliation passes the merged
        // (non-Removed) kind through. Deterministic reproduction of a bug that
        // otherwise only surfaces in a real-FS e2e.
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("restored.txt");
        std::fs::write(&p, b"x").expect("seed");
        let changes = drive(vec![remove_event(&p), create_event(&p)]).await;
        assert_eq!(changes.len(), 1);
        assert!(
            matches!(
                one_for(&changes, &p).kind,
                ChangeKind::Created | ChangeKind::Modified
            ),
            "remove+create must surface a non-Removed change",
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn modify_event_for_absent_path_downgrades_to_removed() {
        // resolve_emit_kind (#823 defense-in-depth) threaded through flush: a
        // Modify whose path is absent on disk is really a deletion → Removed.
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("ghost.txt"); // never created
        let changes = drive(vec![modify_event(&p)]).await;
        assert_eq!(changes.len(), 1);
        assert_eq!(one_for(&changes, &p).kind, ChangeKind::Removed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remove_event_for_present_path_upgrades_to_modified() {
        // A bare Remove for a path that exists at flush = a restore → Modified.
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("present.txt");
        std::fs::write(&p, b"x").expect("seed");
        let changes = drive(vec![remove_event(&p)]).await;
        assert_eq!(changes.len(), 1);
        assert_eq!(one_for(&changes, &p).kind, ChangeKind::Modified);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pure_read_open_access_event_emits_nothing() {
        // classify() drops IN_OPEN so the inotify Access→tick churn loop can't
        // form; proven at the task level (zero emissions from a read-only event).
        let p = PathBuf::from("/synthetic/opened.txt");
        let changes = drive(vec![access_open_event(&p)]).await;
        assert!(
            changes.is_empty(),
            "pure-read Open access must emit nothing, got {changes:#?}",
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn access_close_write_event_emits_modified() {
        // IN_CLOSE_WRITE is a completed write → Modified (not dropped like the
        // pure-read Access variants).
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("written.txt");
        std::fs::write(&p, b"x").expect("seed");
        let changes = drive(vec![access_close_write_event(&p)]).await;
        assert_eq!(changes.len(), 1);
        assert_eq!(one_for(&changes, &p).kind, ChangeKind::Modified);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remove_event_for_absent_path_emits_removed() {
        // Control case for the two reconciliation tests above: a genuine
        // deletion (Remove + absent) stays Removed.
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("deleted.txt"); // never created
        let changes = drive(vec![remove_event(&p)]).await;
        assert_eq!(changes.len(), 1);
        assert_eq!(one_for(&changes, &p).kind, ChangeKind::Removed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn single_event_multiple_paths_fans_out_per_path() {
        // notify can carry >1 path in one Event (a rename delivers from+to). The
        // `for path in evt.paths` loop must coalesce each path independently.
        let tmp = tempfile::tempdir().expect("tempdir");
        let a = tmp.path().join("from.txt");
        let b = tmp.path().join("to.txt");
        for p in [&a, &b] {
            std::fs::write(p, b"x").expect("seed");
        }
        let changes = drive(vec![modify_event_with_paths(vec![a.clone(), b.clone()])]).await;
        assert_eq!(changes.len(), 2, "a 2-path event must fan out to 2 changes");
        one_for(&changes, &a);
        one_for(&changes, &b);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn notify_error_event_is_skipped_valid_event_still_emits() {
        // The `Err(_) => warn` arm must not abort the loop: a valid event after
        // an error event is still delivered.
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("ok.txt");
        std::fs::write(&p, b"x").expect("seed");
        let changes = drive(vec![
            Err(notify::Error::generic("synthetic watcher error")),
            modify_event(&p),
        ])
        .await;
        assert_eq!(changes.len(), 1);
        assert_eq!(one_for(&changes, &p).kind, ChangeKind::Modified);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn event_with_no_paths_emits_nothing() {
        // A pathless event (some backends emit these) is a safe no-op: the
        // `for path in evt.paths` loop runs zero times.
        let changes = drive(vec![modify_event_with_paths(vec![])]).await;
        assert!(
            changes.is_empty(),
            "pathless event must emit nothing, got {changes:#?}",
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn created_then_removed_same_path_emits_removed_when_absent() {
        // Create then Remove in one burst, path absent at flush → deletion.
        // Distinct from the remove-then-create restore shape above.
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("ephemeral.txt"); // never created on disk
        let changes = drive(vec![create_event(&p), remove_event(&p)]).await;
        assert_eq!(changes.len(), 1);
        assert_eq!(one_for(&changes, &p).kind, ChangeKind::Removed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn created_then_two_modifies_stays_created() {
        // The module docs note a Create is followed by "one or more Modify"
        // events; sticky-Created must survive MULTIPLE modifies, not just one.
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("multi.txt");
        std::fs::write(&p, b"x").expect("seed");
        let changes = drive(vec![create_event(&p), modify_event(&p), modify_event(&p)]).await;
        assert_eq!(changes.len(), 1);
        assert_eq!(one_for(&changes, &p).kind, ChangeKind::Created);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn distinct_paths_distinct_kinds_are_preserved() {
        // Per-path pending state is independent: Create(a)/Modify(b)/Remove(c)
        // emit Created/Modified/Removed respectively — no cross-contamination.
        let tmp = tempfile::tempdir().expect("tempdir");
        let a = tmp.path().join("created.txt");
        let b = tmp.path().join("modified.txt");
        let c = tmp.path().join("removed.txt"); // absent → genuine deletion
        std::fs::write(&a, b"x").expect("seed");
        std::fs::write(&b, b"x").expect("seed");
        let changes = drive(vec![create_event(&a), modify_event(&b), remove_event(&c)]).await;
        assert_eq!(changes.len(), 3);
        assert_eq!(one_for(&changes, &a).kind, ChangeKind::Created);
        assert_eq!(one_for(&changes, &b).kind, ChangeKind::Modified);
        assert_eq!(one_for(&changes, &c).kind, ChangeKind::Removed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn interleaved_two_files_preserve_independent_created_state() {
        // Interleaving Create/Modify bursts for two brand-new files must keep
        // BOTH Created — proving per-path merge state does not leak across paths
        // even when the two bursts are woven together.
        let tmp = tempfile::tempdir().expect("tempdir");
        let a = tmp.path().join("a_new.txt");
        let b = tmp.path().join("b_new.txt");
        std::fs::write(&a, b"x").expect("seed");
        std::fs::write(&b, b"x").expect("seed");
        let changes = drive(vec![
            create_event(&a),
            create_event(&b),
            modify_event(&a),
            modify_event(&b),
            modify_event(&a),
        ])
        .await;
        assert_eq!(changes.len(), 2);
        assert_eq!(one_for(&changes, &a).kind, ChangeKind::Created);
        assert_eq!(one_for(&changes, &b).kind, ChangeKind::Created);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn error_only_stream_emits_nothing_and_closes() {
        // An error-only event stream must not emit phantom changes and must
        // still close its output channel (the `drive` drain returns, not times out).
        let changes = drive(vec![
            Err(notify::Error::generic("boom one")),
            Err(notify::Error::generic("boom two")),
        ])
        .await;
        assert!(
            changes.is_empty(),
            "error-only stream must emit nothing, got {changes:#?}",
        );
    }
}
