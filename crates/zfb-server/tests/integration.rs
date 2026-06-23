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
use zfb_test_utils::{next_sse_event_name, wait_for_subscribers};

/// All the per-test handles we need: the bound address, the page
/// cache (so the test can populate it), the broadcast sender (so the
/// test can fire reload events), the join handle of the server task,
/// and a guard for the temp project directory.
struct Harness {
    addr: SocketAddr,
    pages: PageCache,
    tx: broadcast::Sender<ReloadEvent>,
    server: tokio::task::JoinHandle<anyhow::Result<()>>,
    /// Project public-static directory inside the tempdir — exposed so
    /// tests can stage fixture files at runtime (e.g. for precedence
    /// and path-safety checks). `_tmp` keeps the directory alive.
    public_root: PathBuf,
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
            dist_root: dist_root.clone(),
            dev_assets_root: None,
            html_root: dist_root,
            public_root: public_root.clone(),
            addr,
            pages: pages.clone(),
            broadcast: tx.clone(),
            plugins: None,
            injected_routes: None,
            ssr_routes: None,
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
            public_root,
            _tmp: tmp,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    fn public_root(&self) -> &std::path::Path {
        &self.public_root
    }

    fn tmp_path(&self) -> &std::path::Path {
        self._tmp.path()
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

/// `public/favicon.ico` must be reachable at the site root (`/favicon.ico`),
/// NOT under a `/public/` URL prefix. This mirrors the production build
/// (`copy_public_dir` copies straight into `dist/`) so a single
/// `<link rel="icon" href="/favicon.ico">` works in both dev and prod.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_files_serve_at_site_root() {
    let h = Harness::start().await;
    let resp = reqwest::get(h.url("/favicon.ico")).await.unwrap();
    assert_eq!(
        resp.status(),
        200,
        "public file should be reachable at root"
    );
    let body = resp.bytes().await.unwrap();
    assert_eq!(body.as_ref(), b"\x00FAVICON\x00");
}

/// Files in `public/` should NOT be reachable under a `/public/...` URL
/// prefix — that was the pre-fix dev URL space, and it doesn't exist in
/// production (where `public/*` is copied to `dist/` root). Keeping the
/// old URL alive would let developers ship code that works in dev but
/// 404s in prod.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_files_not_reachable_under_public_prefix() {
    let h = Harness::start().await;
    let resp = reqwest::get(h.url("/public/favicon.ico")).await.unwrap();
    assert_eq!(
        resp.status(),
        404,
        "the legacy /public/* mount must be gone — files appear at site root"
    );
}

/// When a `pages/foo.tsx` route and a `public/foo` file collide, the
/// rendered page wins. This is a precedence guarantee: the public-disk
/// fallback runs AFTER the page cache, so the page handler always sees
/// the rendered HTML first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn page_cache_wins_over_public_file() {
    let h = Harness::start().await;
    // Stage a "page" at /robots.txt in the cache while a same-named
    // file exists in public/ (we add one via the harness's tempdir).
    std::fs::write(h.public_root().join("robots.txt"), b"FROM PUBLIC").unwrap();
    h.pages.insert("/robots.txt", "FROM PAGE CACHE").await;

    let resp = reqwest::get(h.url("/robots.txt")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("FROM PAGE CACHE"),
        "page cache must win over public/ file, got: {body}"
    );
}

/// When a file exists in both `dist/` and `public/`, the dist copy
/// wins. The on-disk fallback chain in `serve_page` consults
/// `<dist_root>/<path>` before `<public_root>/<path>`, so any artefact
/// the build pipeline materialised into `dist/` (rendered HTML, hashed
/// assets, etc.) shadows a same-named user file in `public/`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dist_wins_over_public_file() {
    let h = Harness::start().await;
    // The dist_root for the harness is `<tmp>/dist`. Drop a file in
    // there that collides with a public/ file of the same name.
    let dist_root = h.tmp_path().join("dist");
    std::fs::write(dist_root.join("collide.txt"), b"FROM DIST").unwrap();
    std::fs::write(h.public_root().join("collide.txt"), b"FROM PUBLIC").unwrap();

    let resp = reqwest::get(h.url("/collide.txt")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert_eq!(
        body, "FROM DIST",
        "dist/ must win over public/ when both have the same path"
    );
}

