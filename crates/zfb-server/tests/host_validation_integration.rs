//! Integration tests for the Host-header allowlist + Origin checks
//! (issue #931 / #919).
//!
//! Boots the real axum server via [`serve_with_listener`] — once on a
//! non-loopback listener (`0.0.0.0:0`, reached through `127.0.0.1` so
//! the test stays machine-local) to exercise the enforced path, and
//! once on the default loopback bind to prove zero behaviour change
//! there. Covers:
//!
//! 1. enforced bind: disallowed `Host` → 403, allowed → 200 (page
//!    cache), built-in `localhost`/`127.0.0.1` always pass;
//! 2. enforced bind + base prefix: the base-prefixed router branch is
//!    protected too (the #931 two-branches warning);
//! 3. cross-origin non-GET to SSR and plugin dispatch → 403 and the
//!    dispatcher is NOT invoked; allowed-origin and absent-origin
//!    non-GET requests still dispatch;
//! 4. static read paths are exempt from the Origin check (cross-origin
//!    GET works; non-GET to static paths still 405s, not 403s);
//! 5. default loopback bind: any Host and any Origin pass through
//!    unchanged.
//!
//! Uses the same ephemeral-port binding pattern as `integration.rs`
//! and the same synthetic dispatchers as `ssr_integration.rs`.

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
    serve_with_listener, DevMiddlewareDispatcher, DevMiddlewareSet, PageCache, PluginDispatchError,
    PluginDispatchOutcome, PluginRegistration, PluginRequest, PluginResponse,
    PluginResponseEncoding, ServeOpts, SsrDispatchError, SsrDispatcher, SsrRequest, SsrResponse,
    SsrRouteRecord, SsrRouteSet, SsrRoutesHandle,
};

/// Counts invocations and returns a fixed canned response — lets the
/// Origin tests assert the dispatcher was (not) reached.
struct CountingSsrDispatcher {
    invocations: AtomicU32,
}

impl CountingSsrDispatcher {
    fn new() -> Self {
        Self {
            invocations: AtomicU32::new(0),
        }
    }

    fn count(&self) -> u32 {
        self.invocations.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl SsrDispatcher for CountingSsrDispatcher {
    async fn dispatch(&self, _request: SsrRequest) -> Result<SsrResponse, SsrDispatchError> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        let mut headers = BTreeMap::new();
        headers.insert("content-type".into(), "text/html; charset=utf-8".into());
        Ok(SsrResponse {
            status: 200,
            headers,
            body: b"<html><body>ssr ok</body></html>".to_vec(),
        })
    }
}

/// Counting plugin dispatcher that always responds.
struct CountingPluginDispatcher {
    invocations: AtomicU32,
}

impl CountingPluginDispatcher {
    fn new() -> Self {
        Self {
            invocations: AtomicU32::new(0),
        }
    }

