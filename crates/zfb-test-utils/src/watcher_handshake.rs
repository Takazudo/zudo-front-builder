//! Condition-keyed "watcher-live" handshake — promoted from four inline
//! implementations (`dev_serve_e2e.rs`, `dev_serve_injected_routes_e2e.rs`,
//! and two in `watch_add_confirm.rs`) into one reusable helper (#1338).
//!
//! ## The problem this replaces
//!
//! macOS FSEvents (and, to a lesser degree, Linux inotify) has a
//! per-stream startup dead window: a file CREATED in the first moments
//! after a watch is registered can be dropped entirely — no `Create`, no
//! `Modify`, nothing ever reaches the subscriber (confirmed by
//! instrumentation in `watch_add_confirm.rs`: with a 200ms warmup sleep
//! the raw notify channel received zero events for the new file). A fixed
//! `sleep(Nms)` "let the watcher warm up" pre-wait is therefore fragile:
//! under load (`cargo test --workspace`, a busy CI box) the real OS-level
//! registration can land *after* the sleep, so the first real edit still
//! races the dead window and the test flakes.
//!
//! ## The pattern
//!
//! Don't guess a duration — prove the stream is live before the real
//! edit. Repeatedly write a FRESH-NAMED marker file (the same operation
//! class as the edit under test — a brand-new create) and poll a
//! readiness predicate until it observes an event. A fresh name each
//! iteration is essential:
//!
//! - re-writing one path only fires `Modify` (it never re-triggers a
//!   create-keyed discovery hook), and
//! - a single warmup's own first create could itself be the one the dead
//!   window drops.
//!
//! Once ANY marker event is observed the stream is past its dead window
//! and stays live (startup latency is per-stream, not per-path), so the
//! subsequent real edit lands in the live window.
//!
//! ## Why it lives here, and stays crate-agnostic
//!
//! `zfb-watcher`, `zfb-server`, and `zfb-build` tests all need this, but
//! `zfb-test-utils` must NOT depend on `zfb-watcher` (that would risk a
//! dependency cycle). So the helper knows nothing about watchers,
//! channels, or SSE: it is generic over two closures —
//!
//! - `write_marker(idx)` — the caller writes its own fresh-named file
//!   wherever its watched root is (the `idx` gives it a unique name), and
//! - `signal_seen()` — the caller polls its own event source: an mpsc
//!   `try_recv().is_ok()`, a shared discovery-hook invocation log, an
//!   atomic reload counter, whatever it has.
//!
//! The helper owns only the loop, the write/poll cadence, and the
//! deadline.

use std::time::{Duration, Instant};

/// Cadence + deadline knobs for [`watcher_live_handshake`].
///
/// Defaults are tuned for the zfb watcher paths: `marker_interval` sits
/// comfortably above the largest debounce window in the test suite
/// (200ms) so each fresh marker gets its own debounced emit, and
/// `poll_interval` is fine enough to notice the first event promptly
/// without busy-spinning.
#[derive(Debug, Clone)]
pub struct HandshakeOpts {
    /// Overall wall-clock budget. When it elapses the handshake gives up
    /// and returns `live: false` so a genuinely dead watcher fails fast
    /// and diagnosably rather than hanging the suite.
    pub deadline: Duration,
    /// How often to write a fresh marker file. Must exceed the watcher's
    /// debounce window so each marker coalesces into its own emit.
    pub marker_interval: Duration,
    /// How often to re-check `signal_seen` between marker writes.
    pub poll_interval: Duration,
}

impl HandshakeOpts {
    /// New options with the given overall `deadline` and default
    /// cadences (400ms marker interval, 25ms poll interval).
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

    /// Override the predicate-poll interval.
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }
}

impl Default for HandshakeOpts {
    fn default() -> Self {
        Self::new(Duration::from_secs(10))
    }
}

/// Outcome of a handshake attempt.
///
/// Deliberately not an `Err` on timeout: the caller supplies the panic
/// message (it knows what a dead watcher means in its context) and gets
/// the diagnostics — `markers_written` and `elapsed` — to put in it.
#[derive(Debug, Clone)]
pub struct HandshakeResult {
    /// `true` if `signal_seen` observed an event before the deadline.
    pub live: bool,
    /// How many marker files were written before the loop ended.
    pub markers_written: u32,
    /// Wall-clock time spent in the handshake.
    pub elapsed: Duration,
}

