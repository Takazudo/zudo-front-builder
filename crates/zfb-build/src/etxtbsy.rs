//! Bounded ETXTBSY ("Text file busy", raw OS error 26) spawn retries, shared
//! by this crate's two esbuild spawn sites: [`crate::plugin_bundler`]'s async
//! plugin-bundle spawn (issue #2378) and `crate::adapter::run_capturing`'s
//! blocking spawn, which is what the main bundler's `run_esbuild` goes
//! through (issue #2380).
//!
//! [`crate::etxtbsy::spawn_with_etxtbsy_retry_blocking`] is additionally
//! exported to `zfb-islands`, whose esbuild version gate is a fourth spawn
//! of the same just-staged binary (issue #2896). Nothing else here is
//! public: the budget, the classification predicate, the backoff, and the
//! async form stay crate-internal so external callers cannot fork the
//! policy.
//!
//! # The race
//!
//! The esbuild binary being spawned may have JUST been written — a packaged
//! `zfb` extracts the embedded binary into a tempdir before spawning it. If
//! an unrelated thread forks while that write's fd is briefly open, the
//! forked child holds the fd until its own `execve`, and our `execve` of the
//! freshly-written file fails with ETXTBSY. The window is fork-to-exec sized
//! (microseconds), so a handful of linearly backed-off retries absorbs it
//! without delaying genuine failures.
//!
//! # Why retrying is side-effect free
//!
//! The discriminator guarantees it: ETXTBSY can only originate from the
//! `execve` inside `spawn()`, so a matching error means the child never ran.
//! Every other error — including the `NotFound` of a missing binary —
//! returns on the first attempt, so callers' error wording and latency are
//! unchanged.
//!
//! `zfb-config-loader` fixed this same race class for its own subprocess
//! spawns first (zfb#1008); its `output_bounded_with` is shaped around
//! spawn-and-collect-`Output` and owns a timeout too, so it could not be
//! reused here. These constants deliberately match its budget and backoff.

use std::time::Duration;

/// Maximum ETXTBSY retries. Total attempts are `ETXTBSY_MAX_RETRIES + 1`.
pub(crate) const ETXTBSY_MAX_RETRIES: u32 = 5;

/// Backoff unit. Delays grow linearly: 10ms, 20ms, …, 50ms (150ms total).
pub(crate) const ETXTBSY_RETRY_DELAY: Duration = Duration::from_millis(10);

/// True iff `err` is an ETXTBSY that still has retry budget left.
///
/// The single classification both loops below share — so the async and
/// blocking paths can never drift on which errors are retryable or on how
/// many attempts they get.
fn is_retryable(err: &std::io::Error, retries_used: u32) -> bool {
    err.kind() == std::io::ErrorKind::ExecutableFileBusy && retries_used < ETXTBSY_MAX_RETRIES
}

/// Backoff before retry number `retry` (1-based).
fn backoff(retry: u32) -> Duration {
    ETXTBSY_RETRY_DELAY * retry
}

/// Async form: drive `attempt` until it returns anything other than a
/// retryable ETXTBSY. Used by the tokio-based plugin-bundle spawn.
pub(crate) async fn spawn_with_etxtbsy_retry<T, F>(mut attempt: F) -> std::io::Result<T>
where
    F: FnMut() -> std::io::Result<T>,
{
    let mut retries_used = 0u32;
    loop {
        match attempt() {
            Err(err) if is_retryable(&err, retries_used) => {
                retries_used += 1;
                tokio::time::sleep(backoff(retries_used)).await;
            }
            other => return other,
        }
    }
}

