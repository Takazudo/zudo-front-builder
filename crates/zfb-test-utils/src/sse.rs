//! SSE / live-reload test helpers shared by the `zfb-server` integration
//! tests and the `zfb dev` E2E harness (`crates/zfb/tests/dev_serve_e2e.rs`).
//!
//! Promoted from `crates/zfb-server/tests/integration.rs` (#1018) so both
//! call sites parse the `text/event-stream` wire format and wait out the
//! subscriber-registration race with the same code. The only change made
//! during promotion: `wait_for_subscribers` is generic over the broadcast
//! payload type so this crate needs no dependency on `zfb-server`'s
//! `ReloadEvent` (which would create a dev-dep cycle).

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;

/// Wait until the broadcast channel has at least `min` live receivers,
/// or until `dur` elapses. Returns `true` if the count was reached.
///
/// Uses a `tokio::sync::Notify`-keyed wake rather than a fixed 10 ms
/// busy-wait poll. A helper task subscribes a receiver on the channel
/// and wakes a `Notify` on each message — each send is a signal that
/// more subscribers may have joined. The main loop re-checks
/// `receiver_count` after each wake.
///
/// Because this function subscribes its own private receiver (to hear
/// sends), `receiver_count` inside the task is `external_count + 1`
/// (our own). The threshold is adjusted accordingly: we wait until
/// `receiver_count >= min + 1` inside the task.
///
/// `T` must be `Clone + Send + 'static` (broadcast channel + spawn
/// requirements).
pub async fn wait_for_subscribers<T: Clone + Send + 'static>(
    tx: &tokio::sync::broadcast::Sender<T>,
    min: usize,
    dur: Duration,
) -> bool {
    // Fast path — already satisfied.
    if tx.receiver_count() >= min {
        return true;
    }

    let notify = Arc::new(tokio::sync::Notify::new());
    let notify_w = Arc::clone(&notify);
    let mut rx = tx.subscribe();
    // Our subscribe adds 1 to receiver_count; the external threshold is
    // therefore min + 1 from the task's perspective.
    let threshold = min + 1;

    // Task: wake the notify on each message (= each send from the server).
    // We capture `rx` by move; the spawned future is 'static so T must be Send.
    let watch = tokio::spawn(async move {
        // Continue while messages arrive (Ok) or we lag (still a signal);
        // a Closed channel fails the pattern and ends the loop.
        while let Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) = rx.recv().await
        {
            notify_w.notify_one();
        }
    });

    // The tx clone lets us check receiver_count from within the future
    // without a lifetime dependency on the outer `tx` borrow.
    let tx2 = tx.clone();
    let wait = async move {
        let deadline = tokio::time::sleep(dur);
        tokio::pin!(deadline);
        loop {
            if tx2.receiver_count() >= threshold {
                return true;
            }
            tokio::select! {
                _ = notify.notified() => {}
                _ = &mut deadline => { return tx2.receiver_count() >= threshold; }
            }
        }
    };

    let reached = wait.await;
    watch.abort();
    reached
}

