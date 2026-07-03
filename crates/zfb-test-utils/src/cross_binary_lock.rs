//! Cross-binary advisory lock serializing the heavy V8/esbuild-booting e2e
//! test binaries (issue #1339).
//!
//! ## Why this exists
//!
//! `cargo test --workspace` runs each integration-test *binary* to
//! completion before starting the next one. That default sequential
//! binary scheduling is the ONLY thing today that keeps the 5+
//! V8+esbuild-booting e2e binaries (`dev_serve_e2e`,
//! `dev_bind_before_walk_e2e`, `dev_build_static_parity`,
//! `dev_serve_injected_routes_e2e`, `dev_public_large_tree_smoke_e2e`,
//! `dev_dep_invalidation_1284_e2e`, `build_terminates`) from booting
//! concurrently. A parallel test runner (`cargo nextest`, planned in a
//! future sub-task) schedules test *binaries* concurrently by default —
//! several real `zfb dev`/`zfb build` processes (each embedding a V8 host
//! and shelling out to esbuild) booting at once would starve each other's
//! CPU/memory and turn generous boot-deadline watchdogs flaky.
//!
//! This module makes that serialization an explicit, real mechanism
//! instead of an implicit side effect of `cargo test`'s binary
//! scheduling: an OS-level advisory file lock (`flock` on Unix,
//! `LockFileEx` on Windows, via `std::fs::File::{lock, try_lock, unlock}`
//! — stable since Rust 1.89) on a single lock file under the workspace
//! `target/` dir. Every heavy e2e test acquires it for the duration of the
//! test (or, in a multi-scenario binary, once per scenario) via
//! [`CrossBinaryE2eLock::acquire`].
//!
//! ## Poison-safety
//!
//! `flock`/`LockFileEx` locks are owned by the *open file description*
//! (Unix) / *handle* (Windows), not tracked by any separate userspace
//! "who holds this" state. The OS releases the lock the instant every fd
//! referencing that open file description is closed — which includes the
//! holding process dying for ANY reason (panic unwind, SIGKILL, abort):
//! the kernel tears down open file descriptors as part of process exit,
//! so there is nothing to "poison". [`CrossBinaryE2eLock`]'s `Drop` closes
//! the file explicitly for clarity, but even a hard kill that skips `Drop`
//! entirely still releases the lock. This is why an OS file lock (rather
//! than e.g. a lock file's mere on-disk *existence*, PID-file style) is
//! the right primitive: a PID-file lock left behind by a killed process
//! would permanently wedge every future run until someone notices and
//! deletes it by hand.
//!
//! ## Lock ordering vs. in-binary `SERIAL` mutexes
//!
//! Several adopting test files also carry a file-local
//! `static SERIAL: LazyLock<tokio::sync::Mutex<()>>` that serializes
//! multiple heavy tests *within the same binary* (see `dev_serve_e2e.rs`,
//! `dev_bind_before_walk_e2e.rs`, `dev_build_static_parity.rs`,
//! `dev_dep_invalidation_1284_e2e.rs`). Call sites MUST acquire the
//! cross-binary [`CrossBinaryE2eLock`] **before** the in-binary `SERIAL`
//! guard:
//!
//! ```ignore
//! let _e2e_lock = zfb_test_utils::CrossBinaryE2eLock::acquire();
//! let _serial = SERIAL.lock().await;
//! ```
//!
//! This order can never deadlock: `SERIAL` is a process-local static (a
//! fresh, independent copy per test-binary *process*), so no other
//! process can ever be blocked holding a given binary's `SERIAL` while
//! that binary's process is itself waiting on this lock — the two locks
//! never form a cycle. The ordering is chosen for throughput, not
//! correctness: it lets a thread queue for the (potentially minutes-long)
//! cross-process wait *before* touching the binary's cheap local mutex,
//! rather than holding `SERIAL` — and blocking every other same-binary
//! thread that only needs `SERIAL` — for the whole cross-process wait.
//!
//! ## Why not the `fs2` crate
//!
//! `fs2` is already a workspace dependency (`crates/zfb-build/Cargo.toml`,
//! added for a since-removed miniflare e2e test — it is currently unused
//! there). But `std::fs::File` has carried the same
//! `lock`/`try_lock`/`lock_shared`/`try_lock_shared`/`unlock` API natively
//! since Rust 1.89 (`zfb-css/src/engine.rs`'s `warm_oxide_cross_process`
//! already relies on it for a similar cross-process warm-up lock), so no
//! new dependency is needed here either.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Default deadline for acquiring the lock before failing loudly.
///
/// Generous on purpose: a single heavy e2e scenario can legitimately hold
/// the lock for minutes (V8 + esbuild boot plus a dozen render ticks).
/// This must comfortably exceed the slowest single scenario's own
/// wall-clock watchdog (`dev_serve_e2e.rs`'s `OVERALL_DEADLINE_LAZY` is
/// 280s) so that tripping this timeout is real evidence of a wedged
/// holder, not just a slow-but-healthy one running ahead of us.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(360);

