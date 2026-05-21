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
use std::sync::Arc;

use async_trait::async_trait;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use zfb_server::livereload::ReloadEvent;
use zfb_server::{
    serve_with_listener, DevMiddlewareDispatcher, DevMiddlewareSet, PageCache,
    PluginDispatchError, PluginDispatchOutcome, PluginRegistration, PluginRequest, PluginResponse,
    PluginResponseEncoding, ServeOpts, SsrDispatchError, SsrDispatcher, SsrRequest, SsrResponse,
    SsrRouteRecord, SsrRouteSet,
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

    let opts = ServeOpts {
        project_root: tmp.path().to_path_buf(),
        dist_root,
        public_root,
        addr,
        pages: pages.clone(),
        broadcast: tx,
        plugins: plugin_set,
        injected_routes: None,
        ssr_routes,
        base,
        trailing_slash: false,
        mode: zfb_server::ServerMode::Dev,
        islands_bundle_url: None,
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
