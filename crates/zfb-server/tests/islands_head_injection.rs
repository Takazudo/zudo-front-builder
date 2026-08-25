//! Integration tests for issue #377 — dev-mode islands `<script>` tag
//! injection on initial page load.
//!
//! The dev `BuildOrchestrator`'s `run_islands` callback (wired in the
//! bin crate `crates/zfb/src/commands/dev.rs`) writes the current
//! islands bundle URL into a shared `Arc<RwLock<DevPublicationState>>` that
//! lives on `AppState.islands_bundle_url`. These tests cover the
//! response-shaping side:
//!
//! 1. With the URL seeded and `mode == ServerMode::Dev`, a served HTML
//!    page gains a `<script type="module" src="<url>"></script>` in
//!    `<head>` and still gets the livereload `<script>` near `</body>`.
//! 2. With the URL seeded but `mode == ServerMode::Preview`, no
//!    injection happens (production-shaped response — Embed users must
//!    not ship the unhashed `/assets/islands.js` URL).
//! 3. With `islands_bundle_url == None`, no script tag is injected — the
//!    pre-#377 behaviour for projects without `"use client"` components.

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use zfb_server::livereload::ReloadEvent;
use zfb_server::{serve_with_listener, PageCache, ServeOpts};

/// Minimal harness for the islands head-injection tests. Mirrors the
/// shape of the existing `integration.rs` Harness but lets callers
/// customise the `islands_bundle_url` handle and the server mode.
struct Harness {
    addr: SocketAddr,
    pages: PageCache,
    server: tokio::task::JoinHandle<anyhow::Result<()>>,
    _tmp: TempDir,
}

impl Harness {
    async fn start(
        mode: zfb_server::ServerMode,
        islands_bundle_url: Option<zfb_server::IslandsBundleUrl>,
    ) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let dist_root = root.join("dist");
        let public_root = root.join("public");
        std::fs::create_dir_all(dist_root.join("assets")).unwrap();
        std::fs::create_dir_all(&public_root).unwrap();

        let pages = PageCache::new();
        let (tx, _rx) = broadcast::channel::<ReloadEvent>(16);

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
            broadcast: tx,
            plugins: None,
            injected_routes: None,
            ssr_routes: None,
            base: None,
            trailing_slash: false,
            mode,
            islands_bundle_url,
            css_bundle_url: None,
            allowed_hosts: Vec::new(),
            bound_host: None,
            render_on_request_hook: None,
            redirects: None,
        };

        let server = tokio::spawn(async move {
            serve_with_listener(opts, listener, std::future::pending::<()>()).await
        });
        tokio::task::yield_now().await;

        Self {
            addr,
            pages,
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
        self.server.abort();
    }
}

