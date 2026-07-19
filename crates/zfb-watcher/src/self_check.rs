//! Watcher liveness self-check — a production primitive for detecting a
//! never-live FSEvents/inotify stream (issue #1687, epic #1716).
//!
//! ## Background
//!
//! On some real macOS hosts the FSEvents stream backing a [`Watcher`]
//! never starts delivering events at all, so `zfb dev` silently never
//! hot-reloads. The detection technique already exists in test form as
//! `zfb_test_utils::watcher_live_handshake`: write fresh-named marker
//! files on an interval and poll for the watcher observing them, giving
//! up after a deadline. This module ports that loop shape into a
//! first-class, production-facing primitive built directly on the public
//! [`Watcher`] API.
//!
//! `zfb-watcher` deliberately does NOT reuse `zfb-test-utils`'s handshake
//! helper for this: `zfb-test-utils` is a dev-only dependency of this
//! crate (see `Cargo.toml`), so it is not available to the production code
//! path `check_liveness` runs on — only to the crate's own tests. (There
//! is no dependency cycle either way: `zfb-test-utils` does not depend on
//! `zfb-watcher`.) The loop shape is therefore duplicated here as its own
//! private, generic seam instead of shared.
//!
//! ## API shape
//!
//! The caller supplies a probe directory (expected to be a scratch
//! location dedicated to this check — nothing else should read or write
//! it concurrently); this module owns a dedicated [`Watcher`] rooted
//! there plus the handshake loop, and reports a three-way outcome:
//!
//! - [`LivenessOutcome::Live`] — the probe watcher observed a marker
//!   before the deadline. The real dev-server watcher is presumed
//!   healthy too (same underlying OS notification mechanism).
//! - [`LivenessOutcome::TimedOut`] — no marker was observed before the
//!   deadline. This is the "dead FSEvents stream" verdict.
//! - [`LivenessOutcome::SetupFailed`] — probe-directory creation, watcher
//!   startup, or cleanup failed. This is deliberately distinct from
//!   `TimedOut`: a setup failure says nothing about whether the OS
//!   notification mechanism itself is alive, so callers must not treat
//!   it as a dead-watcher verdict.

use std::path::Path;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::error::TryRecvError;
use tracing::warn;

use crate::Watcher;

/// What the injected signal source reports on a single poll from
/// [`run_liveness_loop`].
enum LivenessSignal {
    /// A marker-triggered change was observed — the probe watcher is live.
    Observed,
    /// Nothing observed yet; the loop should keep waiting.
    Waiting,
    /// The signal source is permanently gone — the probe watcher's own
    /// channel disconnected (its backend task died), so no further signal
    /// can ever arrive. Distinct from `Waiting`: waiting longer is futile.
    SourceGone,
}

/// Terminal result of [`run_liveness_loop`].
enum LoopOutcome {
    /// The signal source reported [`LivenessSignal::Observed`] before the
    /// deadline.
    Observed,
    /// The deadline elapsed with the source still only [`LivenessSignal::Waiting`].
    TimedOut,
    /// The signal source reported [`LivenessSignal::SourceGone`] before the
    /// deadline — the probe watcher backend died.
    SourceGone,
}

/// Cadence + deadline knobs for [`check_liveness`].
///
/// Mirrors `zfb_test_utils::watcher_handshake::HandshakeOpts` in shape
/// (deadline + marker-write interval + poll interval) but is an
/// independent copy — see the module docs for why this crate does not
/// share that type.
#[derive(Debug, Clone)]
pub struct LivenessOpts {
    /// Overall wall-clock budget. When it elapses without observing a
    /// marker, the check gives up and reports [`LivenessOutcome::TimedOut`].
    pub deadline: Duration,
    /// How often to write a fresh marker file. Should exceed the probe
    /// watcher's debounce window so each marker coalesces into its own
    /// emit.
    pub marker_interval: Duration,
    /// How often to re-check for an observed marker between writes.
    pub poll_interval: Duration,
}

impl LivenessOpts {
    /// New options with the given overall `deadline` and default
    /// cadences (400ms marker interval, 25ms poll interval) — the same
    /// defaults as the test-only handshake helper.
    pub fn new(deadline: Duration) -> Self {
        Self {
            deadline,
            marker_interval: Duration::from_millis(400),
            poll_interval: Duration::from_millis(25),
        }
    }

