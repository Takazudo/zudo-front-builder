//! Combined Gap 1 + Gap 2 end-to-end integration test (sub-task 2.3 / #369).
//!
//! Verifies that:
//!
//! - Gap 1 (request-time SSR): a `prerender = false` route reads content
//!   from disk at every request so successive curls to the same URL can
//!   return different bodies after an on-disk edit.
//!
//! - Gap 2 (extra_watch_paths): the watcher registered on the same extra
//!   directory fires a debounced change event when the file is edited —
//!   the change proves the watcher infrastructure is wired up, not that
//!   the server drives a rebuild (the server under test does not own the
//!   rebuild loop).
//!
//! Both gaps must be exercised in the same test so the interaction is
//! verified: the extra directory is a temp path outside the project root,
//! and the SSR dispatcher reads a file from that same temp path. If
//! either seam were broken the assertions below would fail.
//!
//! Uses the same ephemeral-port + in-process axum pattern as the other
//! integration tests in this directory. No background process is held
//! open; the server task is aborted at the end of the test.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use tempfile::tempdir;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::time::timeout;
use zfb_server::livereload::ReloadEvent;
use zfb_server::{
    serve_with_listener, PageCache, ServeOpts, SsrDispatchError, SsrDispatcher, SsrRequest,
    SsrResponse, SsrRouteRecord, SsrRouteSet, SsrRoutesHandle,
};
use zfb_watcher::Watcher;

/// SSR dispatcher that reads `<base_dir>/example.md` from disk on every
/// dispatch call. This is intentionally NOT cached so each request sees
/// the current on-disk state, proving request-time render semantics
/// (Gap 1).
struct FileSsrDispatcher {
    /// Absolute path to the directory that contains `example.md`.
    base_dir: PathBuf,
}

#[async_trait]
impl SsrDispatcher for FileSsrDispatcher {
    async fn dispatch(&self, _request: SsrRequest) -> Result<SsrResponse, SsrDispatchError> {
        let path = self.base_dir.join("example.md");
        let content = tokio::fs::read_to_string(&path)
            .await
            .unwrap_or_else(|e| format!("read error: {e}"));

        let headers = vec![("content-type".into(), "text/html; charset=utf-8".into())];

        Ok(SsrResponse {
            status: 200,
            headers,
            // Wrap in minimal HTML so the livereload injection logic is
            // exercised the same way a real page would be.
            body: format!("<html><body>{content}</body></html>").into_bytes(),
        })
    }
}