/// Requesting a subdirectory of `public/` (e.g. `/img/`) must NOT
/// produce a directory listing — `read_from_public` rejects directory
/// reads explicitly. Returns the dev 404.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_directory_request_does_not_list() {
    let h = Harness::start().await;
    std::fs::create_dir_all(h.public_root().join("img")).unwrap();
    std::fs::write(h.public_root().join("img/inside.png"), b"PNG").unwrap();

    // `/img/inside.png` works — it's a real file.
    let ok = reqwest::get(h.url("/img/inside.png")).await.unwrap();
    assert_eq!(ok.status(), 200);

    // `/img` as a directory must 404 — neither the page cache nor the
    // public fallback should serve it. (read_from_public explicitly
    // rejects directory reads; read_from_dist would also miss because
    // we're under the harness's empty dist/.)
    let dir = reqwest::get(h.url("/img")).await.unwrap();
    assert_eq!(
        dir.status(),
        404,
        "directory request should fall through to dev 404"
    );
}

/// Path traversal must not escape `public_root`. The `is_safe_url_path`
/// gate (shared with the dist fallback) rejects `..` and absolute
/// segments before any disk read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_fallback_rejects_path_traversal() {
    let h = Harness::start().await;
    // Place a secret file OUTSIDE public/ but inside the harness tmp,
    // so a successful traversal would surface it.
    std::fs::write(h.tmp_path().join("secret.txt"), b"top secret").unwrap();

    // `/%2e%2e/secret.txt` decodes to `/../secret.txt`. Must 404, not
    // 200 with the secret bytes.
    let resp = reqwest::get(h.url("/%2e%2e/secret.txt")).await.unwrap();
    assert_eq!(resp.status(), 404);
    let body = resp.text().await.unwrap();
    assert!(
        !body.contains("top secret"),
        "secret file must not leak through traversal, body: {body}"
    );
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

// `wait_for_subscribers` + `next_sse_event_name` were promoted to
// `zfb-test-utils` (#1018) so the `zfb dev` E2E harness
// (`crates/zfb/tests/dev_serve_e2e.rs`) can reuse them — see the
// `use zfb_test_utils::...` import at the top of this file.

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

// ---------------------------------------------------------------------------
// Occupied-port bind ordering
// ---------------------------------------------------------------------------

/// `serve` must return an error (port in use) when the requested address
/// is already occupied. This exercises the bind-first contract: the
/// function binds before the caller prints any ready banner, so a
/// failed bind never causes a stale "ready" announcement.
#[tokio::test]
async fn serve_returns_error_when_port_is_occupied() {
    use zfb_server::serve;

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    let dist_root = root.join("dist");
    std::fs::create_dir_all(dist_root.join("assets")).unwrap();
    let public_root = root.join("public");
    std::fs::create_dir_all(&public_root).unwrap();

    // Grab an ephemeral port by binding it ourselves, then keep the
    // listener alive so the port stays occupied.
    let occupier = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("bind occupier");
    let occupied_addr = occupier.local_addr().expect("local_addr");

    let (tx, _rx) = broadcast::channel::<ReloadEvent>(64);
    let opts = ServeOpts {
        project_root: root.clone(),
        dist_root: dist_root.clone(),
        dev_assets_root: None,
        html_root: dist_root,
        public_root,
        addr: occupied_addr,
        pages: PageCache::new(),
        broadcast: tx,
        plugins: None,
        injected_routes: None,
        ssr_routes: None,
        base: None,
        trailing_slash: false,
        mode: zfb_server::ServerMode::Dev,
        islands_bundle_url: None,
        css_bundle_url: None,
        allowed_hosts: Vec::new(),
        bound_host: None,
        render_on_request_hook: None,
    };

    let result = serve(opts, std::future::ready(())).await;
    assert!(
        result.is_err(),
        "serve on an occupied port must return an error"
    );
    let msg = format!("{:#}", result.unwrap_err());
    assert!(
        msg.contains("failed to bind"),
        "error message must mention bind failure, got: {msg}"
    );

    // Keep the occupier alive until after the assertion so the OS does
    // not reclaim the port before `serve` attempts its bind.
    drop(occupier);
}

