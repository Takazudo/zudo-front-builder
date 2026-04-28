//! End-to-end integration tests for `zfb-server`.
//!
//! These spin up the real axum server on an ephemeral 127.0.0.1 port
//! (so they never collide across worktrees or with a developer's
//! running `zfb dev`) and exercise it via a real HTTP client. The page
//! cache and the live-reload broadcast channel are populated directly
//! from the test (the rebuild orchestrator that normally fills them in
//! the bin crate is out of scope for Sub 6).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use futures_util::StreamExt;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::time::timeout;
use zfb_build::BuildOutcome;
use zfb_graph::PageId;
use zfb_server::livereload::{outcome_to_events, ReloadEvent};
use zfb_server::{serve_with_listener, PageCache, ServeOpts};

/// All the per-test handles we need: the bound address, the page
/// cache (so the test can populate it), the broadcast sender (so the
/// test can fire reload events), the join handle of the server task,
/// and a guard for the temp project directory.
struct Harness {
    addr: SocketAddr,
    pages: PageCache,
    tx: broadcast::Sender<ReloadEvent>,
    server: tokio::task::JoinHandle<anyhow::Result<()>>,
    _tmp: TempDir,
}

impl Harness {
    /// Construct `<root>/dist/assets/`, `<root>/public/`, bind an
    /// ephemeral port, and spawn the server.
    async fn start() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root: PathBuf = tmp.path().to_path_buf();
        let dist_root = root.join("dist");
        let public_root = root.join("public");
        std::fs::create_dir_all(dist_root.join("assets")).unwrap();
        std::fs::create_dir_all(&public_root).unwrap();

        // Pre-populate fixture assets used by the asset/public tests.
        std::fs::write(
            dist_root.join("assets").join("main.css"),
            "body { color: red; }",
        )
        .unwrap();
        std::fs::write(public_root.join("favicon.ico"), b"\x00FAVICON\x00").unwrap();

        let pages = PageCache::new();
        let (tx, _rx) = broadcast::channel::<ReloadEvent>(64);

        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .expect("bind ephemeral");
        let addr = listener.local_addr().expect("local_addr");

        let opts = ServeOpts {
            project_root: root.clone(),
            dist_root,
            public_root,
            addr,
            pages: pages.clone(),
            broadcast: tx.clone(),
        };

        let server = tokio::spawn(async move {
            serve_with_listener(opts, listener, std::future::pending::<()>()).await
        });

        // Tiny readiness wait: serve_with_listener has the listener
        // already bound, so the OS will queue connections immediately.
        // No sleep needed — reqwest will retry under the hood once axum
        // accepts. But on slow CI, give the spawn a chance to run.
        tokio::task::yield_now().await;

