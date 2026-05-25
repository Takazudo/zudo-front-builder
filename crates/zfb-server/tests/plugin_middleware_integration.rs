//! Integration test for the dev-middleware path (Sub 3 / #108).
//!
//! Boots the real axum server with a synthetic plugin-middleware
//! dispatcher (no Node subprocess — that's covered by the
//! `zfb-build::plugin_runner` unit tests), and confirms that:
//!
//! 1. requests to a registered path go through the plugin and the
//!    real HTTP response carries the plugin's status/body/headers,
//! 2. requests to an unregistered path fall through to the page cache,
//! 3. a handler returning `Passthrough` falls through to the cache too.
//!
//! Uses the same ephemeral-port binding pattern as `integration.rs`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use zfb_server::livereload::ReloadEvent;
use zfb_server::{
    serve_with_listener, DevMiddlewareDispatcher, DevMiddlewareSet, PageCache,
    PluginDispatchError, PluginDispatchOutcome, PluginRegistration, PluginRequest, PluginResponse,
    PluginResponseEncoding, ServeOpts,
};

struct CountingDispatcher {
    invocations: AtomicU32,
    passthrough_handler_id: String,
    response_handler_id: String,
    /// Records the last [`PluginRequest`] the dispatcher saw so
    /// method/header/body-propagation tests can assert on it.
    last_request: tokio::sync::Mutex<Option<PluginRequest>>,
}

impl CountingDispatcher {
    fn new(pass: &str, respond: &str) -> Self {
        Self {
            invocations: AtomicU32::new(0),
            passthrough_handler_id: pass.into(),
            response_handler_id: respond.into(),
            last_request: tokio::sync::Mutex::new(None),
        }
    }
}

#[async_trait]
impl DevMiddlewareDispatcher for CountingDispatcher {
    async fn dispatch(
        &self,
        handler_id: &str,
        request: PluginRequest,
    ) -> Result<PluginDispatchOutcome, PluginDispatchError> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        *self.last_request.lock().await = Some(request.clone());
        if handler_id == self.passthrough_handler_id {
            return Ok(PluginDispatchOutcome::Passthrough);
        }
        if handler_id == self.response_handler_id {
            let mut headers = HashMap::new();
            headers.insert("content-type".into(), "application/json".into());
            headers.insert("x-zfb-plugin".into(), "ok".into());
            // Echo what the plugin saw so the test can assert on the
            // wire-level method propagation rather than relying on an
            // out-of-band channel.
            let body = format!(
                "{{\"method\":\"{}\",\"url\":\"{}\",\"hasBody\":{}}}",
                request.method,
                request.url,
                request.body.is_some(),
            );
            return Ok(PluginDispatchOutcome::Response(PluginResponse {
                status: 200,
                headers,
                body,
                body_encoding: PluginResponseEncoding::Utf8,
            }));
        }
        Err(PluginDispatchError {
            plugin: "test".into(),
            message: format!("unknown handler {handler_id}"),
        })
    }
}