    /// Override the marker-write interval.
    pub fn with_marker_interval(mut self, interval: Duration) -> Self {
        self.marker_interval = interval;
        self
    }

    /// Override the marker-observed poll interval.
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }
}

impl Default for LivenessOpts {
    fn default() -> Self {
        Self::new(Duration::from_secs(10))
    }
}

/// Outcome of a [`check_liveness`] run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LivenessOutcome {
    /// A marker write was observed by the probe watcher before the
    /// deadline elapsed.
    Live,
    /// No marker was observed before the deadline — the probable
    /// "dead FSEvents/inotify stream" case (issue #1687).
    TimedOut,
    /// Probe-directory creation, watcher startup, or post-check cleanup
    /// failed. Distinct from `TimedOut`: this says nothing about whether
    /// the underlying OS notification mechanism is alive, so it must
    /// never be reported as a dead-watcher verdict. Carries a
    /// human-readable description of what failed.
    SetupFailed(String),
}

/// Run a watcher-liveness self-check rooted at `probe_dir`.
///
/// `probe_dir` is created if missing. If it already existed, its
/// pre-existing contents are left alone — cleanup on the way out removes
/// only the marker files this check wrote, never the whole directory; if
/// this call created the directory itself, the whole thing is removed
/// again, best-effort. A dedicated [`Watcher`] is started there and
/// fresh-named marker files are written until either a resulting change
/// is observed (`Live`) or `opts.deadline` elapses (`TimedOut`).
///
/// Note this dedicated probe watcher is a distinct `notify` subscription
/// from any watcher the caller runs elsewhere (e.g. the dev server's own
/// watcher) — it is a canary for the same underlying OS notification
/// mechanism (FSEvents/inotify), not a check of one specific caller-owned
/// stream. That is an intentional layering choice (issue #1717): this
/// primitive must not reach into a caller's already-running `Watcher`.
///
/// Any failure setting up the probe (directory creation, watcher
/// startup) short-circuits to `SetupFailed` without ever attempting the
/// handshake loop. A marker-write failure does not immediately abort the
/// loop (the loop is still given its full deadline to try again), but if
/// the deadline elapses and NOT ONE marker write ever succeeded, the
/// verdict is `SetupFailed` rather than `TimedOut` — with nothing
/// observed to write, a `TimedOut` verdict would misreport a disk/
/// permission problem as a dead FSEvents/inotify stream. A failure
/// tearing the probe back down after a completed handshake is logged and
/// otherwise ignored — it must not overwrite a `Live`/`TimedOut`/
/// `SetupFailed` verdict that was already determined.
///
/// This function never panics: every fallible step (directory creation,
/// watcher startup, marker writes, cleanup) is handled explicitly.
pub async fn check_liveness<P: AsRef<Path>>(probe_dir: P, opts: LivenessOpts) -> LivenessOutcome {
    let probe_dir = probe_dir.as_ref();
    let dir_pre_existed = probe_dir.exists();

    if let Err(error) = std::fs::create_dir_all(probe_dir) {
        return LivenessOutcome::SetupFailed(format!(
            "failed to create watcher liveness probe directory {}: {error}",
            probe_dir.display()
        ));
    }

    // Small debounce: this is a dedicated, single-purpose probe directory,
    // so we want the fastest possible signal rather than editor-save-burst
    // coalescing.
    let (watcher, mut rx) = match Watcher::start_with_debounce(
        probe_dir,
        std::iter::once("."),
        Duration::from_millis(10),
    ) {
        Ok(pair) => pair,
        Err(error) => {
            return LivenessOutcome::SetupFailed(format!(
                "failed to start watcher liveness probe rooted at {}: {error}",
                probe_dir.display()
            ));
        }
    };

    let mut any_write_succeeded = false;
    let mut last_write_error: Option<String> = None;
    let mut written_markers: Vec<std::path::PathBuf> = Vec::new();

    let loop_outcome = run_liveness_loop(
        &opts,
        |idx| {
            let marker = probe_dir.join(format!(".zfb-liveness-probe-{idx}"));
            match std::fs::write(&marker, b"x") {
                Ok(()) => {
                    any_write_succeeded = true;
                    written_markers.push(marker);
                }
                Err(error) => {
                    // A write failure here is usually a disk/permission
                    // problem, not a dead-watcher signal — log and let the
                    // loop keep trying (an intermittent failure with later
                    // successes still yields a trustworthy Live/TimedOut
                    // verdict). If EVERY attempt fails, the check below
                    // reclassifies the final verdict as SetupFailed instead
                    // of TimedOut.
                    warn!(
                        path = %marker.display(),
                        %error,
                        "failed to write watcher liveness probe marker"
                    );
                    last_write_error = Some(format!(
                        "failed to write watcher liveness probe marker {}: {error}",
                        marker.display()
                    ));
                }
            }
        },
        || match rx.try_recv() {
            Ok(_) => LivenessSignal::Observed,
            Err(TryRecvError::Empty) => LivenessSignal::Waiting,
            // The probe watcher's channel disconnected — its backend task
            // died, so no marker will ever be delivered. This is a setup/
            // infrastructure failure, NOT a dead OS-notification verdict.
            Err(TryRecvError::Disconnected) => LivenessSignal::SourceGone,
        },
    )
    .await;

    watcher.shutdown().await;

    if dir_pre_existed {
        // Caller-owned directory: remove only the marker files this check
        // wrote, never the directory or its pre-existing contents.
        for marker in &written_markers {
            let _ = std::fs::remove_file(marker);
        }
    } else if let Err(error) = std::fs::remove_dir_all(probe_dir) {
        warn!(
            path = %probe_dir.display(),
            %error,
            "failed to clean up watcher liveness probe directory"
        );
    }

    match loop_outcome {
        LoopOutcome::Observed => LivenessOutcome::Live,
        // The probe watcher's channel disconnected before any marker was
        // observed: report a setup failure, never a `TimedOut` dead-watcher
        // verdict — the backend dying says nothing about whether a healthy
        // FSEvents/inotify stream would have delivered the event.
        LoopOutcome::SourceGone => LivenessOutcome::SetupFailed(
            "watcher liveness probe channel disconnected before observing a marker: \
             the probe watcher backend stopped"
                .to_string(),
        ),
        LoopOutcome::TimedOut => {
            if !any_write_succeeded {
                LivenessOutcome::SetupFailed(last_write_error.unwrap_or_else(|| {
                    "no watcher liveness probe marker could be written before the deadline"
                        .to_string()
                }))
            } else {
                LivenessOutcome::TimedOut
            }
        }
    }
}