/// Poll interval while waiting for a contended lock.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Relative path (from the workspace root) of the lock file.
const LOCK_FILE_RELATIVE_PATH: &str = "target/.e2e-serialize.lock";

/// Held guard for the cross-binary e2e advisory lock.
///
/// Keep the returned value bound (`let _guard = ...`) for the duration of
/// the section that must be serialized against sibling e2e binaries.
/// Dropping it — or the process exiting for any reason, including a panic
/// or SIGKILL — releases the underlying OS lock; see the module docs'
/// poison-safety section.
pub struct CrossBinaryE2eLock {
    file: File,
}

impl CrossBinaryE2eLock {
    /// Block (up to [`DEFAULT_TIMEOUT`]) until the cross-binary e2e lock
    /// is acquired.
    ///
    /// # Panics
    ///
    /// Panics with a diagnostic naming the lock file path and likely
    /// holders if the lock cannot be acquired within the deadline, or if
    /// the lock file cannot be opened/created at all.
    pub fn acquire() -> Self {
        Self::acquire_with_timeout(DEFAULT_TIMEOUT)
    }

    /// Like [`Self::acquire`] but with an explicit deadline. Exposed so
    /// the unit test below can prove mutual exclusion without a
    /// minutes-long wait; production call sites should use
    /// [`Self::acquire`].
    pub fn acquire_with_timeout(timeout: Duration) -> Self {
        Self::acquire_at(&lock_file_path(), timeout)
    }

