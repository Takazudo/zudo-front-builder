//! Precedence smoke test for the embed-API HTTP handler layer
//! (sub-task 3.1b / #372).
//!
//! Asserts that when a Rust handler registered via
//! [`zfb_server::ServerBuilder::with_ssr_handler`] AND a JS
//! runtime-rendered route both claim the same URL pattern, the **Rust
//! handler wins** — the JS dispatcher must not be consulted. This is
//! the load-bearing precedence rule documented on the
//! `embed-as-library` guide.
//!
//! Two cases:
//!
//! 1. A custom handler at `/dynamic` short-circuits an SSR route that
//!    also matches `/dynamic`; the SSR dispatcher's invocation counter
//!    must stay at zero.
//! 2. A handler with a captured `:id` param sees the captured value.
//!
//! The server is booted by composing an [`zfb_server::AppState`]
//! directly — the public `Server::builder` API doesn't yet accept the
//! SSR route set that comes from the bin crate, so this test stitches
//! both layers together via a small local helper. The unit tests in
//! `crates/zfb-server/src/embed_handlers.rs` cover the matcher;
//! `embed_lifecycle_smoke.rs` covers the builder lifecycle.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use zfb_server::embed_handlers::erase_handler_for_test;
use zfb_server::livereload::ReloadEvent;
use zfb_server::{
    build_router, AppState, DevMiddlewareDispatcher, DevMiddlewareSet, EmbedHandler,
    EmbedHandlerSet, PageCache, PluginDispatchError, PluginDispatchOutcome, PluginRegistration,
    PluginRequest, PluginResponse, PluginResponseEncoding, RouteParams, SsrDispatchError,
    SsrDispatcher, SsrRequest, SsrResponse, SsrRouteRecord, SsrRouteSet, SsrRoutesHandle,
};

/// SSR dispatcher that counts invocations and returns a canned body.
/// Used to assert the layer was bypassed.
struct CountingSsrDispatcher {
    invocations: AtomicU32,
    canned: SsrResponse,
}

impl CountingSsrDispatcher {
    fn new(canned: SsrResponse) -> Self {
        Self {
            invocations: AtomicU32::new(0),
            canned,
        }
    }
}

#[async_trait]
impl SsrDispatcher for CountingSsrDispatcher {
    async fn dispatch(&self, _request: SsrRequest) -> Result<SsrResponse, SsrDispatchError> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        Ok(self.canned.clone())
    }
}

/// Always-responding plugin dispatcher used by the plugin>rust-handler
/// precedence test.
struct AlwaysRespondingPluginDispatcher;

#[async_trait]
impl DevMiddlewareDispatcher for AlwaysRespondingPluginDispatcher {
    async fn dispatch(
        &self,
        _id: &str,
        request: PluginRequest,
    ) -> Result<PluginDispatchOutcome, PluginDispatchError> {
        let headers = vec![("content-type".into(), "text/plain; charset=utf-8".into())];
        Ok(PluginDispatchOutcome::Response(PluginResponse {
            status: 200,
            headers,
            body: format!("plugin handled {}", request.url),
            body_encoding: PluginResponseEncoding::Utf8,
        }))
    }
}

/// Boot a server with `ssr_routes`, `embed_handlers`, and optionally a
/// plugin set populated inside the [`AppState`] directly. The public
/// builder doesn't accept an SSR set yet, so this test threads each
/// layer in manually — the resulting router is the exact same shape the
/// embed builder constructs at runtime.
async fn boot_with_plugins(
    handlers: EmbedHandlerSet,
    ssr: SsrRouteSet,
    plugins: Option<DevMiddlewareSet>,
) -> (
    SocketAddr,
    tokio::task::JoinHandle<anyhow::Result<()>>,
    tempfile::TempDir,
) {
    boot_with_plugins_and_host_validation(
        handlers,
        ssr,
        plugins,
        zfb_server::HostValidation::disabled(),
    )
    .await
}

