//! SSE / live-reload test helpers shared by the `zfb-server` integration
//! tests and the `zfb dev` E2E harness (`crates/zfb/tests/dev_serve_e2e.rs`).
//!
//! Promoted from `crates/zfb-server/tests/integration.rs` (#1018) so both
//! call sites parse the `text/event-stream` wire format and wait out the
//! subscriber-registration race with the same code. The only change made
//! during promotion: `wait_for_subscribers` is generic over the broadcast
//! payload type so this crate needs no dependency on `zfb-server`'s
//! `ReloadEvent` (which would create a dev-dep cycle).

use std::time::Duration;

use futures_util::StreamExt;

/// Wait until the broadcast channel has at least `min` live receivers,
/// or until `dur` elapses. Returns `true` if the count was reached.
///
/// This eliminates the timing race between "the SSE handler hooked up
/// its subscriber" and "the test fires a broadcast event" — without
/// relying on a fixed `sleep` that's flaky on slow CI.
pub async fn wait_for_subscribers<T>(
    tx: &tokio::sync::broadcast::Sender<T>,
    min: usize,
    dur: Duration,
) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < dur {
        if tx.receiver_count() >= min {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tx.receiver_count() >= min
}

/// Read a `text/event-stream` body chunk-by-chunk and return the first
/// `event:` line we observe. Times out after `dur` so a missing event
/// fails the test fast rather than hanging the suite.
pub async fn next_sse_event_name(
    resp: reqwest::Response,
    dur: Duration,
) -> anyhow::Result<Option<String>> {
    let mut stream = resp.bytes_stream();
    let mut buf = Vec::<u8>::new();

    let task = async {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            buf.extend_from_slice(&chunk);
            // Look for any `event: <name>\n` line in what we have so far.
            let s = std::str::from_utf8(&buf).unwrap_or("");
            for line in s.lines() {
                if let Some(rest) = line.strip_prefix("event:") {
                    let name = rest.trim().to_string();
                    if !name.is_empty() {
                        return Ok::<Option<String>, anyhow::Error>(Some(name));
                    }
                }
            }
        }
        Ok(None)
    };

    match tokio::time::timeout(dur, task).await {
        Ok(res) => res,
        Err(_) => Ok(None),
    }
}
