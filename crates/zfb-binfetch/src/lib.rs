use std::fs::File;
use std::path::Path;
use std::time::Duration;

/// Options controlling fetch behaviour.
pub struct FetchOpts {
    /// Number of attempts before giving up. Default: 3.
    pub attempts: u32,
    /// TCP connect timeout per attempt. Default: 15 s.
    pub connect_timeout: Duration,
    /// Overall request timeout (used as a fallback cap because reqwest 0.12's
    /// blocking client does not expose a per-read idle timeout). Kept generous
    /// — the body is a ~75 MB download that must not be aborted on a slow link;
    /// this is a stuck-transfer backstop, not a tight per-request limit.
    /// Default: 300 s.
    pub read_timeout: Duration,
    /// Backoff before the second attempt; doubled each retry. Default: 500 ms.
    pub initial_backoff: Duration,
}

impl Default for FetchOpts {
    fn default() -> Self {
        Self {
            attempts: 3,
            connect_timeout: Duration::from_secs(15),
            read_timeout: Duration::from_secs(300),
            initial_backoff: Duration::from_millis(500),
        }
    }
}

/// Stream `url` to `dest` (a caller-owned temp path), retrying on transient errors.
///
/// On `Ok`, the full body has been written to `dest`. On `Err`, no partial file
/// remains at `dest`. The caller is responsible for hashing, chmod, and renaming
/// into the final slot, and **MUST** ensure `dest`'s parent directory exists.
pub fn fetch_to_file(url: &str, dest: &Path, opts: &FetchOpts) -> Result<(), String> {
    let client = reqwest::blocking::ClientBuilder::new()
        .use_rustls_tls()
        .connect_timeout(opts.connect_timeout)
        // reqwest 0.12 blocking::ClientBuilder does not expose read_timeout, so
        // we use the overall `.timeout()` as a fallback cap. This is a generous
        // deadline (default 300 s) rather than a tight per-request limit, so it
        // won't abort a legitimately slow ~75 MB download within that window.
        // If a future reqwest version adds blocking read_timeout, swap this for
        // `.read_timeout(opts.read_timeout)` and remove the overall cap.
        .timeout(opts.read_timeout)
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;

    let mut backoff = opts.initial_backoff;
    let mut last_err = String::new();

    for attempt in 1..=opts.attempts {
        // Fresh GET each attempt — never reuse a response across retries.
        let response = match client.get(url).send() {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("attempt {attempt}/{}: send error: {e}", opts.attempts);
                if attempt < opts.attempts {
                    std::thread::sleep(backoff);
                    backoff *= 2;
                }
                continue;
            }
        };

        let status = response.status();
        if !status.is_success() {
            last_err = format!("attempt {attempt}/{}: HTTP {status}", opts.attempts);
            if attempt < opts.attempts {
                std::thread::sleep(backoff);
                backoff *= 2;
            }
            continue;
        }

        // Per-attempt File::create truncates any partial write from the previous
        // attempt so stale bytes never bleed into the next try.
        let mut file = match File::create(dest) {
            Ok(f) => f,
            Err(e) => {
                let _ = std::fs::remove_file(dest);
                return Err(format!("cannot create dest file: {e}"));
            }
        };

        let mut response = response;
        match response.copy_to(&mut file) {
            Ok(_) => return Ok(()),
            Err(e) => {
                last_err = format!("attempt {attempt}/{}: body error: {e}", opts.attempts);
            }
        }

        if attempt < opts.attempts {
            std::thread::sleep(backoff);
            backoff *= 2;
        }
    }

    // All attempts failed — remove any partial dest file before returning.
    let _ = std::fs::remove_file(dest);
    let reason = if last_err.is_empty() {
        // attempts == 0: the loop never ran.
        format!("no attempts made (opts.attempts = {})", opts.attempts)
    } else {
        last_err
    };
    Err(format!(
        "{reason}. \
         Tip: set ZFB_ESBUILD_BIN or ZFB_TAILWIND_BIN to an existing binary \
         path to skip the download entirely."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    // ── server helpers ───────────────────────────────────────────────────────

    /// Drain HTTP request headers (read until `\r\n\r\n`).
    fn drain_request_headers(stream: &mut TcpStream) {
        stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .ok();
        let mut buf = Vec::with_capacity(512);
        let mut byte = [0u8; 1];
        loop {
            match stream.read(&mut byte) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    buf.push(byte[0]);
                    if buf.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
            }
        }
    }

    /// Write a complete HTTP/1.1 200 OK response with the given body.
    fn serve_full(stream: &mut TcpStream, body: &[u8]) {
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.write_all(body);
    }

    /// Write a truncated response: claims `claimed_len` bytes in Content-Length
    /// but only sends `partial_body`, then closes — forcing a body decode error.
    fn serve_truncated(stream: &mut TcpStream, partial_body: &[u8], claimed_len: usize) {
        let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {claimed_len}\r\n\r\n");
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.write_all(partial_body);
        // Socket closes here; reqwest detects the body is shorter than Content-Length.
    }

    type Handler = Box<dyn FnOnce(&mut TcpStream) + Send + 'static>;

    /// Spawn a background HTTP/1.1 server that serves each incoming connection
    /// with the next handler from the queue, then idles (blocks on `accept`).
    /// Returns the URL (e.g. `http://127.0.0.1:PORT/`).
    fn spawn_server(handlers: Vec<Handler>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let queue: Arc<Mutex<VecDeque<Handler>>> = Arc::new(Mutex::new(VecDeque::from(handlers)));

        std::thread::spawn(move || loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    drain_request_headers(&mut stream);
                    let handler = queue.lock().unwrap().pop_front();
                    if let Some(h) = handler {
                        h(&mut stream);
                    }
                    // If no handler remains, close the connection silently.
                }
                Err(_) => break,
            }
        });

        format!("http://{addr}/")
    }

    /// Tiny opts for tests: small backoff so suites finish quickly.
    fn tiny_opts(attempts: u32) -> FetchOpts {
        FetchOpts {
            attempts,
            connect_timeout: Duration::from_secs(5),
            read_timeout: Duration::from_secs(5),
            initial_backoff: Duration::from_millis(2),
        }
    }

    // ── tests ────────────────────────────────────────────────────────────────

    /// Attempt 1 truncates; attempt 2 serves the full body.
    /// Assert: Ok, file == full body, counter == 2.
    #[test]
    fn truncate_then_succeed() {
        const FULL_BODY: &[u8] = b"the complete response body for the test";

        let counter = Arc::new(AtomicUsize::new(0));
        let c1 = Arc::clone(&counter);
        let c2 = Arc::clone(&counter);

        let handlers: Vec<Handler> = vec![
            Box::new(move |s: &mut TcpStream| {
                c1.fetch_add(1, Ordering::SeqCst);
                // claim 200 bytes but only send 9 — forces a body error
                serve_truncated(s, b"the compl", FULL_BODY.len() + 200);
            }),
            Box::new(move |s: &mut TcpStream| {
                c2.fetch_add(1, Ordering::SeqCst);
                serve_full(s, FULL_BODY);
            }),
        ];

        let url = spawn_server(handlers);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");

        let result = fetch_to_file(&url, &dest, &tiny_opts(3));
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert_eq!(std::fs::read(&dest).unwrap(), FULL_BODY);
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    /// All attempts truncate → Err after exactly `attempts` tries.
    /// Assert: Err, dest file absent, counter == attempts.
    #[test]
    fn all_attempts_fail() {
        let attempts: u32 = 3;
        let counter = Arc::new(AtomicUsize::new(0));

        let handlers: Vec<Handler> = (0..attempts)
            .map(|_| {
                let c = Arc::clone(&counter);
                let h: Handler = Box::new(move |s: &mut TcpStream| {
                    c.fetch_add(1, Ordering::SeqCst);
                    serve_truncated(s, b"part", 9_999);
                });
                h
            })
            .collect();

        let url = spawn_server(handlers);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");

        let result = fetch_to_file(&url, &dest, &tiny_opts(attempts));
        assert!(result.is_err(), "expected Err, got Ok");
        assert!(!dest.exists(), "dest must not exist after all-fail");
        assert_eq!(counter.load(Ordering::SeqCst), attempts as usize);
    }

    /// Single clean response → Ok, file matches.
    #[test]
    fn happy_path() {
        const BODY: &[u8] = b"happy response body";
        let counter = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&counter);

        let handlers: Vec<Handler> = vec![Box::new(move |s: &mut TcpStream| {
            c.fetch_add(1, Ordering::SeqCst);
            serve_full(s, BODY);
        })];

        let url = spawn_server(handlers);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");

        let result = fetch_to_file(&url, &dest, &tiny_opts(3));
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert_eq!(std::fs::read(&dest).unwrap(), BODY);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    /// Attempt 1 writes distinct garbage bytes then truncates.  Attempt 2 serves
    /// the real body.  The final file must equal the real body and must NOT
    /// contain the garbage, proving per-attempt `File::create` truncated it.
    #[test]
    fn truncate_replaced_between_attempts() {
        const GARBAGE: &[u8] = b"GARBAGE";
        const REAL_BODY: &[u8] = b"CORRECT_RESPONSE_BODY";

        let handlers: Vec<Handler> = vec![
            Box::new(|s: &mut TcpStream| {
                // Claim a large body; only write distinctive garbage.
                serve_truncated(s, GARBAGE, 9_999);
            }),
            Box::new(|s: &mut TcpStream| {
                serve_full(s, REAL_BODY);
            }),
        ];

        let url = spawn_server(handlers);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");

        let result = fetch_to_file(&url, &dest, &tiny_opts(3));
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        let got = std::fs::read(&dest).unwrap();
        assert_eq!(got, REAL_BODY, "final file must match real body");
        // Belt-and-suspenders: garbage from attempt 1 must not appear anywhere.
        assert!(
            !got.windows(GARBAGE.len()).any(|w| w == GARBAGE),
            "garbage bytes from attempt 1 must not appear in the final file"
        );
    }

    /// Clean body with `Connection: close` and no `Content-Length` → Ok, file matches.
    #[test]
    fn no_content_length_body() {
        const BODY: &[u8] = b"no content length response body";

        let handlers: Vec<Handler> = vec![Box::new(|s: &mut TcpStream| {
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n");
            let _ = s.write_all(BODY);
        })];

        let url = spawn_server(handlers);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");

        let result = fetch_to_file(&url, &dest, &tiny_opts(3));
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert_eq!(std::fs::read(&dest).unwrap(), BODY);
    }
}