async fn boot_with_dispatcher(
    dispatcher: Arc<CountingDispatcher>,
    registrations: Vec<PluginRegistration>,
) -> (SocketAddr, PageCache, tokio::task::JoinHandle<anyhow::Result<()>>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let dist_root = tmp.path().join("dist");
    let public_root = tmp.path().join("public");
    std::fs::create_dir_all(dist_root.join("assets")).unwrap();
    std::fs::create_dir_all(&public_root).unwrap();

    let pages = PageCache::new();
    let (tx, _rx) = broadcast::channel::<ReloadEvent>(8);
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await.unwrap();
    let addr = listener.local_addr().unwrap();

    let plugin_set = DevMiddlewareSet {
        registrations: Arc::new(registrations),
        dispatcher: dispatcher.clone() as Arc<dyn DevMiddlewareDispatcher>,
    };
    let opts = ServeOpts {
        project_root: tmp.path().to_path_buf(),
        dist_root,
        public_root,
        addr,
        pages: pages.clone(),
        broadcast: tx,
        plugins: Some(plugin_set),
        injected_routes: None,
        ssr_routes: None,
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
    (addr, pages, server, tmp)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dev_middleware_handles_registered_path() {
    let dispatcher = Arc::new(CountingDispatcher::new("h-pass", "h-respond"));
    let (addr, _pages, server, _tmp) = boot_with_dispatcher(
        dispatcher.clone(),
        vec![PluginRegistration {
            path: "/api/echo".into(),
            handler_id: "h-respond".into(),
            plugin: "echo-test".into(),
        }],
    )
    .await;

    let resp = reqwest::get(format!("http://{addr}/api/echo?x=1")).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("x-zfb-plugin").and_then(|h| h.to_str().ok()),
        Some("ok"),
    );
    let ct = resp.headers().get("content-type").and_then(|h| h.to_str().ok());
    assert_eq!(ct, Some("application/json"));
    let body = resp.text().await.unwrap();
    assert!(body.contains("/api/echo?x=1"), "body: {body}");
    assert_eq!(dispatcher.invocations.load(Ordering::SeqCst), 1);

    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dev_middleware_passthrough_falls_back_to_page_cache() {
    let dispatcher = Arc::new(CountingDispatcher::new("h-pass", "h-respond"));
    let (addr, pages, server, _tmp) = boot_with_dispatcher(
        dispatcher.clone(),
        vec![PluginRegistration {
            path: "/maybe".into(),
            handler_id: "h-pass".into(),
            plugin: "passthrough-test".into(),
        }],
    )
    .await;
    pages
        .insert("/maybe", "<html><body><h1>cached</h1></body></html>")
        .await;

    let resp = reqwest::get(format!("http://{addr}/maybe")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("<h1>cached</h1>"), "body: {body}");
    // The plugin DID get called (and chose to pass) — confirms the
    // server reached the dispatcher rather than skipping it.
    assert_eq!(dispatcher.invocations.load(Ordering::SeqCst), 1);

    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dev_middleware_unregistered_path_skips_dispatch() {
    let dispatcher = Arc::new(CountingDispatcher::new("h-pass", "h-respond"));
    let (addr, _pages, server, _tmp) = boot_with_dispatcher(
        dispatcher.clone(),
        vec![PluginRegistration {
            path: "/api".into(),
            handler_id: "h-respond".into(),
            plugin: "scoped".into(),
        }],
    )
    .await;

    // No registered prefix matches `/different` — the dispatcher must
    // never be called, and the server returns the dev-mode 404.
    let resp = reqwest::get(format!("http://{addr}/different")).await.unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(dispatcher.invocations.load(Ordering::SeqCst), 0);

    server.abort();
}

// ---------------------------------------------------------------------
// Issue #230 — devMiddleware accepts every HTTP method end-to-end.
// ---------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dev_middleware_handles_post_with_body_and_propagates_method() {
    // The whole point of issue #230: a POST to a registered path now
    // reaches the plugin handler instead of being short-circuited by
    // axum's GET/HEAD-only filter. Verify the plugin sees the real
    // method and request body across the full HTTP stack.
    let dispatcher = Arc::new(CountingDispatcher::new("h-pass", "h-respond"));
    let (addr, _pages, server, _tmp) = boot_with_dispatcher(
        dispatcher.clone(),
        vec![PluginRegistration {
            path: "/api/echo".into(),
            handler_id: "h-respond".into(),
            plugin: "echo-test".into(),
        }],
    )
    .await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/api/echo"))
        .header("content-type", "application/json")
        .body("{\"x\":1}")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("\"method\":\"POST\""),
        "plugin did not see POST: {body}"
    );
    assert!(
        body.contains("\"hasBody\":true"),
        "plugin did not receive body: {body}"
    );
    let captured = dispatcher.last_request.lock().await.clone().unwrap();
    assert_eq!(captured.method, "POST");
    assert_eq!(captured.body.as_deref(), Some("{\"x\":1}"));
    assert_eq!(
        captured.headers.get("content-type").map(String::as_str),
        Some("application/json"),
    );

    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dev_middleware_handles_put_and_delete() {
    let dispatcher = Arc::new(CountingDispatcher::new("h-pass", "h-respond"));
    let (addr, _pages, server, _tmp) = boot_with_dispatcher(
        dispatcher.clone(),
        vec![PluginRegistration {
            path: "/api/echo".into(),
            handler_id: "h-respond".into(),
            plugin: "echo-test".into(),
        }],
    )
    .await;

    let client = reqwest::Client::new();
    for method in [reqwest::Method::PUT, reqwest::Method::DELETE, reqwest::Method::PATCH] {
        let resp = client
            .request(method.clone(), format!("http://{addr}/api/echo"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "method {method} should reach plugin");
        let body = resp.text().await.unwrap();
        assert!(
            body.contains(&format!("\"method\":\"{}\"", method.as_str())),
            "plugin did not see {method}: {body}",
        );
    }

    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_to_built_in_livereload_js_returns_405() {
    // Critical CSRF guarantee from issue #230's "security callout":
    // built-in routes keep their GET-only semantics. The dev-server
    // must not let a misrouted POST land on `/__zfb/livereload.js`
    // just because user-registered devMiddleware paths now accept
    // every method.
    let dispatcher = Arc::new(CountingDispatcher::new("h-pass", "h-respond"));
    let (addr, _pages, server, _tmp) = boot_with_dispatcher(
        dispatcher.clone(),
        // No registrations — exercises the built-in route directly.
        vec![],
    )
    .await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/__zfb/livereload.js"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 405);
    let allow = resp
        .headers()
        .get("allow")
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(allow.to_ascii_uppercase().contains("GET"), "allow: {allow}");
    // Plugin layer must NOT have been touched for a built-in path.
    assert_eq!(dispatcher.invocations.load(Ordering::SeqCst), 0);

    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_to_unregistered_path_returns_405() {
    // Non-GET requests that do not match any plugin must 405 rather
    // than slip into the page-cache lookup (which is GET-only).
    let dispatcher = Arc::new(CountingDispatcher::new("h-pass", "h-respond"));
    let (addr, _pages, server, _tmp) = boot_with_dispatcher(
        dispatcher.clone(),
        vec![PluginRegistration {
            path: "/api".into(),
            handler_id: "h-respond".into(),
            plugin: "scoped".into(),
        }],
    )
    .await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/totally-unrelated"))
        .body("ignored")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 405);
    let allow = resp
        .headers()
        .get("allow")
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(allow.contains("GET"), "allow header missing GET: {allow}");
    assert!(allow.contains("HEAD"), "allow header missing HEAD: {allow}");
    // Plugin path did not match — dispatcher must not have been called.
    assert_eq!(dispatcher.invocations.load(Ordering::SeqCst), 0);

    server.abort();
}