/// Drive marker writes until a readiness signal is observed or the
/// deadline elapses.
///
/// Writes a fresh marker (`write_marker(idx)` with a monotonically
/// increasing `idx`), then polls `signal_seen` at `poll_interval` until
/// either it returns `true`, `marker_interval` elapses (time for the next
/// fresh marker), or the overall `deadline` is hit.
///
/// `signal_seen` is checked once up front so an already-live watcher
/// (e.g. warmed by an earlier phase of the same test) returns immediately
/// with `markers_written == 0`.
///
/// Both closures are `FnMut` and synchronous; a marker write is a fast
/// local `std::fs::write` (the caller `.expect`s inside on failure), and
/// a poll is a non-blocking `try_recv` / lock-and-scan. Only the cadence
/// waits are `await`ed, so this works on both current-thread and
/// multi-thread tokio runtimes.
pub async fn watcher_live_handshake<W, S>(
    opts: HandshakeOpts,
    mut write_marker: W,
    mut signal_seen: S,
) -> HandshakeResult
where
    W: FnMut(u32),
    S: FnMut() -> bool,
{
    let start = Instant::now();

    // Fast path — the stream may already be live.
    if signal_seen() {
        return HandshakeResult {
            live: true,
            markers_written: 0,
            elapsed: start.elapsed(),
        };
    }

    let mut markers: u32 = 0;
    loop {
        if start.elapsed() >= opts.deadline {
            return HandshakeResult {
                live: false,
                markers_written: markers,
                elapsed: start.elapsed(),
            };
        }

        // Fresh-named marker each iteration (see module docs: a reused
        // name only fires Modify and its own create could be dead-windowed).
        write_marker(markers);
        markers += 1;

        // Poll finely until it's time for the next marker or we run out
        // of budget.
        let next_marker_at = Instant::now() + opts.marker_interval;
        loop {
            if signal_seen() {
                return HandshakeResult {
                    live: true,
                    markers_written: markers,
                    elapsed: start.elapsed(),
                };
            }
            let now = Instant::now();
            if start.elapsed() >= opts.deadline {
                return HandshakeResult {
                    live: false,
                    markers_written: markers,
                    elapsed: start.elapsed(),
                };
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

// ---------------------------------------------------------------------------
// Unit tests — exercise the loop with a simulated event source so the
// helper is covered WITHOUT a real watcher (and without a zfb-watcher dep).
// ---------------------------------------------------------------------------

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

    #[test]
    fn becomes_live_after_markers_and_reports_count() {
        rt().block_on(async {
            // The simulated watcher "delivers an event" once the third
            // marker has been written.
            let writes = Arc::new(AtomicU32::new(0));
            let writes_w = Arc::clone(&writes);
            let seen = Arc::clone(&writes);

            let opts = HandshakeOpts::new(Duration::from_secs(5))
                .with_marker_interval(Duration::from_millis(10))
                .with_poll_interval(Duration::from_millis(1));

            let res = watcher_live_handshake(
                opts,
                move |_idx| {
                    writes_w.fetch_add(1, Ordering::SeqCst);
                },
                move || seen.load(Ordering::SeqCst) >= 3,
            )
            .await;

            assert!(res.live, "handshake should observe the signal");
            assert_eq!(
                res.markers_written, 3,
                "predicate flips exactly when the 3rd marker is written"
            );
        });
    }

    #[test]
    fn already_live_writes_no_markers() {
        rt().block_on(async {
            let res = watcher_live_handshake(
                HandshakeOpts::default(),
                |_idx| panic!("must not write any marker when already live"),
                || true,
            )
            .await;

            assert!(res.live);
            assert_eq!(res.markers_written, 0, "fast path writes zero markers");
        });
    }

    #[test]
    fn times_out_when_signal_never_arrives() {
        rt().block_on(async {
            let writes = Arc::new(AtomicU32::new(0));
            let writes_w = Arc::clone(&writes);

            let opts = HandshakeOpts::new(Duration::from_millis(120))
                .with_marker_interval(Duration::from_millis(20))
                .with_poll_interval(Duration::from_millis(5));

            let res = watcher_live_handshake(
                opts,
                move |_idx| {
                    writes_w.fetch_add(1, Ordering::SeqCst);
                },
                || false,
            )
            .await;

            assert!(!res.live, "signal never arrives → not live");
            assert!(
                res.markers_written > 0,
                "the loop keeps writing markers while it waits (wrote {})",
                res.markers_written
            );
            assert!(
                res.elapsed >= Duration::from_millis(120),
                "must honour the full deadline before giving up (elapsed {:?})",
                res.elapsed
            );
        });
    }
}