    fn count(&self) -> u32 {
        self.invocations.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl DevMiddlewareDispatcher for CountingPluginDispatcher {
    async fn dispatch(
        &self,
        _id: &str,
        request: PluginRequest,
    ) -> Result<PluginDispatchOutcome, PluginDispatchError> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
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

struct BootOpts {
    /// Bind `0.0.0.0:0` (non-loopback → host validation enforced)
    /// instead of the default `127.0.0.1:0`.
    bind_any: bool,
    allowed_hosts: Vec<String>,
    ssr_routes: Option<SsrRouteSet>,
    plugin_set: Option<DevMiddlewareSet>,
    base: Option<String>,
}

impl Default for BootOpts {
    fn default() -> Self {
        Self {
            bind_any: true,
            allowed_hosts: Vec::new(),
            ssr_routes: None,
            plugin_set: None,
            base: None,
        }
    }
}

/// Boot the real server; returns the local port (requests go to
/// `127.0.0.1:<port>` regardless of bind, with the Host header set per
/// test) plus the handles the tests need.
async fn boot(
    opts: BootOpts,
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
    std::fs::write(dist_root.join("assets").join("app.css"), "body{}").unwrap();

    let pages = PageCache::new();
    let (tx, _rx) = broadcast::channel::<ReloadEvent>(8);
    let bind: SocketAddr = if opts.bind_any {
        SocketAddr::from(([0, 0, 0, 0], 0))
    } else {
        SocketAddr::from(([127, 0, 0, 1], 0))
    };
    let listener = TcpListener::bind(bind).await.unwrap();
    let addr = listener.local_addr().unwrap();

    let ssr_handle: Option<SsrRoutesHandle> =
        opts.ssr_routes.map(|s| Arc::new(RwLock::new(Some(s))));

    let serve_opts = ServeOpts {
        project_root: tmp.path().to_path_buf(),
        dist_root: dist_root.clone(),
        html_root: dist_root,
        public_root,
        addr,
        pages: pages.clone(),
        broadcast: tx,
        plugins: opts.plugin_set,
        injected_routes: None,
        ssr_routes: ssr_handle,
        base: opts.base,
        trailing_slash: false,
        mode: zfb_server::ServerMode::Dev,
        islands_bundle_url: None,
        css_bundle_url: None,
        allowed_hosts: opts.allowed_hosts,
        bound_host: None,
    };
    let server = tokio::spawn(async move {
        serve_with_listener(serve_opts, listener, std::future::pending::<()>()).await
    });
    tokio::task::yield_now().await;
    (addr, pages, server, tmp)
}

/// All requests physically go to 127.0.0.1 (the 0.0.0.0 listener is
/// reachable there) — the `Host` header is what's under test.
fn local_url(addr: SocketAddr, path: &str) -> String {
    format!("http://127.0.0.1:{}{}", addr.port(), path)
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().build().unwrap()
}

fn ssr_set(pattern: &str, dispatcher: Arc<dyn SsrDispatcher>) -> SsrRouteSet {
    SsrRouteSet::new(
        vec![SsrRouteRecord {
            pattern: pattern.into(),
        }],
        dispatcher,
    )
}

fn plugin_set(path: &str, dispatcher: Arc<CountingPluginDispatcher>) -> DevMiddlewareSet {
    DevMiddlewareSet {
        registrations: Arc::new(vec![PluginRegistration {
            path: path.to_string(),
            handler_id: "h1".to_string(),
            plugin: "test-plugin".to_string(),
        }]),
        dispatcher: dispatcher as Arc<dyn DevMiddlewareDispatcher>,
    }
}

// --- Host-header allowlist (enforced bind) -------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enforced_bind_rejects_disallowed_host_and_accepts_allowed() {
    let (addr, pages, server, _tmp) = boot(BootOpts {
        allowed_hosts: vec!["allowed.test".to_string()],
        ..Default::default()
    })
    .await;
    pages.insert("/", "<html><body>home</body></html>").await;

    // Disallowed Host → 403, Dev-mode body names the knob.
    let resp = client()
        .get(local_url(addr, "/"))
        .header(reqwest::header::HOST, "evil.test")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);
    let body = resp.text().await.unwrap();
    assert!(body.contains("allowedHosts"), "body: {body}");

    // Allowed Host (with a port — must be stripped) → 200.
    let resp = client()
        .get(local_url(addr, "/"))
        .header(reqwest::header::HOST, "allowed.test:8080")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("home"), "body: {body}");

    // Built-in localhost forms and IP-literal hosts (the LAN URLs the
    // bind-all startup banner prints) always pass.
    for host in [
        "localhost:3000",
        "127.0.0.1:9999",
        "192.168.1.9:3000",
        "[::1]:3000",
    ] {
        let resp = client()
            .get(local_url(addr, "/"))
            .header(reqwest::header::HOST, host)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200, "host {host} must be allowed");
    }

    server.abort();
}

/// The #931 two-branches warning: the base-prefixed router construction
/// branch must be protected too, not just the no-base one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enforced_bind_protects_base_prefixed_router_too() {
    let (addr, pages, server, _tmp) = boot(BootOpts {
        allowed_hosts: vec!["allowed.test".to_string()],
        base: Some("/docs/".to_string()),
        ..Default::default()
    })
    .await;
    pages
        .insert("/", "<html><body>docs home</body></html>")
        .await;

    // Disallowed Host under the prefix → 403.
    let resp = client()
        .get(local_url(addr, "/docs/"))
        .header(reqwest::header::HOST, "evil.test")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);

    // The outside-base fallback and the bare `/` redirect are layered too.
    let resp = client()
        .get(local_url(addr, "/elsewhere"))
        .header(reqwest::header::HOST, "evil.test")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);

    // Allowed Host under the prefix → 200.
    let resp = client()
        .get(local_url(addr, "/docs/"))
        .header(reqwest::header::HOST, "allowed.test")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    server.abort();
}