/// Blocking form, for callers that are synchronous end to end.
///
/// `crate::adapter::run_capturing` already blocks its thread for up to
/// 300s waiting on the child, so a bounded `thread::sleep` here changes
/// nothing about its concurrency shape. `zfb-islands`' version gate is
/// likewise synchronous, and this is the only item it needs.
///
/// `attempt` must perform exactly one spawn and nothing else: the retry is
/// side-effect free only because ETXTBSY can originate solely from the
/// `execve` inside `spawn()`, so waiting on the child belongs outside.
pub fn spawn_with_etxtbsy_retry_blocking<T, F>(mut attempt: F) -> std::io::Result<T>
where
    F: FnMut() -> std::io::Result<T>,
{
    let mut retries_used = 0u32;
    loop {
        match attempt() {
            Err(err) if is_retryable(&err, retries_used) => {
                retries_used += 1;
                std::thread::sleep(backoff(retries_used));
            }
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Both loops are driven through their injectable `attempt` seam: a real
    // ETXTBSY is a fork-to-exec-sized race no test can induce on demand.
    // The async tests use virtual time (`start_paused`) so the backoff is
    // instant; the blocking ones sleep for real, bounded by 150ms.

    fn etxtbsy() -> std::io::Error {
        std::io::Error::from(std::io::ErrorKind::ExecutableFileBusy)
    }

    /// The retry's discriminator is `io::Error::kind()`, but what the kernel
    /// hands back is raw errno 26. Nothing else here makes that mapping
    /// falsifiable: were it to change, `is_retryable` would simply stop
    /// matching and the flake it fixes would return with every test still
    /// green. Unix-only — `ExecutableFileBusy` has no Windows counterpart,
    /// and the retry branch is dead code there by construction.
    #[cfg(unix)]
    #[test]
    fn raw_os_error_26_is_classified_as_executable_file_busy() {
        assert_eq!(
            std::io::Error::from_raw_os_error(26).kind(),
            std::io::ErrorKind::ExecutableFileBusy,
            "ETXTBSY (errno 26) must map to the ErrorKind the retry guard matches on"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn async_retry_absorbs_transient_etxtbsy_and_then_succeeds() {
        let mut attempts = 0u32;
        let result = spawn_with_etxtbsy_retry(|| {
            attempts += 1;
            if attempts < 3 {
                Err(etxtbsy())
            } else {
                Ok("spawned")
            }
        })
        .await;

        assert_eq!(result.expect("should succeed after retries"), "spawned");
        assert_eq!(attempts, 3, "expected 2 ETXTBSY attempts plus 1 success");
    }

    #[tokio::test(start_paused = true)]
    async fn async_retry_exhausts_its_budget_and_surfaces_the_original_error() {
        let mut attempts = 0u32;
        let result = spawn_with_etxtbsy_retry(|| {
            attempts += 1;
            Err::<(), _>(etxtbsy())
        })
        .await;

        let err = result.expect_err("a permanently busy binary must still fail");
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::ExecutableFileBusy,
            "the original error must surface, not a retry-specific one"
        );
        assert_eq!(
            attempts,
            ETXTBSY_MAX_RETRIES + 1,
            "expected 1 initial attempt plus {ETXTBSY_MAX_RETRIES} retries"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn async_retry_never_retries_a_non_etxtbsy_failure() {
        let mut attempts = 0u32;
        let result = spawn_with_etxtbsy_retry(|| {
            attempts += 1;
            Err::<(), _>(std::io::Error::from(std::io::ErrorKind::NotFound))
        })
        .await;

        assert_eq!(
            result.expect_err("a missing binary must fail").kind(),
            std::io::ErrorKind::NotFound
        );
        assert_eq!(attempts, 1, "a non-ETXTBSY failure must not be retried");
    }

    /// The blocking loop is a second code path, so it carries its own bound
    /// and classification pins rather than inheriting the async ones.
    #[test]
    fn blocking_retry_exhausts_the_same_budget_and_surfaces_the_original_error() {
        let mut attempts = 0u32;
        let result = spawn_with_etxtbsy_retry_blocking(|| {
            attempts += 1;
            Err::<(), _>(etxtbsy())
        });

        assert_eq!(
            result
                .expect_err("a permanently busy binary must still fail")
                .kind(),
            std::io::ErrorKind::ExecutableFileBusy
        );
        assert_eq!(
            attempts,
            ETXTBSY_MAX_RETRIES + 1,
            "the blocking loop must share the async loop's bound"
        );
    }

    /// Mirrors `async_retry_absorbs_transient_etxtbsy_and_then_succeeds`:
    /// the blocking loop must also *recover*, not merely bound its failures.
    /// Without this, nothing here would catch a blocking loop that returned
    /// the last error after a run of retries that ended in success.
    #[test]
    fn blocking_retry_absorbs_transient_etxtbsy_and_then_succeeds() {
        let mut attempts = 0u32;
        let result = spawn_with_etxtbsy_retry_blocking(|| {
            attempts += 1;
            if attempts < 3 {
                Err(etxtbsy())
            } else {
                Ok("spawned")
            }
        });

        assert_eq!(result.expect("should succeed after retries"), "spawned");
        assert_eq!(attempts, 3, "expected 2 ETXTBSY attempts plus 1 success");
    }

    #[test]
    fn blocking_retry_never_retries_a_non_etxtbsy_failure() {
        let mut attempts = 0u32;
        let result = spawn_with_etxtbsy_retry_blocking(|| {
            attempts += 1;
            Err::<(), _>(std::io::Error::from(std::io::ErrorKind::NotFound))
        });

        assert_eq!(
            result.expect_err("a missing binary must fail").kind(),
            std::io::ErrorKind::NotFound
        );
        assert_eq!(attempts, 1, "a non-ETXTBSY failure must not be retried");
    }
}
