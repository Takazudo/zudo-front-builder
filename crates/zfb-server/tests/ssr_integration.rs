//! Integration test for the request-time SSR layer (Gap 1, #367).
//!
//! Boots the real axum server with a synthetic [`SsrDispatcher`] —
//! the V8 host itself is exercised end-to-end in `zfb-render`'s
//! embedded_v8 smoke suite — and asserts:
//!
//! 1. a URL pattern matched by [`SsrRouteSet`] is dispatched through
//!    the SSR layer and the response carries the rendered body /
//!    status / content-type returned by the dispatcher,
//! 2. plugin dev-middleware still wins (highest precedence) so an SSR
//!    pattern shadowed by a plugin does not double-dispatch,
//! 3. an unmatched URL falls through to the page cache (next
//!    precedence step after SSR),
//! 4. a dispatcher error surfaces as a 500 with the error message in
//!    the body, instead of swallowing the failure into a 404.
//!
//! Uses the same ephemeral-port binding pattern as `integration.rs`.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use zfb_server::livereload::ReloadEvent;
use zfb_server::{
    serve_with_listener, DevMiddlewareDispatcher, DevMiddlewareSet, PageCache,
    PluginDispatchError, PluginDispatchOutcome, PluginRegistration, PluginRequest, PluginResponse,
    PluginResponseEncoding, ServeOpts, SsrDispatchError, SsrDispatcher, SsrRequest, SsrResponse,
    SsrRouteRecord, SsrRouteSet, SsrRoutesHandle,
};

/// Records the last request and returns a fixed canned response.
struct RecordingSsrDispatcher {
    invocations: AtomicU32,
    last: tokio::sync::Mutex<Option<SsrRequest>>,
    canned: SsrResponse,
}

impl RecordingSsrDispatcher {
    fn new(canned: SsrResponse) -> Self {
        Self {
            invocations: AtomicU32::new(0),
            last: tokio::sync::Mutex::new(None),
            canned,
        }
    }
}

#[async_trait]
impl SsrDispatcher for RecordingSsrDispatcher {
    async fn dispatch(&self, request: SsrRequest) -> Result<SsrResponse, SsrDispatchError> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        *self.last.lock().await = Some(request.clone());
        Ok(self.canned.clone())
    }
}

/// Dispatcher that always fails — used to confirm the dev server
/// surfaces V8 errors as a visible 500 instead of silently 404ing.
struct FailingSsrDispatcher;

#[async_trait]
impl SsrDispatcher for FailingSsrDispatcher {
    async fn dispatch(&self, request: SsrRequest) -> Result<SsrResponse, SsrDispatchError> {
        Err(SsrDispatchError {
            url_path: request.url_path,
            message: "Error: simulated SSR failure".into(),
        })
    }
}

/// No-op plugin dispatcher used by the precedence test.
struct AlwaysRespondingPluginDispatcher;

#[async_trait]
impl DevMiddlewareDispatcher for AlwaysRespondingPluginDispatcher {
    async fn dispatch(
        &self,
        _id: &str,
        request: PluginRequest,
    ) -> Result<PluginDispatchOutcome, PluginDispatchError> {
        let mut headers = HashMap::new();
        headers.insert("content-type".into(), "text/plain; charset=utf-8".into());
        Ok(PluginDispatchOutcome::Response(PluginResponse {
            status: 200,
            headers,
            body: format!("plugin handled {}", request.url),
            body_encoding: PluginResponseEncoding::Utf8,
        }))
    }
}

async fn boot_with_base(
    ssr_routes: Option<SsrRouteSet>,
    plugin_set: Option<DevMiddlewareSet>,
    base: Option<String>,
) -> (
    SocketAddr,
    PageCache,
    tokio::task::JoinHandle<anyhow::Result<()>>,
    tempfile::TempDir,
) {
    boot_with_opts(ssr_routes, plugin_set, base, zfb_server::ServerMode::Dev).await
}