/// Same as [`boot_with_plugins`] but threads an explicit
/// [`zfb_server::HostValidation`] instead of always disabling it —
/// needed to cover the Host/Origin allowlist reaching the embed
/// handler surface (issue #1770), since `ServeOpts` (used by
/// `host_validation_integration.rs`) always builds `embed_handlers:
/// None` and can never exercise this dispatch path.
async fn boot_with_plugins_and_host_validation(
    handlers: EmbedHandlerSet,
    ssr: SsrRouteSet,
    plugins: Option<DevMiddlewareSet>,
    host_validation: zfb_server::HostValidation,
) -> (
    SocketAddr,
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

    let ssr_handle: SsrRoutesHandle = Arc::new(RwLock::new(Some(ssr)));
    let state = AppState {
        mode: zfb_server::ServerMode::Dev,
        pages,
        broadcast: tx,
        plugins,
        injected_routes: None,
        ssr_routes: Some(ssr_handle),
        embed_handlers: Some(handlers),
        dist_root: dist_root.clone(),
        html_root: dist_root,
        public_root,
        base_prefix: None,
        trailing_slash: false,
        islands_bundle_url: None,
        css_bundle_url: None,
        host_validation,
        render_on_request_hook: None,
        redirects: None,
        // Test uses temp dirs — canonical roots not precomputed.
        canonical_html_root: None,
        canonical_dist_root: None,
        dev_assets_root: None,
        canonical_public_root: None,
    };
    let router = build_router(state);

    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .map_err(anyhow::Error::from)
    });
    tokio::task::yield_now().await;
    (addr, server, tmp)
}

/// Convenience wrapper for tests that don't need a plugin set.
async fn boot(
    handlers: EmbedHandlerSet,
    ssr: SsrRouteSet,
) -> (
    SocketAddr,
    tokio::task::JoinHandle<anyhow::Result<()>>,
    tempfile::TempDir,
) {
    boot_with_plugins(handlers, ssr, None).await
}

#[tokio::test]
async fn rust_handler_wins_over_ssr_for_overlapping_pattern() {
    // SSR dispatcher claims `/dynamic` and would respond 200 with the
    // body `ssr-body` if it were ever consulted.
    let canned = SsrResponse {
        status: 200,
        headers: Vec::new(),
        body: b"ssr-body".to_vec(),
    };
    let ssr_dispatcher = Arc::new(CountingSsrDispatcher::new(canned));
    let ssr_set = SsrRouteSet::new(
        vec![SsrRouteRecord {
            pattern: "/dynamic".into(),
        }],
        ssr_dispatcher.clone() as Arc<dyn SsrDispatcher>,
    );

    let handlers = EmbedHandlerSet::new(vec![EmbedHandler {
        pattern: "/dynamic".into(),
        handler: erase_handler_for_test(|req: Request<Body>, _params: RouteParams| async move {
            (StatusCode::OK, format!("rust-handler|{}", req.uri().path()))
        }),
    }]);

    let (addr, server, _tmp) = boot(handlers, ssr_set).await;

    let client = reqwest::Client::builder().build().unwrap();
    let resp = client
        .get(format!("http://{addr}/dynamic"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("rust-handler|/dynamic"),
        "expected Rust handler to win; got {body:?}"
    );
    assert_eq!(
        ssr_dispatcher.invocations.load(Ordering::SeqCst),
        0,
        "SSR dispatcher must be skipped when a Rust handler claims the URL"
    );

    server.abort();
}

#[tokio::test]
async fn rust_handler_captures_path_params() {
    let canned = SsrResponse {
        status: 200,
        headers: Vec::new(),
        body: b"ssr".to_vec(),
    };
    let ssr_dispatcher = Arc::new(CountingSsrDispatcher::new(canned));
    let ssr_set = SsrRouteSet::new(
        vec![SsrRouteRecord {
            pattern: "/users/[id]".into(),
        }],
        ssr_dispatcher.clone() as Arc<dyn SsrDispatcher>,
    );

    let handlers = EmbedHandlerSet::new(vec![EmbedHandler {
        pattern: "/users/:id".into(),
        handler: erase_handler_for_test(|_req, params: RouteParams| async move {
            (
                StatusCode::OK,
                format!("id={}", params.get("id").unwrap_or("missing")),
            )
        }),
    }]);

    let (addr, server, _tmp) = boot(handlers, ssr_set).await;

    let client = reqwest::Client::builder().build().unwrap();
    let resp = client
        .get(format!("http://{addr}/users/42"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("id=42"), "got body {body:?}");
    assert_eq!(
        ssr_dispatcher.invocations.load(Ordering::SeqCst),
        0,
        "SSR must be skipped when the Rust handler matches"
    );

    server.abort();
}

/// Issue #1770: the embed handler dispatch surface (`AppState`'s
/// `embed_handlers`) is gated by the same Origin check as SSR/plugin
/// routes, and an IP-literal `Origin` does NOT ride the Host-only
/// IP-literal always-allow rule. Embed callers have no `allowedHosts`
/// config surface (see `serve_with_listener` in `embed.rs`), so their
/// documented remedy is binding to a concrete LAN IP instead of
/// `0.0.0.0` — that bound IP is seeded as an `Exact` rule by
/// `HostValidation::for_bind` and its Origin is allowed through that
/// rule, while an unrelated IP is rejected.
#[tokio::test]
async fn embed_handler_origin_gate_rejects_unrelated_ip_but_allows_bound_ip() {
    let canned = SsrResponse {
        status: 200,
        headers: Vec::new(),
        body: b"ssr".to_vec(),
    };
    let ssr_dispatcher = Arc::new(CountingSsrDispatcher::new(canned));
    let ssr_set = SsrRouteSet::new(Vec::new(), ssr_dispatcher.clone() as Arc<dyn SsrDispatcher>);

    let rust_handler_calls = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&rust_handler_calls);
    let handlers = EmbedHandlerSet::new(vec![EmbedHandler {
        pattern: "/api/echo".into(),
        handler: erase_handler_for_test(move |_req, _params: RouteParams| {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                (StatusCode::OK, "rust-handler-body".to_string())
            }
        }),
    }]);

    // Simulates an embed server documented as bound to a concrete LAN
    // IP (the #1770 remedy) rather than `0.0.0.0`; `bind_host_validation`
    // is independent of the real test listener (which stays on
    // loopback), mirroring the `local_url`-vs-`Host` pattern used by
    // `host_validation_integration.rs`.
    let bind_ip = std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 5));
    let host_validation =
        zfb_server::HostValidation::for_bind(bind_ip, None, &[], zfb_server::ServerMode::Embed);

    let (addr, server, _tmp) =
        boot_with_plugins_and_host_validation(handlers, ssr_set, None, host_validation).await;

    let client = reqwest::Client::builder().build().unwrap();

    // Unrelated IP-literal Origin → 403, Rust handler never invoked.
    for origin in ["http://192.168.1.9:3000", "http://[2001:db8::7]:3000"] {
        let resp = client
            .post(format!("http://{addr}/api/echo"))
            .header(reqwest::header::HOST, "192.168.1.5")
            .header(reqwest::header::ORIGIN, origin)
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status().as_u16(),
            403,
            "origin {origin} must be rejected"
        );
    }
    assert_eq!(
        rust_handler_calls.load(Ordering::SeqCst),
        0,
        "embed handler must not run for an unrelated IP-literal origin"
    );

    // The bound IP's own Origin is allowed through its explicit `Exact`
    // rule (not the removed IP-literal short-circuit).
    let resp = client
        .post(format!("http://{addr}/api/echo"))
        .header(reqwest::header::HOST, "192.168.1.5")
        .header(reqwest::header::ORIGIN, "http://192.168.1.5:8080")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(rust_handler_calls.load(Ordering::SeqCst), 1);

    // Contrast: the Host header still allows ANY IP literal (the
    // exemption stays Host-only) — an unrelated IP Host with a matching
    // allowed Origin still dispatches.
    let resp = client
        .post(format!("http://{addr}/api/echo"))
        .header(reqwest::header::HOST, "192.168.1.9")
        .header(reqwest::header::ORIGIN, "http://192.168.1.5:8080")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(rust_handler_calls.load(Ordering::SeqCst), 2);

    server.abort();
}

