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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as NotifyWatcher};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

/// Default debounce window. Chosen short enough to feel instant in dev
/// (~one frame at 60fps + a bit) but long enough to coalesce typical
/// editor save bursts on Linux/macOS/Windows.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(50);

/// Normalised filesystem change kind.
///
/// Collapses notify's many `EventKind` variants into the three things
/// downstream rebuild logic actually cares about. Anything we cannot
/// classify is treated as `Modified` — we'd rather rebuild
/// unnecessarily than miss a real change.
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

        for rel in relative_paths {
            let full = root.join(rel.as_ref());
            if !full.exists() {
                warn!(path = %full.display(), "watch target missing; skipping");
                continue;
            }
            if let Err(e) = notify_watcher.watch(&full, RecursiveMode::Recursive) {
                warn!(path = %full.display(), error = %e, "failed to watch path");
            } else {
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
            },
            out_rx,
        ))
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
fn classify(kind: &EventKind) -> ChangeKind {
    match kind {
        EventKind::Create(_) => ChangeKind::Created,
        EventKind::Remove(_) => ChangeKind::Removed,
        EventKind::Modify(_) | EventKind::Access(_) | EventKind::Any | EventKind::Other => {
            ChangeKind::Modified
        }
    }
}

/// Per-path pending state used by the debouncer.
struct Pending {
    kind: ChangeKind,
    last_seen: Instant,
}

/// The debouncer loop. Runs on a tokio task.
///
/// Strategy: bridge the sync `notify` channel onto a tokio channel via
/// `spawn_blocking`, then loop with a small "wake every debounce/2"
/// timer that flushes any path whose last event is older than the
/// debounce window. This avoids per-path timer tasks (cheap) and gives
/// O(1) flush per tick.
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
                        let kind = classify(&evt.kind);
                        let now = Instant::now();
                        for path in evt.paths {
                            // Merge the incoming kind with any already-pending
                            // kind for this path, preserving the highest-priority
                            // kind across the burst window.
                            //
                            // Priority (highest → lowest): Removed > Created > Modified.
                            //
                            // Rationale:
                            //   - `Removed` always wins: the file is gone — any
                            //     preceding Create/Modify is moot.
                            //   - `Created` wins over `Modified`: on macOS FSEvents
                            //     (and some Linux inotify setups), `fs::write` to a
                            //     brand-new path fires both a Create event and then
                            //     one or more Modify events within the debounce
                            //     window. The old "keep latest" rule collapsed the
                            //     burst to `Modified`, silently dropping the `Created`
                            //     signal that the watch-ADD discovery hook relies on
                            //     (`tick_with_kinds` only calls the hook for
                            //     `ChangeKind::Created`). Preserving `Created` when
                            //     it was ever seen during the burst closes that hole.
                            //   - `Modified` is a no-op upgrade when `Created` is
                            //     already pending: the consumer re-reads the file
                            //     either way, so demoting `Created` to `Modified`
                            //     would only hurt (breaks discovery), never help.
                            let merged_kind = match pending.get(&path).map(|p| p.kind) {
                                Some(ChangeKind::Removed) => ChangeKind::Removed, // Removed is sticky
                                Some(ChangeKind::Created) => {
                                    // Created already seen; only Removed can upgrade it.
                                    if kind == ChangeKind::Removed {
                                        ChangeKind::Removed
                                    } else {
                                        ChangeKind::Created
                                    }
                                }
                                _ => kind, // Modified or nothing: take the incoming kind
                            };
                            pending.insert(path, Pending { kind: merged_kind, last_seen: now });
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
                for (path, kind) in ready {
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
    for (path, p) in pending.drain() {
        if out_tx.send(Change { path, kind: p.kind }).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_create_is_created() {
        use notify::event::{CreateKind, EventKind};
        assert_eq!(classify(&EventKind::Create(CreateKind::File)), ChangeKind::Created);
    }

    #[test]
    fn classify_remove_is_removed() {
        use notify::event::{EventKind, RemoveKind};
        assert_eq!(classify(&EventKind::Remove(RemoveKind::File)), ChangeKind::Removed);
    }

    #[test]
    fn classify_unknown_is_modified() {
        use notify::event::EventKind;
        assert_eq!(classify(&EventKind::Any), ChangeKind::Modified);
        assert_eq!(classify(&EventKind::Other), ChangeKind::Modified);
    }

    // ---------------------------------------------------------------------------
    // Burst-coalescing merge tests (the "sticky Created" fix — issue #660).
    //
    // On macOS FSEvents (and some Linux inotify setups), `fs::write` to a
    // brand-new path fires both a `Create` event and then one or more
    // `Modify` events within the debounce window. The "keep latest kind" rule
    // that existed before this fix collapsed such a burst to `Modified`,
    // silently dropping the `Created` signal that the watch-ADD discovery hook
    // relies on. The merged-kind logic below guarantees `Created` is preserved
    // when it was ever seen for a path during the burst window.
    // ---------------------------------------------------------------------------

    /// Simulate the merge logic inline so we can property-test it without
    /// reaching into the private debouncer internals.
    fn merge(existing: Option<ChangeKind>, incoming: ChangeKind) -> ChangeKind {
        match existing {
            Some(ChangeKind::Removed) => ChangeKind::Removed,
            Some(ChangeKind::Created) => {
                if incoming == ChangeKind::Removed {
                    ChangeKind::Removed
                } else {
                    ChangeKind::Created
                }
            }
            _ => incoming,
        }
    }

    #[test]
    fn created_then_modified_stays_created() {
        // The primary fix: a new file write on macOS fires Create→Modify.
        // Downstream discovery hook requires Created.
        assert_eq!(
            merge(Some(ChangeKind::Created), ChangeKind::Modified),
            ChangeKind::Created,
            "Created must survive a subsequent Modified in the same burst",
        );
    }

    #[test]
    fn created_then_removed_becomes_removed() {
        // Created then immediately Removed: file is gone; Removed wins.
        assert_eq!(
            merge(Some(ChangeKind::Created), ChangeKind::Removed),
            ChangeKind::Removed,
        );
    }

    #[test]
    fn modified_then_removed_becomes_removed() {
        assert_eq!(
            merge(Some(ChangeKind::Modified), ChangeKind::Removed),
            ChangeKind::Removed,
        );
    }

    #[test]
    fn removed_is_sticky_against_anything() {
        // Once Removed is the pending kind, nothing demotes it.
        assert_eq!(merge(Some(ChangeKind::Removed), ChangeKind::Created), ChangeKind::Removed);
        assert_eq!(merge(Some(ChangeKind::Removed), ChangeKind::Modified), ChangeKind::Removed);
        assert_eq!(merge(Some(ChangeKind::Removed), ChangeKind::Removed), ChangeKind::Removed);
    }

    #[test]
    fn no_prior_takes_incoming_kind() {
        assert_eq!(merge(None, ChangeKind::Created), ChangeKind::Created);
        assert_eq!(merge(None, ChangeKind::Modified), ChangeKind::Modified);
        assert_eq!(merge(None, ChangeKind::Removed), ChangeKind::Removed);
    }

    #[test]
    fn modified_then_created_upgrades_to_created() {
        // Unusual but possible: Modify event arrives before Create (OS ordering).
        // The incoming Created must win over the prior Modified.
        assert_eq!(
            merge(Some(ChangeKind::Modified), ChangeKind::Created),
            ChangeKind::Created,
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
}