async fn boot_with_opts(
    ssr_routes: Option<SsrRouteSet>,
    plugin_set: Option<DevMiddlewareSet>,
    base: Option<String>,
    mode: zfb_server::ServerMode,
) -> (
    SocketAddr,
    PageCache,
    tokio::task::JoinHandle<anyhow::Result<()>>,
    tempfile::TempDir,
) {
    let tmp = tempfile::tempdir().unwrap();
    let dist_root = tmp.path().join("dist");
    let public_root = tmp.path().join("public");
    std::fs::create_dir_all(dist_root.join("assets")).unwrap();
    std::fs::create_dir_all(&public_root).unwrap();

    let pages = PageCache::new();
    let (tx, _rx) = broadcast::channel::<ReloadEvent>(8);
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();

    // Wrap the SsrRouteSet in the live handle (issue #807).
    let ssr_handle: Option<SsrRoutesHandle> =
        ssr_routes.map(|s| Arc::new(RwLock::new(Some(s))));

    let opts = ServeOpts {
        project_root: tmp.path().to_path_buf(),
        dist_root: dist_root.clone(),
        html_root: dist_root,
        public_root,
        addr,
        pages: pages.clone(),
        broadcast: tx,
        plugins: plugin_set,
        injected_routes: None,
        ssr_routes: ssr_handle,
        base,
        trailing_slash: false,
        mode,
        islands_bundle_url: None,
        css_bundle_url: None,
    };
    let server = tokio::spawn(async move {
        serve_with_listener(opts, listener, std::future::pending::<()>()).await
    });
    tokio::task::yield_now().await;
    (addr, pages, server, tmp)
}

async fn boot(
    ssr_routes: Option<SsrRouteSet>,
    plugin_set: Option<DevMiddlewareSet>,
) -> (
    SocketAddr,
    PageCache,
    tokio::task::JoinHandle<anyhow::Result<()>>,
    tempfile::TempDir,
) {
    boot_with_base(ssr_routes, plugin_set, None).await
}

fn ssr_set(pattern: &str, dispatcher: Arc<dyn SsrDispatcher>) -> SsrRouteSet {
    SsrRouteSet::new(
        vec![SsrRouteRecord {
            pattern: pattern.into(),
        }],
        dispatcher,
    )
}