// --- Origin checks on dynamic dispatch (enforced bind) --------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cross_origin_post_to_ssr_route_is_rejected_without_dispatch() {
    let dispatcher = Arc::new(CountingSsrDispatcher::new());
    let set = ssr_set("/dynamic", dispatcher.clone() as Arc<dyn SsrDispatcher>);
    let (addr, _pages, server, _tmp) = boot(BootOpts {
        allowed_hosts: vec!["allowed.test".to_string()],
        ssr_routes: Some(set),
        ..Default::default()
    })
    .await;

    // Cross-origin POST → 403, dispatcher never invoked.
    let resp = client()
        .post(local_url(addr, "/dynamic"))
        .header(reqwest::header::HOST, "allowed.test")
        .header(reqwest::header::ORIGIN, "https://evil.test")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);
    assert_eq!(
        dispatcher.count(),
        0,
        "dispatcher must not run on a rejected request"
    );

    // Allowed-origin POST (port differs — only the host matters) → dispatched.
    let resp = client()
        .post(local_url(addr, "/dynamic"))
        .header(reqwest::header::HOST, "allowed.test")
        .header(reqwest::header::ORIGIN, "http://allowed.test:8080")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(dispatcher.count(), 1);

    // Absent Origin (curl-style client) → allowed through.
    let resp = client()
        .post(local_url(addr, "/dynamic"))
        .header(reqwest::header::HOST, "allowed.test")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(dispatcher.count(), 2);

    // Cross-origin GET is NOT blocked (safe method; Host check covers it).
    let resp = client()
        .get(local_url(addr, "/dynamic"))
        .header(reqwest::header::HOST, "allowed.test")
        .header(reqwest::header::ORIGIN, "https://evil.test")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(dispatcher.count(), 3);

    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cross_origin_post_to_plugin_route_is_rejected_without_dispatch() {
    let dispatcher = Arc::new(CountingPluginDispatcher::new());
    let set = plugin_set("/api/echo", dispatcher.clone());
    let (addr, _pages, server, _tmp) = boot(BootOpts {
        allowed_hosts: vec!["allowed.test".to_string()],
        plugin_set: Some(set),
        ..Default::default()
    })
    .await;

    let resp = client()
        .post(local_url(addr, "/api/echo"))
        .header(reqwest::header::HOST, "allowed.test")
        .header(reqwest::header::ORIGIN, "https://evil.test")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);
    assert_eq!(
        dispatcher.count(),
        0,
        "plugin must not run on a rejected request"
    );

    let resp = client()
        .post(local_url(addr, "/api/echo"))
        .header(reqwest::header::HOST, "allowed.test")
        .header(reqwest::header::ORIGIN, "http://allowed.test")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(dispatcher.count(), 1);

    server.abort();
}

/// Static read paths are exempt from the Origin check by construction:
/// cross-origin GETs to assets succeed, and a cross-origin POST to a
/// static path keeps its historical 405 (method policy) instead of
/// becoming a 403.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn static_paths_are_exempt_from_origin_check() {
    let (addr, _pages, server, _tmp) = boot(BootOpts {
        allowed_hosts: vec!["allowed.test".to_string()],
        ..Default::default()
    })
    .await;

    let resp = client()
        .get(local_url(addr, "/assets/app.css"))
        .header(reqwest::header::HOST, "allowed.test")
        .header(reqwest::header::ORIGIN, "https://evil.test")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    // No plugin/SSR/embed claims /nothing-here — a cross-origin POST
    // falls through to the page-cache method policy (405), proving the
    // Origin gate only guards dynamic dispatch.
    let resp = client()
        .post(local_url(addr, "/nothing-here"))
        .header(reqwest::header::HOST, "allowed.test")
        .header(reqwest::header::ORIGIN, "https://evil.test")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 405);

    server.abort();
}

// --- Default loopback bind: zero behaviour change --------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_bind_accepts_any_host_and_any_origin() {
    let dispatcher = Arc::new(CountingSsrDispatcher::new());
    let set = ssr_set("/dynamic", dispatcher.clone() as Arc<dyn SsrDispatcher>);
    let (addr, pages, server, _tmp) = boot(BootOpts {
        bind_any: false,
        ssr_routes: Some(set),
        ..Default::default()
    })
    .await;
    pages.insert("/", "<html><body>home</body></html>").await;

    // Any Host passes — no allowlist is enforced on the default bind.
    let resp = client()
        .get(local_url(addr, "/"))
        .header(reqwest::header::HOST, "totally.unknown.example")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    // Cross-origin POST to an SSR route still dispatches.
    let resp = client()
        .post(local_url(addr, "/dynamic"))
        .header(reqwest::header::ORIGIN, "https://evil.test")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(dispatcher.count(), 1);

    server.abort();
}