    fn acquire_at(path: &Path, timeout: Duration) -> Self {
        if let Some(parent) = path.parent() {
            // Best-effort: a concurrent create_dir_all from a sibling
            // process racing us here is fine (AlreadyExists); any other
            // failure surfaces below when we try to open the file itself.
            let _ = fs::create_dir_all(parent);
        }

        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .unwrap_or_else(|e| {
                panic!(
                    "cross-binary e2e lock: failed to open/create lock file at {}: {e}",
                    path.display()
                )
            });

        let deadline = Instant::now() + timeout;
        loop {
            match file.try_lock() {
                Ok(()) => return Self { file },
                Err(TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        panic!(
                            "cross-binary e2e lock: timed out after {timeout:?} waiting to \
                             acquire {} — another heavy e2e test binary (dev_serve_e2e, \
                             dev_bind_before_walk_e2e, dev_build_static_parity, \
                             dev_serve_injected_routes_e2e, dev_public_large_tree_smoke_e2e, \
                             dev_dep_invalidation_1284_e2e, or build_terminates) is very likely \
                             still holding it. Check for a wedged `zfb dev` / `zfb build` \
                             process (or a hung/killed prior test run that left a V8 host or \
                             esbuild child alive) with `ps aux | grep zfb` and kill it, or — if \
                             this is a legitimately slow machine — call \
                             CrossBinaryE2eLock::acquire_with_timeout with a longer deadline.",
                            path.display()
                        );
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(TryLockError::Error(e)) => panic!(
                    "cross-binary e2e lock: failed to acquire lock at {}: {e}",
                    path.display()
                ),
            }
        }
    }
}

impl Drop for CrossBinaryE2eLock {
    fn drop(&mut self) {
        // Explicit for clarity; closing the File (which happens anyway
        // once this struct is dropped) releases the OS lock regardless.
        let _ = self.file.unlock();
    }
}

/// Resolve the workspace-scoped lock-file path:
/// `<workspace_root>/target/.e2e-serialize.lock`.
///
/// Reuses [`crate::candidate_workspace_roots`] (issue #1007) so a
/// `worktrees/<topic>/` checkout resolves to ITS OWN root — and therefore
/// its own, independent lock file — rather than a stale path baked into a
/// reused rlib from a different checkout.
fn lock_file_path() -> PathBuf {
    let root = crate::candidate_workspace_roots()
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            panic!(
                "cross-binary e2e lock: could not resolve a workspace root (no ancestor of \
                 the cwd or of current_exe has both a Cargo.toml file and a crates/ dir) — \
                 cannot place the lock file"
            )
        });
    root.join(LOCK_FILE_RELATIVE_PATH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Mutual exclusion, proven on timing/ordering (not just "no crash"):
    /// two threads contend for the same lock file. The first holds it for
    /// a fixed duration; the second must not observe the lock as acquired
    /// until AFTER the first releases, and must have waited at least
    /// (roughly) the first's hold duration.
    #[test]
    fn acquire_at_serializes_two_contenders() {
        let dir = std::env::temp_dir().join(format!(
            "zfb-test-utils-cross-binary-lock-test-{}-{:?}",
            std::process::id(),
            Instant::now()
        ));
        fs::create_dir_all(&dir).expect("create temp dir for lock test");
        let lock_path = dir.join("contend.lock");

        let hold_duration = Duration::from_millis(400);
        let events: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));

        // Thread 1: acquire immediately, hold for `hold_duration`, then
        // release by dropping the guard. Signals `acquired_tx` the instant
        // it has genuinely acquired the lock so the main thread can spawn
        // thread 2 only once contention is real, rather than hoping a
        // fixed sleep was long enough for thread 1 to win the race — under
        // a loaded runner it might not be, which would let thread 2 start
        // before thread 1 holds the lock and invalidate the timing
        // assertion below.
        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel::<Instant>();
        let events1 = Arc::clone(&events);
        let path1 = lock_path.clone();
        let t1 = std::thread::spawn(move || {
            let _guard = CrossBinaryE2eLock::acquire_at(&path1, Duration::from_secs(10));
            let acquired_at = Instant::now();
            events1.lock().unwrap().push("t1_acquired");
            let _ = acquired_tx.send(acquired_at);
            std::thread::sleep(hold_duration);
            events1.lock().unwrap().push("t1_released");
            // Guard drops here, releasing the OS lock.
        });

        // Block until thread 1 has genuinely acquired the lock before
        // spawning the contender.
        let t1_acquired_at = acquired_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("thread 1 acquired the lock within 10s");

        let events2 = Arc::clone(&events);
        let path2 = lock_path.clone();
        let t2_start = Instant::now();
        let t2 = std::thread::spawn(move || {
            let _guard = CrossBinaryE2eLock::acquire_at(&path2, Duration::from_secs(10));
            let waited = t2_start.elapsed();
            events2.lock().unwrap().push("t2_acquired");
            waited
        });

        t1.join().expect("t1 join");
        let t2_waited = t2.join().expect("t2 join");

        let seq = events.lock().unwrap().clone();
        assert_eq!(
            seq,
            vec!["t1_acquired", "t1_released", "t2_acquired"],
            "thread 2 must only acquire the lock after thread 1 released it"
        );

        // Thread 2 only started once thread 1 had genuinely acquired the
        // lock (`head_start` below is however long thread 1 had already
        // been holding it by the time thread 2 began waiting — just
        // signal + spawn overhead), and thread 1 held the lock for
        // `hold_duration` total. So thread 2's wait should be at least
        // `hold_duration - head_start`, with slack for scheduling jitter.
        let head_start = t2_start.saturating_duration_since(t1_acquired_at);
        let expected_min_wait = hold_duration.saturating_sub(head_start);
        assert!(
            t2_waited >= expected_min_wait.saturating_sub(Duration::from_millis(100)),
            "thread 2 acquired the lock suspiciously early: waited {t2_waited:?}, expected at \
             least ~{expected_min_wait:?} — mutual exclusion is not actually blocking"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lock_file_path_is_under_a_workspace_target_dir() {
        let path = lock_file_path();
        assert!(
            path.ends_with(LOCK_FILE_RELATIVE_PATH),
            "lock path {} must end with {LOCK_FILE_RELATIVE_PATH}",
            path.display()
        );
        assert!(
            path.parent().unwrap().file_name().unwrap() == "target",
            "lock path {} must live directly under a target/ dir",
            path.display()
        );
    }
}