/// Legacy polled variant of `wait_for_subscribers`. Kept for call sites
/// that cannot drive the notify (e.g. where the sender is owned by an
/// external service under test). Prefers 10 ms ticks which is enough for
/// local test flakiness mitigation, though condition-keyed
/// `wait_for_subscribers` is always preferred when available.
pub async fn wait_for_subscribers_polled<T>(
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
///
/// UTF-8 decoding is done **incrementally**: we track how many bytes of
/// `buf` have already been decoded and decoded as valid UTF-8. On each
/// new chunk we decode only the portion that forms complete UTF-8
/// sequences — a multibyte character split across a chunk boundary is
/// left in the undecoded tail until the next chunk completes it. This
/// prevents two correctness problems with the naïve full-buffer
/// `from_utf8`:
///
/// 1. A multibyte character split at a chunk boundary makes
///    `from_utf8(&whole_buf)` return an error, so `unwrap_or("")` would
///    silently discard the entire accumulated buffer including any
///    already-decoded `event:` line (the original defect).
/// 2. Re-scanning the whole buffer on every chunk is O(n²) in the
///    number of bytes received.
///
/// The incremental approach is O(1) per chunk: only the new bytes are
/// decoded, and previously scanned lines are never re-examined.
pub async fn next_sse_event_name(
    resp: reqwest::Response,
    dur: Duration,
) -> anyhow::Result<Option<String>> {
    let mut stream = resp.bytes_stream();
    // Raw byte accumulator for all bytes received so far.
    let mut buf = Vec::<u8>::new();
    // How many bytes from `buf` have already been decoded and scanned.
    // We only decode bytes in `buf[decoded_up_to..]`.
    let mut decoded_up_to: usize = 0;
    // Undecoded text that was part of a previous valid UTF-8 prefix but
    // belongs to an incomplete last line (no newline yet). Carried over
    // so we can re-check once more bytes arrive.
    let mut pending_line = String::new();

    let task = async {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            buf.extend_from_slice(&chunk);

            // Decode only the newly arrived (and any previously
            // undecodable) tail. `from_utf8` on the entire new tail:
            // on error, use `from_utf8_lossy` to advance past the
            // longest valid prefix.
            let tail = &buf[decoded_up_to..];
            let (valid_str, advanced) = match std::str::from_utf8(tail) {
                Ok(s) => (s, tail.len()),
                Err(e) => {
                    // Only the valid prefix is decodable right now;
                    // the remainder might complete with future chunks.
                    let valid_up_to = e.valid_up_to();
                    // Safety: `valid_up_to` is a char boundary by
                    // contract of `Utf8Error`.
                    (
                        std::str::from_utf8(&tail[..valid_up_to])
                            .expect("valid_up_to is a char boundary"),
                        valid_up_to,
                    )
                }
            };
            decoded_up_to += advanced;

            // Combine the leftover partial line from the previous chunk
            // with the freshly decoded string before scanning for lines.
            let to_scan = if pending_line.is_empty() {
                valid_str.to_string()
            } else {
                let mut s = std::mem::take(&mut pending_line);
                s.push_str(valid_str);
                s
            };

            let mut lines = to_scan.split('\n').peekable();
            while let Some(line) = lines.next() {
                if lines.peek().is_none() {
                    // Last segment — may be an incomplete line (no
                    // trailing newline yet). Save it for the next chunk.
                    if !line.is_empty() {
                        pending_line = line.to_string();
                    }
                    break;
                }
                // Complete line (had a '\n' after it).
                let trimmed = line.trim_end_matches('\r');
                if let Some(rest) = trimmed.strip_prefix("event:") {
                    let name = rest.trim().to_string();
                    if !name.is_empty() {
                        return Ok::<Option<String>, anyhow::Error>(Some(name));
                    }
                }
            }
        }
        // EOF: check any remaining pending line.
        if let Some(rest) = pending_line.strip_prefix("event:") {
            let name = rest.trim().to_string();
            if !name.is_empty() {
                return Ok(Some(name));
            }
        }
        Ok(None)
    };

    match tokio::time::timeout(dur, task).await {
        Ok(res) => res,
        Err(_) => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// Internal helpers exposed for unit-testing the incremental decoder.
// ---------------------------------------------------------------------------

/// Scan `buf[decoded_up_to..]` for complete UTF-8 codepoints. Returns
/// `(decoded_str, new_decoded_up_to)`. Only complete codepoints are
/// returned; a split codepoint at the tail is left for the next call.
///
/// Exposed for testing; not part of the public API.
#[doc(hidden)]
pub fn decode_utf8_incremental(buf: &[u8], decoded_up_to: usize) -> (String, usize) {
    let tail = &buf[decoded_up_to..];
    if tail.is_empty() {
        return (String::new(), decoded_up_to);
    }
    let (valid_str, advanced) = match std::str::from_utf8(tail) {
        Ok(s) => (s, tail.len()),
        Err(e) => {
            let valid_up_to = e.valid_up_to();
            (
                std::str::from_utf8(&tail[..valid_up_to]).expect("valid_up_to is a char boundary"),
                valid_up_to,
            )
        }
    };
    (valid_str.to_string(), decoded_up_to + advanced)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    // ── decode_utf8_incremental ──────────────────────────────────────────

    #[test]
    fn incremental_decode_pure_ascii() {
        let buf = b"event: reload\n".to_vec();
        let (s, n) = decode_utf8_incremental(&buf, 0);
        assert_eq!(s, "event: reload\n");
        assert_eq!(n, buf.len());
    }

    #[test]
    fn incremental_decode_multibyte_char_split_at_boundary() {
        // U+2764 ❤ is 3 bytes: 0xE2 0x9D 0xA4.
        // Split: first two bytes in buf so far.
        let mut buf: Vec<u8> = b": comment_".to_vec();
        buf.extend_from_slice(&[0xE2, 0x9D]); // incomplete ❤
        let (s, n) = decode_utf8_incremental(&buf, 0);
        // Only the pure-ASCII prefix is decoded; the two incomplete bytes are left.
        assert_eq!(s, ": comment_");
        assert_eq!(n, b": comment_".len());

        // Extend with the completing byte + ASCII suffix.
        buf.push(0xA4); // completes ❤
        buf.extend_from_slice(b"\nevent: reload\n");
        let (s2, n2) = decode_utf8_incremental(&buf, n);
        // Should decode ❤ + the rest.
        assert!(s2.contains('❤'), "❤ decoded: {s2:?}");
        assert!(s2.contains("event: reload"), "ascii tail: {s2:?}");
        assert_eq!(n2, buf.len());
    }

    #[test]
    fn incremental_decode_advances_up_to_pointer() {
        let buf = b"hello world".to_vec();
        // First call: decode first 5 bytes.
        let (s1, n1) = decode_utf8_incremental(&buf, 0);
        assert_eq!(s1, "hello world");
        assert_eq!(n1, 11);

        // Second call: nothing new to decode.
        let (s2, n2) = decode_utf8_incremental(&buf, n1);
        assert_eq!(s2, "");
        assert_eq!(n2, 11);
    }

    // ── scan_sse_lines — internal logic tested via a helper ──────────────
    //
    // We test the line-scanning logic (the core of next_sse_event_name)
    // directly by simulating the buf+decoded_up_to+pending_line state
    // machine, without needing to build a reqwest::Response.

    /// Simulate the incremental scan loop for a sequence of byte chunks,
    /// returning the first `event:` name found or `None`.
    fn scan_chunks_for_event(chunks: &[&[u8]]) -> Option<String> {
        let mut buf = Vec::<u8>::new();
        let mut decoded_up_to: usize = 0;
        let mut pending_line = String::new();

        for &chunk in chunks {
            buf.extend_from_slice(chunk);

            let (valid_str, new_decoded) = decode_utf8_incremental(&buf, decoded_up_to);
            decoded_up_to = new_decoded;

            let to_scan = if pending_line.is_empty() {
                valid_str.clone()
            } else {
                let mut s = std::mem::take(&mut pending_line);
                s.push_str(&valid_str);
                s
            };

            let mut lines = to_scan.split('\n').peekable();
            while let Some(line) = lines.next() {
                if lines.peek().is_none() {
                    if !line.is_empty() {
                        pending_line = line.to_string();
                    }
                    break;
                }
                let trimmed = line.trim_end_matches('\r');
                if let Some(rest) = trimmed.strip_prefix("event:") {
                    let name = rest.trim().to_string();
                    if !name.is_empty() {
                        return Some(name);
                    }
                }
            }
        }
        // EOF: check pending line.
        if let Some(rest) = pending_line.strip_prefix("event:") {
            let name = rest.trim().to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
        None
    }

    #[test]
    fn sse_scan_single_chunk_finds_event() {
        let result = scan_chunks_for_event(&[b"event: reload\ndata: {}\n\n"]);
        assert_eq!(result.as_deref(), Some("reload"));
    }

    #[test]
    fn sse_scan_event_split_across_chunks() {
        // "event: reload\n" split after the colon.
        let result = scan_chunks_for_event(&[b"event:", b" reload\ndata: {}\n\n"]);
        assert_eq!(result.as_deref(), Some("reload"));
    }

    #[test]
    fn sse_scan_multibyte_char_split_does_not_discard_event() {
        // U+2764 ❤ (3 bytes) embedded in a comment line, split across chunks.
        // The event line that follows must still be found.
        let heart = [0xE2u8, 0x9D, 0xA4]; // ❤ in UTF-8
        let mut chunk1: Vec<u8> = b": comment_".to_vec();
        chunk1.extend_from_slice(&heart[..2]); // incomplete ❤
        let mut chunk2: Vec<u8> = heart[2..].to_vec(); // completes ❤
        chunk2.extend_from_slice(b"\nevent: reload\ndata: {}\n\n");

        let result = scan_chunks_for_event(&[&chunk1, &chunk2]);
        assert_eq!(
            result.as_deref(),
            Some("reload"),
            "event name must be found even with split multibyte char"
        );
    }

    #[test]
    fn sse_scan_no_event_returns_none() {
        let result = scan_chunks_for_event(&[b"data: hello\n\n"]);
        assert_eq!(result, None);
    }

    // ── wait_for_subscribers ─────────────────────────────────────────────

    #[test]
    fn wait_for_subscribers_returns_true_when_already_met() {
        let (tx, _rx1) = tokio::sync::broadcast::channel::<String>(8);
        // One receiver already registered.
        let reached = rt().block_on(wait_for_subscribers(&tx, 1, Duration::from_millis(200)));
        assert!(reached, "count already met before awaiting");
    }

    #[test]
    fn wait_for_subscribers_returns_false_on_timeout() {
        let (tx, _rx) = tokio::sync::broadcast::channel::<String>(8);
        // Keep _rx alive so receiver_count = 1; require 2 — never met.
        // Our function subscribes +1 (threshold=3), so we need 3 total; only 2 exist.
        let reached = rt().block_on(wait_for_subscribers(&tx, 2, Duration::from_millis(50)));
        assert!(!reached, "count 2 never met with only 1 external receiver");
    }

    #[test]
    fn wait_for_subscribers_detects_new_receiver_via_send() {
        // Spawn a task that subscribes a second receiver after a small delay,
        // then sends a message. `wait_for_subscribers` should wake up and
        // return true.
        //
        // We use a second Notify to keep _rx2 alive until wait_for_subscribers
        // has finished its check — otherwise the spawned task's async block
        // would complete (dropping _rx2) before the count check runs.
        rt().block_on(async {
            let (tx, _rx1) = tokio::sync::broadcast::channel::<String>(8);
            let tx_clone = tx.clone();
            // Notify used to signal the spawned task to release _rx2.
            let done = Arc::new(tokio::sync::Notify::new());
            let done_w = Arc::clone(&done);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                let _rx2 = tx_clone.subscribe();
                // Send a message so our wait wakes up.
                let _ = tx_clone.send("ping".to_string());
                // Hold _rx2 alive until the caller signals completion.
                done_w.notified().await;
                drop(_rx2);
            });
            let reached = wait_for_subscribers(&tx, 2, Duration::from_millis(500)).await;
            // Signal the spawned task to release its resources.
            done.notify_one();
            assert!(reached, "second receiver registered within deadline");
        });
    }
}