        Self {
            addr,
            pages,
            tx,
            server,
            _tmp: tmp,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        // Best-effort: kill the server task so the test process can
        // exit cleanly. axum::serve has no graceful shutdown wired up
        // in this crate, so an abort is the simplest and most reliable
        // option here.
        self.server.abort();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn page_route_serves_html_from_cache() {
    let h = Harness::start().await;
    h.pages
        .insert("/foo", "<html><body><h1>foo page</h1></body></html>")
        .await;

    let resp = reqwest::get(h.url("/foo")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("<h1>foo page</h1>"), "body: {body}");
    assert!(
        body.contains("<script src=\"/__zfb/livereload.js\"></script>"),
        "expected injected livereload tag, body: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn asset_route_serves_static_files() {
    let h = Harness::start().await;
    let resp = reqwest::get(h.url("/assets/main.css")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(
        ct.to_ascii_lowercase().contains("css"),
        "unexpected content-type: {ct}"
    );
    let body = resp.text().await.unwrap();
    assert_eq!(body, "body { color: red; }");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_route_serves_user_files() {
    let h = Harness::start().await;
    let resp = reqwest::get(h.url("/public/favicon.ico")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.bytes().await.unwrap();
    assert_eq!(body.as_ref(), b"\x00FAVICON\x00");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn livereload_js_endpoint_returns_script() {
    let h = Harness::start().await;
    let resp = reqwest::get(h.url("/__zfb/livereload.js")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(
        ct.starts_with("application/javascript"),
        "unexpected content-type: {ct}"
    );
    let body = resp.text().await.unwrap();
    assert!(body.contains("EventSource"), "body missing EventSource");
    assert!(body.contains("/__zfb/reload"), "body missing /__zfb/reload");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_path_returns_404() {
    let h = Harness::start().await;
    let resp = reqwest::get(h.url("/does-not-exist")).await.unwrap();
    assert_eq!(resp.status(), 404);
    let body = resp.text().await.unwrap();
    assert!(body.contains("404"), "body: {body}");
    assert!(
        body.contains("page not in cache"),
        "expected dev-mode 404 body, got: {body}"
    );
}

/// Wait until the broadcast channel has at least `min` live receivers,
/// or until `dur` elapses. Returns `true` if the count was reached.
///
/// This eliminates the timing race between "the SSE handler hooked up
/// its subscriber" and "the test fires a broadcast event" — without
/// relying on a fixed `sleep` that's flaky on slow CI.
async fn wait_for_subscribers(
    tx: &broadcast::Sender<ReloadEvent>,
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
async fn next_sse_event_name(
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

    match timeout(dur, task).await {
        Ok(res) => res,
        Err(_) => Ok(None),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sse_emits_page_event_on_rebuild() {
    let h = Harness::start().await;

    // Open the SSE stream first so we don't miss the broadcast.
    let resp = reqwest::Client::new()
        .get(h.url("/__zfb/reload"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Wait for the SSE handler's subscriber to actually register on the
    // broadcast channel. Without this, a `tx.send` racing the handler
    // would be silently dropped (broadcast drops to zero receivers).
    assert!(
        wait_for_subscribers(&h.tx, 1, Duration::from_secs(5)).await,
        "SSE handler never subscribed to broadcast"
    );

    let outcome = BuildOutcome {
        pages_written: vec![PageId::new("/index.html")],
        ..Default::default()
    };
    for ev in outcome_to_events(&outcome) {
        let _ = h.tx.send(ev);
    }

    let name = next_sse_event_name(resp, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(name.as_deref(), Some("page"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sse_emits_css_event_on_css_change() {
    let h = Harness::start().await;

    let resp = reqwest::Client::new()
        .get(h.url("/__zfb/reload"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(
        wait_for_subscribers(&h.tx, 1, Duration::from_secs(5)).await,
        "SSE handler never subscribed to broadcast"
    );

    let outcome = BuildOutcome {
        css_rerun: true,
        css_changed: true,
        ..Default::default()
    };
    for ev in outcome_to_events(&outcome) {
        let _ = h.tx.send(ev);
    }

    let name = next_sse_event_name(resp, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(name.as_deref(), Some("css"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sse_does_not_emit_page_when_only_css_changed() {
    let h = Harness::start().await;

    let resp = reqwest::Client::new()
        .get(h.url("/__zfb/reload"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(
        wait_for_subscribers(&h.tx, 1, Duration::from_secs(5)).await,
        "SSE handler never subscribed to broadcast"
    );

    let outcome = BuildOutcome {
        css_rerun: true,
        css_changed: true,
        ..Default::default()
    };
    for ev in outcome_to_events(&outcome) {
        let _ = h.tx.send(ev);
    }

    // Read the stream for a short window and assert we never see a
    // `page` event. We may or may not see `css` — that's covered by
    // the previous test — what matters here is the absence of `page`.
    let mut stream = resp.bytes_stream();
    let mut buf = Vec::<u8>::new();
    let watch = async {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.unwrap();
            buf.extend_from_slice(&chunk);
            let s = std::str::from_utf8(&buf).unwrap_or("");
            for line in s.lines() {
                if let Some(rest) = line.strip_prefix("event:") {
                    assert_ne!(
                        rest.trim(),
                        "page",
                        "unexpected `event: page` line in stream:\n{s}"
                    );
                }
            }
        }
    };
    // 500 ms is plenty: outcome_to_events runs synchronously and the
    // broadcast hop is sub-millisecond. If `page` were going to appear,
    // it would have appeared by now.
    let _ = timeout(Duration::from_millis(500), watch).await;
}