#[tokio::test]
async fn matched_url_dispatches_through_ssr_layer() {
    // Canned response: HTML with custom Set-Cookie header.
    let mut headers = BTreeMap::new();
    headers.insert("content-type".into(), "text/html; charset=utf-8".into());
    headers.insert("set-cookie".into(), "session=abc; HttpOnly".into());
    let canned = SsrResponse {
        status: 200,
        headers,
        body: b"<html><body><h1>request-time</h1></body></html>".to_vec(),
    };
    let dispatcher = Arc::new(RecordingSsrDispatcher::new(canned));
    let set = ssr_set("/dynamic", dispatcher.clone() as Arc<dyn SsrDispatcher>);
    let (addr, _pages, server, _tmp) = boot(Some(set), None).await;

    let client = reqwest::Client::builder().build().unwrap();
    let resp = client
        .get(format!("http://{addr}/dynamic?q=1"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(
        ct.starts_with("text/html"),
        "expected text/html, got {ct}"
    );
    let cookie = resp
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(cookie.contains("session=abc"));
    let body = resp.text().await.unwrap();
    assert!(body.contains("request-time"));
    // HTML responses always get the live-reload script wired up
    // (matches the page-cache path).
    assert!(body.contains("/__zfb/livereload.js"));

    // The dispatcher should have seen the query string verbatim.
    let captured = dispatcher.last.lock().await.clone().unwrap();
    assert_eq!(captured.method, "GET");
    assert_eq!(captured.url_path, "/dynamic?q=1");
    assert_eq!(dispatcher.invocations.load(Ordering::SeqCst), 1);

    server.abort();
}

#[tokio::test]
async fn plugin_middleware_wins_over_ssr() {
    // Same path is claimed by both a plugin and the SSR set; the plugin
    // must win per the documented precedence (plugin > SSR > cache).
    let canned = SsrResponse {
        status: 200,
        headers: BTreeMap::new(),
        body: b"ssr body".to_vec(),
    };
    let ssr_dispatcher = Arc::new(RecordingSsrDispatcher::new(canned));
    let set = ssr_set(
        "/api/dynamic",
        ssr_dispatcher.clone() as Arc<dyn SsrDispatcher>,
    );

    let plugin_set = DevMiddlewareSet {
        registrations: Arc::new(vec![PluginRegistration {
            path: "/api/dynamic".into(),
            handler_id: "h1".into(),
            plugin: "test".into(),
        }]),
        dispatcher: Arc::new(AlwaysRespondingPluginDispatcher) as Arc<dyn DevMiddlewareDispatcher>,
    };

    let (addr, _pages, server, _tmp) = boot(Some(set), Some(plugin_set)).await;

    let client = reqwest::Client::builder().build().unwrap();
    let resp = client
        .get(format!("http://{addr}/api/dynamic"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("plugin handled"),
        "plugin must take precedence over SSR; got {body}"
    );

    // SSR dispatcher must not have been touched.
    assert_eq!(
        ssr_dispatcher.invocations.load(Ordering::SeqCst),
        0,
        "SSR dispatcher should be skipped when a plugin claims the URL"
    );

    server.abort();
}

#[tokio::test]
async fn unmatched_url_falls_through_to_page_cache() {
    let canned = SsrResponse {
        status: 200,
        headers: BTreeMap::new(),
        body: b"ssr".to_vec(),
    };
    let ssr_dispatcher = Arc::new(RecordingSsrDispatcher::new(canned));
    let set = ssr_set(
        "/dynamic",
        ssr_dispatcher.clone() as Arc<dyn SsrDispatcher>,
    );
    let (addr, pages, server, _tmp) = boot(Some(set), None).await;

    // Static page in the cache at a DIFFERENT URL — confirm the SSR
    // layer doesn't intercept it.
    pages
        .insert("/static", "<html><body>static</body></html>")
        .await;

    let client = reqwest::Client::builder().build().unwrap();
    let resp = client
        .get(format!("http://{addr}/static"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("static"));
    assert_eq!(
        ssr_dispatcher.invocations.load(Ordering::SeqCst),
        0,
        "SSR dispatcher must be skipped for unmatched URLs"
    );

    server.abort();
}

#[tokio::test]
async fn dispatcher_error_surfaces_as_500() {
    let set = ssr_set(
        "/dynamic",
        Arc::new(FailingSsrDispatcher) as Arc<dyn SsrDispatcher>,
    );
    let (addr, _pages, server, _tmp) = boot(Some(set), None).await;

    let client = reqwest::Client::builder().build().unwrap();
    let resp = client
        .get(format!("http://{addr}/dynamic"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        500,
        "dispatcher errors must surface as 500, not 404"
    );
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("simulated SSR failure"),
        "error body should include the underlying message: {body}"
    );

    server.abort();
}

#[tokio::test]
async fn base_prefix_stripped_from_url_path_dispatched_to_ssr() {
    // When the project sets `base: "/docs/"`, dev requests come in as
    // `/docs/dynamic?x=1` but the SSR handler must see `/dynamic?x=1`
    // — the same shape Cloudflare delivers in production. Without the
    // prefix strip, the V8 host's `request.url.pathname` would diverge
    // between dev and prod for any `prerender = false` page that
    // inspects the path.
    let canned = SsrResponse {
        status: 200,
        headers: BTreeMap::new(),
        body: b"ok".to_vec(),
    };
    let dispatcher = Arc::new(RecordingSsrDispatcher::new(canned));
    let set = ssr_set("/dynamic", dispatcher.clone() as Arc<dyn SsrDispatcher>);
    let (addr, _pages, server, _tmp) =
        boot_with_base(Some(set), None, Some("/docs/".to_string())).await;

    let client = reqwest::Client::builder().build().unwrap();
    let resp = client
        .get(format!("http://{addr}/docs/dynamic?x=1"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    let captured = dispatcher.last.lock().await.clone().unwrap();
    assert_eq!(
        captured.url_path, "/dynamic?x=1",
        "dev base prefix must be stripped before dispatching to the SSR handler"
    );

    server.abort();
}

#[tokio::test]
async fn post_to_ssr_route_reaches_dispatcher() {
    // SSR layer must accept all HTTP methods — `prerender = false`
    // pages can implement non-GET endpoints exactly the way they
    // would in Cloudflare. This mirrors the plugin layer.
    let canned = SsrResponse {
        status: 201,
        headers: {
            let mut h = BTreeMap::new();
            h.insert("content-type".into(), "application/json".into());
            h
        },
        body: br#"{"ok":true}"#.to_vec(),
    };
    let dispatcher = Arc::new(RecordingSsrDispatcher::new(canned));
    let set = ssr_set("/api/submit", dispatcher.clone() as Arc<dyn SsrDispatcher>);
    let (addr, _pages, server, _tmp) = boot(Some(set), None).await;

    let client = reqwest::Client::builder().build().unwrap();
    let resp = client
        .post(format!("http://{addr}/api/submit"))
        .header("content-type", "application/json")
        .body("{\"x\":1}")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 201);

    let captured = dispatcher.last.lock().await.clone().unwrap();
    assert_eq!(captured.method, "POST");
    assert_eq!(captured.body.as_slice(), b"{\"x\":1}");
    assert!(captured
        .headers
        .get("content-type")
        .map(|v| v.contains("application/json"))
        .unwrap_or(false));

    server.abort();
}

// ---------------------------------------------------------------------------
// Live SsrRouteSet (issue #807 Gap 2).
//
// The `SsrRoutesHandle` is an `Arc<RwLock<Option<SsrRouteSet>>>`. A writer
// (the per-tick reload callback) can swap in a fresh route set mid-session.
// These tests verify:
//   1. Adding a new SSR route mid-session makes it dispatchable immediately.
//   2. Removing a route mid-session makes it 404 on the next request.
//   3. Mixed SSG/SSR: the SSR set update does not break static page serving.
// ---------------------------------------------------------------------------

/// Helper: boot a server with a live `SsrRoutesHandle` and return the handle
/// so the test can write fresh route sets mid-session.
async fn boot_with_live_handle(
    initial_ssr: Option<SsrRouteSet>,
    _dispatcher: Arc<dyn SsrDispatcher>,
) -> (
    SocketAddr,
    SsrRoutesHandle,
    tokio::task::JoinHandle<anyhow::Result<()>>,
    tempfile::TempDir,
    PageCache,
) {
    let tmp = tempfile::tempdir().unwrap();
    let dist_root = tmp.path().join("dist");
    let public_root = tmp.path().join("public");
    std::fs::create_dir_all(dist_root.join("assets")).unwrap();
    std::fs::create_dir_all(&public_root).unwrap();

    let pages = PageCache::new();
    let (tx, _rx) = broadcast::channel::<ReloadEvent>(8);
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();

    // The handle wraps the initial set; the test will mutate it later.
    let handle: SsrRoutesHandle = Arc::new(RwLock::new(initial_ssr));

    let opts = ServeOpts {
        project_root: tmp.path().to_path_buf(),
        dist_root: dist_root.clone(),
        html_root: dist_root,
        public_root,
        addr,
        pages: pages.clone(),
        broadcast: tx,
        plugins: None,
        injected_routes: None,
        ssr_routes: Some(Arc::clone(&handle)),
        base: None,
        trailing_slash: false,
        mode: zfb_server::ServerMode::Dev,
        islands_bundle_url: None,
        css_bundle_url: None,
    };
    let server = tokio::spawn(async move {
        serve_with_listener(opts, listener, std::future::pending::<()>()).await
    });
    tokio::task::yield_now().await;
    (addr, handle, server, tmp, pages)
}

/// Adding a new `prerender = false` route mid-session: before the write the
/// URL 404s; after the write (simulating a reload callback updating the handle)
/// the URL dispatches through the SSR set and returns 200.
#[tokio::test]
async fn adding_ssr_route_mid_session_becomes_dispatchable() {
    let canned = SsrResponse {
        status: 200,
        headers: {
            let mut h = std::collections::BTreeMap::new();
            h.insert("content-type".into(), "text/plain".into());
            h
        },
        body: b"new-route-body".to_vec(),
    };
    let dispatcher = Arc::new(RecordingSsrDispatcher::new(canned));

    // Boot with an empty initial route set (no SSR routes at startup).
    let empty_set = SsrRouteSet::new(vec![], dispatcher.clone() as Arc<dyn SsrDispatcher>);
    let (addr, handle, server, _tmp, _pages) =
        boot_with_live_handle(Some(empty_set), dispatcher.clone() as Arc<dyn SsrDispatcher>).await;

    let client = reqwest::Client::new();

    // Baseline: route not yet registered → 404.
    let before = client
        .get(format!("http://{addr}/new-route"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        before.status().as_u16(),
        404,
        "/new-route must 404 before it is added"
    );

    // Simulate the reload callback writing a fresh SsrRouteSet with the new route.
    {
        let new_set = SsrRouteSet::new(
            vec![SsrRouteRecord {
                pattern: "/new-route".into(),
            }],
            dispatcher.clone() as Arc<dyn SsrDispatcher>,
        );
        *handle.write().unwrap() = Some(new_set);
    }

    // After the update the route must dispatch.
    let after = client
        .get(format!("http://{addr}/new-route"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        after.status().as_u16(),
        200,
        "/new-route must return 200 after being added to the live handle"
    );
    let body = after.text().await.unwrap();
    assert!(
        body.contains("new-route-body"),
        "body must contain the SSR response; got: {body}"
    );

    server.abort();
}

/// Removing a `prerender = false` route mid-session: route is initially live,
/// then the handle is updated to remove it, and the next request gets 404.
#[tokio::test]
async fn removing_ssr_route_mid_session_becomes_404() {
    let canned = SsrResponse {
        status: 200,
        headers: std::collections::BTreeMap::new(),
        body: b"dynamic-body".to_vec(),
    };
    let dispatcher = Arc::new(RecordingSsrDispatcher::new(canned));

    // Boot with one active route.
    let initial_set = SsrRouteSet::new(
        vec![SsrRouteRecord {
            pattern: "/dynamic".into(),
        }],
        dispatcher.clone() as Arc<dyn SsrDispatcher>,
    );
    let (addr, handle, server, _tmp, _pages) =
        boot_with_live_handle(Some(initial_set), dispatcher.clone() as Arc<dyn SsrDispatcher>).await;

    let client = reqwest::Client::new();

    // Baseline: route exists → 200.
    let before = client
        .get(format!("http://{addr}/dynamic"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        before.status().as_u16(),
        200,
        "/dynamic must return 200 when registered"
    );

    // Simulate the reload callback removing the route (empty set).
    {
        let empty_set = SsrRouteSet::new(vec![], dispatcher.clone() as Arc<dyn SsrDispatcher>);
        *handle.write().unwrap() = Some(empty_set);
    }

    // Route is gone → 404.
    let after = client
        .get(format!("http://{addr}/dynamic"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        after.status().as_u16(),
        404,
        "/dynamic must 404 after being removed from the live handle"
    );

    server.abort();
}

/// Mixed SSG/SSR: updating the SSR route handle must not break static pages
/// already in the page cache.
#[tokio::test]
async fn ssr_route_update_does_not_break_static_pages() {
    let canned = SsrResponse {
        status: 200,
        headers: std::collections::BTreeMap::new(),
        body: b"ssr-body".to_vec(),
    };
    let dispatcher = Arc::new(RecordingSsrDispatcher::new(canned));

    let (addr, handle, server, _tmp, pages) =
        boot_with_live_handle(None, dispatcher.clone() as Arc<dyn SsrDispatcher>).await;

    // Seed a static SSG page.
    pages
        .insert("/static", "<html><body>static</body></html>")
        .await;

    let client = reqwest::Client::new();

    // Static page must serve before any SSR changes.
    let r1 = client
        .get(format!("http://{addr}/static"))
        .send()
        .await
        .unwrap();
    assert_eq!(r1.status().as_u16(), 200);

    // Now add an SSR route mid-session.
    {
        let new_set = SsrRouteSet::new(
            vec![SsrRouteRecord {
                pattern: "/ssr-new".into(),
            }],
            dispatcher.clone() as Arc<dyn SsrDispatcher>,
        );
        *handle.write().unwrap() = Some(new_set);
    }

    // SSR route now dispatches.
    let r2 = client
        .get(format!("http://{addr}/ssr-new"))
        .send()
        .await
        .unwrap();
    assert_eq!(r2.status().as_u16(), 200, "/ssr-new must dispatch after handle update");

    // Static page still serves correctly after the SSR update.
    let r3 = client
        .get(format!("http://{addr}/static"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r3.status().as_u16(),
        200,
        "/static must still serve 200 after SSR handle update"
    );
    let body = r3.text().await.unwrap();
    assert!(body.contains("static"), "static page body must be unchanged");

    server.abort();
}

// -------------------------------------------------------------------
// Issue #926 — error-body gating: Dev vs Preview/Embed
// -------------------------------------------------------------------

/// In Dev mode, an SSR dispatch error must return a 500 whose body
/// contains the underlying error message (V8 stack trace visible to
/// the developer in the browser).
#[tokio::test]
async fn ssr_error_body_is_verbose_in_dev_mode() {
    let set = ssr_set(
        "/dynamic",
        Arc::new(FailingSsrDispatcher) as Arc<dyn SsrDispatcher>,
    );
    let (addr, _pages, server, _tmp) =
        boot_with_opts(Some(set), None, None, zfb_server::ServerMode::Dev).await;

    let client = reqwest::Client::builder().build().unwrap();
    let resp = client
        .get(format!("http://{addr}/dynamic"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 500);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("simulated SSR failure"),
        "Dev mode must include the error detail in the body; got: {body}"
    );

    server.abort();
}

/// In Preview mode, an SSR dispatch error must return a 500 with a
/// generic body — the V8/internal detail must not leak to the client.
#[tokio::test]
async fn ssr_error_body_is_generic_in_preview_mode() {
    let set = ssr_set(
        "/dynamic",
        Arc::new(FailingSsrDispatcher) as Arc<dyn SsrDispatcher>,
    );
    let (addr, _pages, server, _tmp) =
        boot_with_opts(Some(set), None, None, zfb_server::ServerMode::Preview).await;

    let client = reqwest::Client::builder().build().unwrap();
    let resp = client
        .get(format!("http://{addr}/dynamic"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 500);
    let body = resp.text().await.unwrap();
    assert!(
        !body.contains("simulated SSR failure"),
        "Preview mode must not leak the error detail in the body; got: {body}"
    );
    assert!(
        body.contains("Internal Server Error"),
        "Preview mode must return a generic error body; got: {body}"
    );

    server.abort();
}