#[tokio::test]
async fn gap1_ssr_and_gap2_watcher_work_together() {
    // ----------------------------------------------------------------
    // 1. Set up temp directories
    // ----------------------------------------------------------------
    let project = tempdir().expect("project tempdir");
    let extra = tempdir().expect("extra tempdir");

    // Canonicalise so notify's reported paths and our assertions agree
    // on the exact form (macOS resolves /var/... → /private/var/...).
    let extra_canonical = extra.path().canonicalize().expect("canonicalize extra");

    // Write the initial file content that the SSR dispatcher will read.
    std::fs::write(extra_canonical.join("example.md"), "v1 content\n")
        .expect("write initial example.md");

    // ----------------------------------------------------------------
    // 2. Start the watcher with the extra path (Gap 2 seam)
    // ----------------------------------------------------------------
    let (_watcher, mut watch_rx) = Watcher::start_with_extras(
        project.path(),
        std::iter::empty::<&str>(),
        std::iter::once(extra_canonical.as_path()),
        // Short debounce so the test doesn't wait long for the event.
        Duration::from_millis(50),
    )
    .expect("start watcher with extras");

    // Prove the watcher stream is live before relying on it: a fixed
    // warmup sleep races the FSEvents startup dead window under load.
    // Write fresh-named marker files under the watched extra dir until one
    // is delivered on the channel (#1338 shared helper). The leftover
    // warmup events are drained just before the real edit below.
    let handshake = zfb_test_utils::watcher_live_handshake(
        zfb_test_utils::HandshakeOpts::new(Duration::from_secs(10)),
        |idx| {
            std::fs::write(
                extra_canonical.join(format!("__warmup_{idx}.md")),
                b"warmup\n",
            )
            .expect("write warmup file");
        },
        || watch_rx.try_recv().is_ok(),
    )
    .await;
    assert!(
        handshake.live,
        "watcher never delivered a warmup event within {:?} ({} markers) — \
         the extra-path watch never started delivering events",
        handshake.elapsed, handshake.markers_written,
    );

    // ----------------------------------------------------------------
    // 3. Boot the server with an SSR route that reads from extra_canonical
    // ----------------------------------------------------------------
    let dist_root = project.path().join("dist");
    let public_root = project.path().join("public");
    std::fs::create_dir_all(dist_root.join("assets")).expect("create dist/assets");
    std::fs::create_dir_all(&public_root).expect("create public");

    let dispatcher = Arc::new(FileSsrDispatcher {
        base_dir: extra_canonical.clone(),
    });
    let ssr_set = SsrRouteSet::new(
        vec![SsrRouteRecord {
            pattern: "/page".into(),
        }],
        dispatcher as Arc<dyn SsrDispatcher>,
    );

    let pages = PageCache::new();
    let (tx, _rx) = broadcast::channel::<ReloadEvent>(8);
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");

    let ssr_handle: SsrRoutesHandle = Arc::new(RwLock::new(Some(ssr_set)));
    let opts = ServeOpts {
        project_root: project.path().to_path_buf(),
        dist_root: dist_root.clone(),
        dev_assets_root: None,
        html_root: dist_root,
        public_root,
        addr,
        pages,
        broadcast: tx,
        plugins: None,
        injected_routes: None,
        ssr_routes: Some(ssr_handle),
        base: None,
        trailing_slash: false,
        mode: zfb_server::ServerMode::Dev,
        islands_bundle_url: None,
        css_bundle_url: None,
        allowed_hosts: Vec::new(),
        bound_host: None,
        render_on_request_hook: None,
    };
    let server = tokio::spawn(async move {
        serve_with_listener(opts, listener, std::future::pending::<()>()).await
    });
    tokio::task::yield_now().await;

    let client = reqwest::Client::builder().build().expect("build client");

    // ----------------------------------------------------------------
    // 4. First curl: assert body contains "v1 content" (Gap 1 proof)
    // ----------------------------------------------------------------
    let resp = client
        .get(format!("http://{addr}/page"))
        .send()
        .await
        .expect("GET /page v1");
    assert_eq!(resp.status().as_u16(), 200, "expected 200 for /page");
    let body = resp.text().await.expect("read body v1");
    assert!(
        body.contains("v1 content"),
        "expected 'v1 content' in body; got: {body}"
    );

    // ----------------------------------------------------------------
    // 5. Edit the file
    // ----------------------------------------------------------------
    // Drain any warmup events still queued from the liveness handshake so
    // the assertion below observes the real example.md edit, not a warmup.
    while watch_rx.try_recv().is_ok() {}
    std::fs::write(extra_canonical.join("example.md"), "v2 content\n")
        .expect("write updated example.md");

    // ----------------------------------------------------------------
    // 6. Assert watcher fires a change for the extra path (Gap 2 proof)
    // ----------------------------------------------------------------
    let change = timeout(Duration::from_secs(2), watch_rx.recv())
        .await
        .expect("watcher change arrives within 2s")
        .expect("watcher channel open");
    assert!(
        change.path.starts_with(&extra_canonical),
        "expected change path {:?} to be under extra root {:?}",
        change.path,
        extra_canonical
    );

    // ----------------------------------------------------------------
    // 7. Second curl: assert body now contains "v2 content" (Gap 1
    //    request-time-render proof — SSR always reads from disk fresh)
    // ----------------------------------------------------------------
    let resp2 = client
        .get(format!("http://{addr}/page"))
        .send()
        .await
        .expect("GET /page v2");
    assert_eq!(
        resp2.status().as_u16(),
        200,
        "expected 200 for /page after edit"
    );
    let body2 = resp2.text().await.expect("read body v2");
    assert!(
        body2.contains("v2 content"),
        "expected 'v2 content' in body after edit; got: {body2}"
    );

    // ----------------------------------------------------------------
    // 8. Teardown
    // ----------------------------------------------------------------
    server.abort();
}