/// Project-specific magic value: the canonical stable islands URL the
/// dev orchestrator seeds when bundling completes. Pinned to the same
/// value `zfb_types::STABLE_ISLANDS_URL` declares so this test breaks
/// loudly if the URL contract drifts.
const ISLANDS_URL: &str = "/assets/islands.js";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dev_mode_injects_islands_script_into_head_when_url_seeded() {
    let handle: zfb_server::IslandsBundleUrl = Arc::new(RwLock::new(
        zfb_server::DevPublicationState::from_islands_url(Some(ISLANDS_URL.to_string())),
    ));
    let h = Harness::start(zfb_server::ServerMode::Dev, Some(handle)).await;

    h.pages
        .insert(
            "/",
            "<!doctype html><html><head><title>t</title></head><body><div data-zfb-island=\"X\" data-when=\"visible\"></div></body></html>",
        )
        .await;

    let resp = reqwest::get(h.url("/")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    // The new tag is spliced before </head>.
    let close_head = body.find("</head>").expect("body must contain </head>");
    let script = body
        .find("<script type=\"module\" src=\"/assets/islands.js\"></script>")
        .expect("body must contain islands module script tag");
    assert!(
        script < close_head,
        "islands <script> must sit inside <head>, got body: {body}"
    );

    // The livereload tag still lands near </body> — the new injection
    // must not displace the existing dev-server behaviour.
    assert!(
        body.contains("<script src=\"/__zfb/livereload.js\"></script>"),
        "livereload tag must still appear, body: {body}"
    );

    // The original island marker must round-trip — the splicer must
    // not corrupt user-authored content.
    assert!(
        body.contains("data-zfb-island=\"X\""),
        "island marker must round-trip, body: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dev_mode_skips_islands_script_when_url_handle_is_none_inside() {
    // Outer `Some(handle)` with inner `None` is the "no `"use client"`
    // components in this project" shape. The server must still skip
    // injection — shipping a `<script src="/assets/islands.js">` for a
    // bundle that was never written would 404 on every page load.
    let handle: zfb_server::IslandsBundleUrl = Arc::new(RwLock::new(
        zfb_server::DevPublicationState::from_islands_url(None),
    ));
    let h = Harness::start(zfb_server::ServerMode::Dev, Some(handle)).await;

    h.pages
        .insert(
            "/",
            "<!doctype html><html><head><title>t</title></head><body><p>no islands here</p></body></html>",
        )
        .await;

    let resp = reqwest::get(h.url("/")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        !body.contains("/assets/islands.js"),
        "must not inject the islands script when the handle is empty, body: {body}"
    );
    // The livereload tag MUST still be present — the no-islands
    // skip must not regress the dev-server's livereload contract.
    assert!(
        body.contains("<script src=\"/__zfb/livereload.js\"></script>"),
        "livereload tag must still appear, body: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dev_mode_skips_islands_script_when_app_state_has_no_handle() {
    // Outer `None` means the bin crate did not hand the server an
    // islands-URL handle at all. Same effective behaviour as the inner-
    // `None` case above (skip injection), but exercised as a separate
    // arm so a future refactor that collapses the two outer states
    // breaks loudly.
    let h = Harness::start(zfb_server::ServerMode::Dev, None).await;

    h.pages
        .insert(
            "/",
            "<!doctype html><html><head></head><body></body></html>",
        )
        .await;

    let resp = reqwest::get(h.url("/")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        !body.contains("/assets/islands.js"),
        "must not inject without a configured handle, body: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preview_mode_never_injects_islands_script_even_with_seeded_url() {
    // Preview / Embed callers ship production-shaped responses. Even
    // when a caller passes a seeded `islands_bundle_url`, the response
    // shaper must skip the injection — leaking the stable, unhashed
    // `/assets/islands.js` URL into production HTML would defeat the
    // prod pipeline's content-hash rewrite (which only matches its own
    // stable forms; an in-flight injection here would not).
    let handle: zfb_server::IslandsBundleUrl = Arc::new(RwLock::new(
        zfb_server::DevPublicationState::from_islands_url(Some(ISLANDS_URL.to_string())),
    ));
    let h = Harness::start(zfb_server::ServerMode::Preview, Some(handle)).await;

    h.pages
        .insert(
            "/",
            "<!doctype html><html><head><title>t</title></head><body><div data-zfb-island=\"X\"></div></body></html>",
        )
        .await;

    let resp = reqwest::get(h.url("/")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        !body.contains("/assets/islands.js"),
        "preview mode must not inject the islands script, body: {body}"
    );
    // Preview also drops the livereload script — sanity check we are
    // exercising the production-shaped path.
    assert!(
        !body.contains("/__zfb/livereload.js"),
        "preview must not inject livereload either, body: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clearing_the_shared_handle_stops_head_injection_on_next_request() {
    // Regression guard for the issue #377 codex review: when a project
    // removes its last `"use client"` component the `run_islands`
    // rebuild callback writes `None` back into the shared handle. The
    // server MUST stop injecting on the next request — otherwise the
    // previously-emitted bundle URL keeps appearing in HTML, leaving
    // stale hydration code referenced after the bundle is gone.
    let handle: zfb_server::IslandsBundleUrl = Arc::new(RwLock::new(
        zfb_server::DevPublicationState::from_islands_url(Some(ISLANDS_URL.to_string())),
    ));
    let h = Harness::start(zfb_server::ServerMode::Dev, Some(Arc::clone(&handle))).await;

    h.pages
        .insert(
            "/",
            "<!doctype html><html><head></head><body></body></html>",
        )
        .await;

    let resp = reqwest::get(h.url("/")).await.unwrap();
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("src=\"/assets/islands.js\""),
        "baseline: script tag must be present when handle holds Some(url), body: {body}"
    );

    // Simulate the orchestrator's "no islands left" rebuild outcome.
    handle.write().unwrap().publish_islands(Vec::new());

    let resp = reqwest::get(h.url("/")).await.unwrap();
    let body = resp.text().await.unwrap();
    assert!(
        !body.contains("/assets/islands.js"),
        "after clearing the handle, the script tag must be gone, body: {body}"
    );
    // Livereload tag must still be there — clearing the islands URL
    // must not regress the dev-server contract.
    assert!(
        body.contains("/__zfb/livereload.js"),
        "livereload tag must still appear, body: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dev_mode_double_request_is_idempotent_on_head_injection() {
    // The body is read off the page cache verbatim on every request
    // — the splicer is also called every time. Idempotency at the
    // splicer means the served body for two back-to-back requests
    // contains EXACTLY ONE islands `<script>` tag, not two.
    let handle: zfb_server::IslandsBundleUrl = Arc::new(RwLock::new(
        zfb_server::DevPublicationState::from_islands_url(Some(ISLANDS_URL.to_string())),
    ));
    let h = Harness::start(zfb_server::ServerMode::Dev, Some(handle)).await;

    h.pages
        .insert(
            "/",
            "<!doctype html><html><head></head><body></body></html>",
        )
        .await;

    let resp = reqwest::get(h.url("/")).await.unwrap();
    let body = resp.text().await.unwrap();
    assert_eq!(
        body.matches("src=\"/assets/islands.js\"").count(),
        1,
        "single islands script tag expected after one request, body: {body}"
    );

    let resp = reqwest::get(h.url("/")).await.unwrap();
    let body = resp.text().await.unwrap();
    assert_eq!(
        body.matches("src=\"/assets/islands.js\"").count(),
        1,
        "second request must still yield exactly one islands script tag, body: {body}"
    );
}

// ---------------------------------------------------------------------------
// Issue #530 — dev SSR doctype prepend
// ---------------------------------------------------------------------------

/// Regression test for issue #530: `page_response_bytes` must prepend
/// `<!doctype html>` on dev-mode `text/html` responses that lack a doctype.
///
/// All existing test fixtures seed pages with `<!doctype html>` already
/// present, which masks the bug.  This test uses a doctype-less body
/// (as a V8 renderer might produce for a `prerender = false` route) and
/// asserts the response body begins with `<!doctype html>`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dev_mode_prepends_doctype_when_body_lacks_one() {
    let h = Harness::start(zfb_server::ServerMode::Dev, None).await;

    // Insert a doctype-less HTML page — exactly what a V8 renderer
    // running a `prerender = false` route might hand back.
    h.pages
        .insert(
            "/",
            "<html><head><title>no-doctype</title></head><body><p>content</p></body></html>",
        )
        .await;

    let resp = reqwest::get(h.url("/")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.starts_with("<!doctype html>"),
        "dev SSR response must begin with <!doctype html> when the cached body lacks one; got: {body}"
    );
    // The original content must survive the prepend unchanged.
    assert!(
        body.contains("<title>no-doctype</title>"),
        "original page content must round-trip, body: {body}"
    );
    assert!(
        body.contains("<p>content</p>"),
        "body content must round-trip, body: {body}"
    );
}

/// Regression guard for issue #530: when the cached page body already
/// starts with `<!doctype html>` the response must not double-prepend.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dev_mode_does_not_double_prepend_doctype_when_already_present() {
    let h = Harness::start(zfb_server::ServerMode::Dev, None).await;

    h.pages
        .insert(
            "/",
            "<!doctype html><html><head><title>has-doctype</title></head><body></body></html>",
        )
        .await;

    let resp = reqwest::get(h.url("/")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    // Exactly one `<!doctype` in the response.
    let count = body.to_lowercase().matches("<!doctype").count();
    assert_eq!(
        count, 1,
        "must not double-prepend <!doctype html>; found {count} occurrences in: {body}"
    );
}