#[tokio::test]
async fn plugin_dev_middleware_wins_over_rust_handler() {
    // Same path is claimed by a plugin and a Rust handler; the plugin
    // must win per the documented precedence (plugin > rust > SSR).
    let canned_ssr = SsrResponse {
        status: 200,
        headers: Vec::new(),
        body: b"ssr".to_vec(),
    };
    let ssr_dispatcher = Arc::new(CountingSsrDispatcher::new(canned_ssr));
    let ssr_set = SsrRouteSet::new(Vec::new(), ssr_dispatcher.clone() as Arc<dyn SsrDispatcher>);

    let rust_handler_calls = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&rust_handler_calls);
    let handlers = EmbedHandlerSet::new(vec![EmbedHandler {
        pattern: "/api/echo".into(),
        handler: erase_handler_for_test(move |_req, _params: RouteParams| {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                (StatusCode::OK, "rust-handler-body".to_string())
            }
        }),
    }]);

    let plugin_set = DevMiddlewareSet {
        registrations: Arc::new(vec![PluginRegistration {
            path: "/api/echo".into(),
            handler_id: "h1".into(),
            plugin: "test".into(),
        }]),
        dispatcher: Arc::new(AlwaysRespondingPluginDispatcher) as Arc<dyn DevMiddlewareDispatcher>,
    };

    let (addr, server, _tmp) = boot_with_plugins(handlers, ssr_set, Some(plugin_set)).await;

    let client = reqwest::Client::builder().build().unwrap();
    let resp = client
        .get(format!("http://{addr}/api/echo"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("plugin handled"),
        "plugin must take precedence over Rust handler; got {body:?}"
    );
    assert_eq!(
        rust_handler_calls.load(Ordering::SeqCst),
        0,
        "Rust handler must not be invoked when a plugin claims the URL"
    );

    server.abort();
}
