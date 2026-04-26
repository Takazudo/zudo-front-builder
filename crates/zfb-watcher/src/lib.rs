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
/// flush to the receiver before the task exits, then aborts the task as
/// a fallback if it does not finish promptly.
///
/// Construct via [`Watcher::start`], which returns this handle plus the
/// receiver end of the `Change` channel.
pub struct Watcher {
    // Kept alive: dropping the notify watcher stops the OS-level watch.
    _notify: RecommendedWatcher,
    // Some(_) until Drop fires the shutdown signal.
    shutdown: Option<oneshot::Sender<()>>,
    // Aborted on drop as a fallback so the debouncer task does not
    // outlive the handle indefinitely.
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

impl Drop for Watcher {
    fn drop(&mut self) {
        // Best-effort graceful shutdown: signal the debouncer to flush
        // its pending map and exit. If we are not on a tokio runtime (or
        // the receiver is gone), the JoinHandle::abort() below ensures
        // the task is reaped regardless. We deliberately do NOT block
        // here — Drop must stay non-blocking.
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.debouncer.take() {
            h.abort();
        }
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

            // Shutdown signal from Drop: flush whatever we have and exit
            // before the JoinHandle::abort() in Drop forcibly cancels us.
            _ = &mut shutdown => {
                flush_all(&mut pending, &out_tx).await;
                break;
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
                            // For a given burst we keep the *latest* kind.
                            // A Created followed by Modified collapses to
                            // Modified, which is fine: the consumer will
                            // re-read the file either way. A trailing
                            // Removed wins, which is correct (file is gone).
                            pending.insert(path, Pending { kind, last_seen: now });
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
}
