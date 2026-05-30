//! Thread-join with a deadline.
//!
//! [`join_with_deadline`] wraps an arbitrary [`std::thread::JoinHandle`] and
//! returns either the thread's return value (on a clean join within the
//! deadline) or `None` (on timeout or panic), **without blocking the caller
//! indefinitely**.  On timeout the watcher thread and the original handle are
//! both detached; the OS reaps them on process exit.
//!
//! The module is unconditionally compiled (no `embed_v8` gate) so its unit
//! tests run in every CI job, including `--no-default-features`.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Join `handle` with a deadline.
///
/// Spawns a watcher thread that calls `handle.join()` and forwards the result
/// over a channel.  The caller waits on that channel for at most `timeout`.
///
/// Returns:
/// - `Some(value)` — the thread finished and returned `value` within the
///   deadline.
/// - `None` — timeout elapsed (or the thread panicked).  The watcher thread
///   and the original handle are detached; the OS cleans them up on exit.
pub fn join_with_deadline<T: Send + 'static>(
    handle: thread::JoinHandle<T>,
    timeout: Duration,
) -> Option<T> {
    let (tx, rx) = mpsc::channel::<thread::Result<T>>();
    thread::spawn(move || {
        let _ = tx.send(handle.join());
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(value)) => Some(value),
        // Thread panicked or timed out / disconnected — detach.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc as std_mpsc;
    use std::time::Duration;

    #[test]
    fn happy_path_returns_some() {
        let handle = thread::spawn(|| 42u32);
        let result = join_with_deadline(handle, Duration::from_secs(5));
        assert_eq!(result, Some(42));
    }

    #[test]
    fn timeout_returns_none_without_hanging() {
        // Spawn a thread that blocks forever via a never-sent channel receive.
        let (_tx, rx) = std_mpsc::channel::<()>();
        let handle = thread::spawn(move || {
            // park() can return spuriously; recv() on a live channel cannot.
            let _ = rx.recv();
        });
        // A short timeout — should complete quickly (not block for ~5 s).
        let result = join_with_deadline(handle, Duration::from_millis(100));
        assert!(result.is_none());
    }
}
