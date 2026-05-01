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
}

#[async_trait]
impl DevMiddlewareDispatcher for CountingDispatcher {
    async fn dispatch(
        &self,
        handler_id: &str,
        request: PluginRequest,
    ) -> Result<PluginDispatchOutcome, PluginDispatchError> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        if handler_id == self.passthrough_handler_id {
            return Ok(PluginDispatchOutcome::Passthrough);
        }
        if handler_id == self.response_handler_id {
            let mut headers = HashMap::new();
            headers.insert("content-type".into(), "application/json".into());
            headers.insert("x-zfb-plugin".into(), "ok".into());
            return Ok(PluginDispatchOutcome::Response(PluginResponse {
                status: 200,
                headers,
                body: format!("{{\"echo\":\"{}\"}}", request.url),
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
    };
    let server = tokio::spawn(async move {
        serve_with_listener(opts, listener, std::future::pending::<()>()).await
    });
    tokio::task::yield_now().await;
    (addr, pages, server, tmp)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dev_middleware_handles_registered_path() {
    let dispatcher = Arc::new(CountingDispatcher {
        passthrough_handler_id: "h-pass".into(),
        response_handler_id: "h-respond".into(),
        invocations: AtomicU32::new(0),
    });
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
    let dispatcher = Arc::new(CountingDispatcher {
        passthrough_handler_id: "h-pass".into(),
        response_handler_id: "h-respond".into(),
        invocations: AtomicU32::new(0),
    });
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
    let dispatcher = Arc::new(CountingDispatcher {
        passthrough_handler_id: "h-pass".into(),
        response_handler_id: "h-respond".into(),
        invocations: AtomicU32::new(0),
    });
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