/// Private generic handshake loop — deadline + injected `signal_seen`
/// closure, deliberately decoupled from any real filesystem or watcher so
/// the never-fires (`TimedOut`) case is unit-testable without one.
/// Mirrors the shape of `zfb_test_utils::watcher_live_handshake` (see the
/// module docs for why that helper isn't reused directly).
///
/// `signal_seen` is checked once up front so an already-live signal
/// source returns [`LoopOutcome::Observed`] immediately without writing any
/// marker. A [`LivenessSignal::SourceGone`] report short-circuits to
/// [`LoopOutcome::SourceGone`] the moment it is seen — waiting out the
/// remaining deadline would be futile once the source is permanently gone.
async fn run_liveness_loop<W, S>(
    opts: &LivenessOpts,
    mut write_marker: W,
    mut signal_seen: S,
) -> LoopOutcome
where
    W: FnMut(u32),
    S: FnMut() -> LivenessSignal,
{
    let start = Instant::now();

    match signal_seen() {
        LivenessSignal::Observed => return LoopOutcome::Observed,
        LivenessSignal::SourceGone => return LoopOutcome::SourceGone,
        LivenessSignal::Waiting => {}
    }

    let mut markers: u32 = 0;
    loop {
        if start.elapsed() >= opts.deadline {
            return LoopOutcome::TimedOut;
        }

        write_marker(markers);
        markers += 1;

        let next_marker_at = Instant::now() + opts.marker_interval;
        loop {
            match signal_seen() {
                LivenessSignal::Observed => return LoopOutcome::Observed,
                LivenessSignal::SourceGone => return LoopOutcome::SourceGone,
                LivenessSignal::Waiting => {}
            }
            let now = Instant::now();
            if start.elapsed() >= opts.deadline {
                return LoopOutcome::TimedOut;
            }
            if now >= next_marker_at {
                break;
            }
            let nap = next_marker_at
                .saturating_duration_since(now)
                .min(opts.poll_interval);
            tokio::time::sleep(nap).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    // -----------------------------------------------------------------
    // Private-loop unit tests — simulated signal source, no real fs.
    // -----------------------------------------------------------------

    #[test]
    fn loop_times_out_when_signal_never_arrives() {
        rt().block_on(async {
            let writes = Arc::new(AtomicU32::new(0));
            let writes_w = Arc::clone(&writes);

            let opts = LivenessOpts::new(Duration::from_millis(120))
                .with_marker_interval(Duration::from_millis(20))
                .with_poll_interval(Duration::from_millis(5));

            let outcome = run_liveness_loop(
                &opts,
                move |_idx| {
                    writes_w.fetch_add(1, Ordering::SeqCst);
                },
                || LivenessSignal::Waiting,
            )
            .await;

            assert!(
                matches!(outcome, LoopOutcome::TimedOut),
                "signal never arrives -> timed out"
            );
            assert!(
                writes.load(Ordering::SeqCst) > 0,
                "the loop keeps writing markers while it waits"
            );
        });
    }

    #[test]
    fn loop_reports_live_once_signal_is_observed() {
        rt().block_on(async {
            let writes = Arc::new(AtomicU32::new(0));
            let writes_w = Arc::clone(&writes);
            let seen = Arc::clone(&writes);

            let opts = LivenessOpts::new(Duration::from_secs(5))
                .with_marker_interval(Duration::from_millis(10))
                .with_poll_interval(Duration::from_millis(1));

            let outcome = run_liveness_loop(
                &opts,
                move |_idx| {
                    writes_w.fetch_add(1, Ordering::SeqCst);
                },
                move || {
                    if seen.load(Ordering::SeqCst) >= 3 {
                        LivenessSignal::Observed
                    } else {
                        LivenessSignal::Waiting
                    }
                },
            )
            .await;

            assert!(
                matches!(outcome, LoopOutcome::Observed),
                "handshake should observe the simulated signal"
            );
        });
    }

    #[test]
    fn loop_already_live_writes_no_markers() {
        rt().block_on(async {
            let outcome = run_liveness_loop(
                &LivenessOpts::default(),
                |_idx| panic!("must not write any marker when already live"),
                || LivenessSignal::Observed,
            )
            .await;

            assert!(matches!(outcome, LoopOutcome::Observed));
        });
    }

    #[test]
    fn loop_reports_source_gone_when_signal_source_disconnects() {
        rt().block_on(async {
            // A source that is permanently gone from the first poll must
            // short-circuit to `SourceGone` — never `TimedOut` — so the
            // caller can map it to `SetupFailed` rather than a dead-watcher
            // verdict. Uses a long deadline to prove the loop returns
            // immediately rather than waiting it out.
            let outcome = run_liveness_loop(
                &LivenessOpts::new(Duration::from_secs(30)),
                |_idx| {},
                || LivenessSignal::SourceGone,
            )
            .await;

            assert!(matches!(outcome, LoopOutcome::SourceGone));
        });
    }

    /// A source that only reports `SourceGone` AFTER a few `Waiting` polls
    /// (the realistic case: the backend task dies mid-handshake) must still
    /// short-circuit to `SourceGone` from inside the wait loop, not run out
    /// the deadline as a `TimedOut`.
    #[test]
    fn loop_reports_source_gone_when_source_disconnects_mid_handshake() {
        rt().block_on(async {
            let polls = Arc::new(AtomicU32::new(0));
            let polls_s = Arc::clone(&polls);

            let opts = LivenessOpts::new(Duration::from_secs(30))
                .with_marker_interval(Duration::from_millis(20))
                .with_poll_interval(Duration::from_millis(1));

            let outcome = run_liveness_loop(
                &opts,
                |_idx| {},
                move || {
                    if polls_s.fetch_add(1, Ordering::SeqCst) >= 3 {
                        LivenessSignal::SourceGone
                    } else {
                        LivenessSignal::Waiting
                    }
                },
            )
            .await;

            assert!(matches!(outcome, LoopOutcome::SourceGone));
        });
    }

    // -----------------------------------------------------------------
    // `check_liveness` — setup-failure path.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn setup_failure_is_distinct_from_timed_out() {
        // Create a plain FILE, then ask check_liveness to treat a path
        // *underneath* it as the probe directory. create_dir_all must
        // fail deterministically (a regular file can't have children) —
        // this exercises the directory-creation failure branch fast and
        // without ever touching the real watcher/FSEvents machinery, so
        // it cannot be confused with a slow-and-eventually-timed-out
        // probe.
        let tmp = tempfile::tempdir().expect("tempdir");
        let blocking_file = tmp.path().join("not-a-directory");
        std::fs::write(&blocking_file, b"x").expect("seed blocking file");
        let probe_dir = blocking_file.join("probe");

        let outcome =
            check_liveness(&probe_dir, LivenessOpts::new(Duration::from_millis(50))).await;

        match outcome {
            LivenessOutcome::SetupFailed(_) => {}
            other => panic!("expected SetupFailed, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // Real-fs smoke test — Live fast on a healthy host.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn live_fast_on_a_healthy_host() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let probe_dir = tmp.path().join("liveness-probe");

        let outcome = check_liveness(
            &probe_dir,
            LivenessOpts::new(Duration::from_secs(10))
                .with_marker_interval(Duration::from_millis(100))
                .with_poll_interval(Duration::from_millis(10)),
        )
        .await;

        assert_eq!(
            outcome,
            LivenessOutcome::Live,
            "a healthy host should observe its own probe marker well within 10s"
        );
    }

    /// A caller may pass a `probe_dir` that already exists with content of
    /// its own (the API contract explicitly allows this). Cleanup must
    /// remove only the marker files this check wrote, never the directory
    /// or its pre-existing contents.
    #[tokio::test]
    async fn preserves_pre_existing_probe_directory_contents() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let probe_dir = tmp.path().join("liveness-probe");
        std::fs::create_dir_all(&probe_dir).expect("create probe dir");
        let caller_owned = probe_dir.join("caller-owned.txt");
        std::fs::write(&caller_owned, b"do not delete me").expect("seed caller file");

        let outcome = check_liveness(
            &probe_dir,
            LivenessOpts::new(Duration::from_secs(10))
                .with_marker_interval(Duration::from_millis(100))
                .with_poll_interval(Duration::from_millis(10)),
        )
        .await;

        assert_eq!(outcome, LivenessOutcome::Live);
        assert!(
            caller_owned.exists(),
            "pre-existing probe-directory contents must survive the check"
        );
        assert!(
            probe_dir.exists(),
            "a pre-existing probe directory must not be removed"
        );
    }

    /// If every marker write fails (e.g. the probe location becomes
    /// read-only mid-run), no notification could ever have been
    /// generated — the verdict must be `SetupFailed`, not `TimedOut`,
    /// or a disk/permission problem would misreport as a dead
    /// FSEvents/inotify stream.
    #[cfg(unix)]
    #[tokio::test]
    async fn all_marker_writes_failing_yields_setup_failed_not_timed_out() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("tempdir");
        let probe_dir = tmp.path().join("liveness-probe");
        std::fs::create_dir_all(&probe_dir).expect("create probe dir");

        let mut perms = std::fs::metadata(&probe_dir)
            .expect("stat probe dir")
            .permissions();
        perms.set_mode(0o555);
        std::fs::set_permissions(&probe_dir, perms).expect("chmod probe dir read-only");

        // Some environments (root, certain sandboxes) bypass directory
        // permission checks entirely. Confirm enforcement actually blocks
        // a write here before asserting on it, so this test skips rather
        // than false-failing when it can't exercise the path it targets.
        let enforced = std::fs::write(probe_dir.join("canary"), b"x").is_err();

        if enforced {
            let outcome = check_liveness(
                &probe_dir,
                LivenessOpts::new(Duration::from_millis(200))
                    .with_marker_interval(Duration::from_millis(30))
                    .with_poll_interval(Duration::from_millis(10)),
            )
            .await;

            match outcome {
                LivenessOutcome::SetupFailed(_) => {}
                other => {
                    panic!("expected SetupFailed when every marker write fails, got {other:?}")
                }
            }
        }

        // Restore permissions so the tempdir's own Drop cleanup can
        // remove it.
        let mut perms = std::fs::metadata(&probe_dir)
            .expect("stat probe dir")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&probe_dir, perms).expect("chmod probe dir restore");
    }
}