/// Islands code-split chunks written to `dist/assets/` alongside
/// `islands.js` must be served at `/assets/<chunk-filename>` (issue #809).
///
/// The dev server serves `/assets/*` from `<dist_root>/assets/` via
/// ServeDir. When the islands bundler emits code-split chunks, the dev
/// command writes them to that same directory so the entry's relative
/// `import("./islands-chunk-<hash>.js")` can resolve without any extra
/// routing code.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn islands_chunks_served_from_assets_dir() {
    let h = Harness::start().await;
    let dist_root = h.tmp_path().join("dist");

    // Simulate what dev.rs's refresh_dev_island_chunks writes: the islands
    // entry and a code-split chunk, both under dist/assets/.
    let islands_js = b"export default function() {}; import('./islands-chunk-TESTHASH.js');";
    let chunk_js = b"export function LazyComponent() {}";

    std::fs::write(dist_root.join("assets/islands.js"), islands_js).unwrap();
    std::fs::write(dist_root.join("assets/islands-chunk-TESTHASH.js"), chunk_js).unwrap();

    // The entry must be served.
    let resp = reqwest::get(h.url("/assets/islands.js")).await.unwrap();
    assert_eq!(resp.status(), 200, "islands.js must be served");
    assert_eq!(resp.bytes().await.unwrap().as_ref(), islands_js);

    // The code-split chunk must also be served — no extra routing needed.
    let resp = reqwest::get(h.url("/assets/islands-chunk-TESTHASH.js"))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "islands-chunk-TESTHASH.js must be served alongside the entry"
    );
    assert_eq!(resp.bytes().await.unwrap().as_ref(), chunk_js);
}

/// After a dev rebundle that changes the dynamically-imported module, the
/// new chunk must be served and the previous generation's chunk must 404
/// (issue #809).
///
/// This test directly exercises the `refresh_dev_island_chunks` write/prune
/// logic by writing chunk files into the harness's dist/assets/ dir (the
/// real dev command delegates to that function) and verifying that the
/// old chunk disappears from the filesystem and the server.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_chunk_pruned_after_rebundle() {
    let h = Harness::start().await;
    let assets_dir = h.tmp_path().join("dist/assets");

    // Generation 1: write a chunk.
    let gen1_chunk = "islands-chunk-GEN1XXXX.js";
    std::fs::write(assets_dir.join(gen1_chunk), b"// gen1 chunk").unwrap();

    // Verify gen-1 chunk is served.
    let resp = reqwest::get(h.url(&format!("/assets/{gen1_chunk}")))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "gen-1 chunk must be served before rebundle"
    );

    // Simulate a rebundle: delete the old chunk, write the new one.
    std::fs::remove_file(assets_dir.join(gen1_chunk)).unwrap();
    let gen2_chunk = "islands-chunk-GEN2YYYY.js";
    std::fs::write(assets_dir.join(gen2_chunk), b"// gen2 chunk").unwrap();

    // Gen-1 chunk must 404 now (pruned).
    let resp = reqwest::get(h.url(&format!("/assets/{gen1_chunk}")))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        404,
        "gen-1 chunk must 404 after rebundle pruned it"
    );

    // Gen-2 chunk must be served.
    let resp = reqwest::get(h.url(&format!("/assets/{gen2_chunk}")))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "gen-2 chunk must be served after rebundle"
    );
}
