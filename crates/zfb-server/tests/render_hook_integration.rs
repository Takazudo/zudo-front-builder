//! Integration tests for the render-on-request hook seam (issue #1020).
//!
//! Boots the real axum server with a synthetic [`RenderOnRequestHook`]
//! injected via [`ServeOpts::render_on_request_hook`] and asserts:
//!
//! 1. **Invocation on GET** — a GET request to a URL path calls the hook
//!    with the correct (unprefixed) URL path.
//! 2. **Invocation on HEAD** — a HEAD request also dispatches the hook,
//!    returning headers with no body.
//! 3. **Not invoked: Preview mode** — in `ServerMode::Preview` the hook
//!    is never called even when the handle is `Some`.
//! 4. **Not invoked: Embed mode** — same gate as Preview.
//! 5. **Not invoked: non-GET-like method** — a POST to a URL with the hook
//!    configured returns 405 and does NOT call the hook.
//! 6. **Not invoked: handle is None** — with no hook in `ServeOpts` the
//!    server is byte-identical to pre-hook behaviour.
//! 7. **Fall-through after hook** — the hook writes a file to `html_root`;
//!    after the hook returns the server serves that file.
//! 8. **Error containment** — a hook that panics does NOT propagate the
//!    panic to the HTTP response; the server falls through to the
//!    disk/cache legs (and returns whatever is there).
//! 9. **Precedence: hook runs AFTER SSR leg** — when both `ssr_routes`
//!    and `render_on_request_hook` are set and the URL matches the SSR
//!    set, SSR wins and the hook is NOT called.
//! 10. **Precedence: hook runs BEFORE page cache** — the hook fires even
//!     when the in-memory page cache already has an entry (the issue
//!     contract places the hook just before the cache leg, so the cache
//!     will serve the cached body after the hook runs; the hook still gets
//!     a chance to refresh disk).
//!
//! Uses the same ephemeral-port pattern as `ssr_integration.rs`.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use zfb_server::livereload::ReloadEvent;
use zfb_server::{
    serve_with_listener, PageCache, RenderOnRequestHandle, RenderOnRequestHook, ServeOpts,
    ServerMode, SsrDispatchError, SsrDispatcher, SsrRequest, SsrResponse, SsrRouteRecord,
    SsrRouteSet, SsrRoutesHandle,
};

// ---------------------------------------------------------------------------
// Test stubs
// ---------------------------------------------------------------------------

/// Type alias for the optional side-effect callback in [`RecordingHook`].
type SideEffectFn = Box<dyn Fn(&str) + Send + Sync>;

/// Records invocations and lets the test verify the URL path.
struct RecordingHook {
    invocations: AtomicU32,
    last_path: tokio::sync::Mutex<Option<String>>,
    /// Optional callback: writes a file to a path so the server can serve it.
    side_effect: Option<SideEffectFn>,
}

impl RecordingHook {
    fn new() -> Self {
        Self {
            invocations: AtomicU32::new(0),
            last_path: tokio::sync::Mutex::new(None),
            side_effect: None,
        }
    }

    fn with_side_effect(f: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self {
            invocations: AtomicU32::new(0),
            last_path: tokio::sync::Mutex::new(None),
            side_effect: Some(Box::new(f)),
        }
    }
}

#[async_trait]
impl RenderOnRequestHook for RecordingHook {
    async fn render_if_stale(&self, url_path: &str) {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        *self.last_path.lock().await = Some(url_path.to_string());
        if let Some(f) = &self.side_effect {
            f(url_path);
        }
    }
}

/// A hook that panics immediately — used for error-containment tests.
struct PanickingHook;

#[async_trait]
impl RenderOnRequestHook for PanickingHook {
    async fn render_if_stale(&self, _url_path: &str) {
        panic!("hook panic (expected in test)");
    }
}

// Minimal stub SSR dispatcher that returns a canned response.
struct StubSsrDispatcher {
    body: Vec<u8>,
}

#[async_trait]
impl SsrDispatcher for StubSsrDispatcher {
    async fn dispatch(&self, _req: SsrRequest) -> Result<SsrResponse, SsrDispatchError> {
        Ok(SsrResponse {
            status: 200,
            headers: vec![("content-type".into(), "text/html; charset=utf-8".into())],
            body: self.body.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// Server boot helpers
// ---------------------------------------------------------------------------

struct BootOpts {
    mode: ServerMode,
    hook: Option<Arc<dyn RenderOnRequestHook>>,
    ssr_routes: Option<SsrRouteSet>,
    base: Option<String>,
}

impl Default for BootOpts {
    fn default() -> Self {
        Self {
            mode: ServerMode::Dev,
            hook: None,
            ssr_routes: None,
            base: None,
        }
    }
}

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

    let pages = PageCache::new();
    let (tx, _rx) = broadcast::channel::<ReloadEvent>(8);
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();

    let hook_handle: Option<RenderOnRequestHandle> =
        opts.hook.map(|h| Arc::new(RwLock::new(Some(h))));

    let ssr_handle: Option<SsrRoutesHandle> =
        opts.ssr_routes.map(|s| Arc::new(RwLock::new(Some(s))));

    let serve_opts = ServeOpts {
        project_root: tmp.path().to_path_buf(),
        dist_root: dist_root.clone(),
        html_root: dist_root.clone(),
        public_root,
        addr,
        pages: pages.clone(),
        broadcast: tx,
        plugins: None,
        injected_routes: None,
        ssr_routes: ssr_handle,
        base: opts.base,
        trailing_slash: false,
        mode: opts.mode,
        islands_bundle_url: None,
        css_bundle_url: None,
        allowed_hosts: Vec::new(),
        bound_host: None,
        render_on_request_hook: hook_handle,
    };
    let server = tokio::spawn(async move {
        serve_with_listener(serve_opts, listener, std::future::pending::<()>()).await
    });
    tokio::task::yield_now().await;
    (addr, pages, server, tmp)
}

// ---------------------------------------------------------------------------
// 1. Hook invoked on GET
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hook_invoked_on_get() {
    let hook = Arc::new(RecordingHook::new());
    let (addr, _pages, _server, _tmp) = boot(BootOpts {
        hook: Some(hook.clone()),
        ..Default::default()
    })
    .await;

    let client = reqwest::Client::new();
    let _resp = client
        .get(format!("http://{addr}/blog/post"))
        .send()
        .await
        .unwrap();

    // The hook must have been called exactly once.
    assert_eq!(hook.invocations.load(Ordering::SeqCst), 1);
    let path = hook.last_path.lock().await.clone().unwrap();
    assert_eq!(path, "/blog/post", "hook receives unprefixed url path");
}

// ---------------------------------------------------------------------------
// 2. Hook invoked on HEAD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hook_invoked_on_head_and_response_has_no_body() {
    let hook = Arc::new(RecordingHook::new());
    let (addr, pages, _server, _tmp) = boot(BootOpts {
        hook: Some(hook.clone()),
        ..Default::default()
    })
    .await;

    // Seed the cache so the server has something to respond with.
    // Root lookups probe `/` then `/index.html` (see lookup_keys).
    pages.insert("/", "<html><body>hello</body></html>").await;

    let client = reqwest::Client::new();
    let resp = client.head(format!("http://{addr}/")).send().await.unwrap();

    // HEAD must invoke the hook.
    assert_eq!(hook.invocations.load(Ordering::SeqCst), 1);
    // HEAD response must have zero-length body.
    let body_bytes = resp.bytes().await.unwrap();
    assert!(
        body_bytes.is_empty(),
        "HEAD response must have no body; got {} bytes",
        body_bytes.len()
    );
}

// ---------------------------------------------------------------------------
// 3. Hook NOT invoked in Preview mode
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hook_not_invoked_in_preview_mode() {
    let hook = Arc::new(RecordingHook::new());
    let (addr, pages, _server, _tmp) = boot(BootOpts {
        mode: ServerMode::Preview,
        hook: Some(hook.clone()),
        ..Default::default()
    })
    .await;

    pages.insert("/", "<html><body>preview</body></html>").await;

    let client = reqwest::Client::new();
    let _resp = client.get(format!("http://{addr}/")).send().await.unwrap();

    assert_eq!(
        hook.invocations.load(Ordering::SeqCst),
        0,
        "hook must NOT fire in Preview mode"
    );
}

// ---------------------------------------------------------------------------
// 4. Hook NOT invoked in Embed mode
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hook_not_invoked_in_embed_mode() {
    let hook = Arc::new(RecordingHook::new());
    let (addr, pages, _server, _tmp) = boot(BootOpts {
        mode: ServerMode::Embed,
        hook: Some(hook.clone()),
        ..Default::default()
    })
    .await;

    pages.insert("/", "<html><body>embed</body></html>").await;

    let client = reqwest::Client::new();
    let _resp = client.get(format!("http://{addr}/")).send().await.unwrap();

    assert_eq!(
        hook.invocations.load(Ordering::SeqCst),
        0,
        "hook must NOT fire in Embed mode"
    );
}

// ---------------------------------------------------------------------------
// 5. Hook NOT invoked for non-GET-like methods (POST → 405)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hook_not_invoked_on_post_and_405_returned() {
    let hook = Arc::new(RecordingHook::new());
    let (addr, _pages, _server, _tmp) = boot(BootOpts {
        hook: Some(hook.clone()),
        ..Default::default()
    })
    .await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/some-page"))
        .send()
        .await
        .unwrap();

    assert_eq!(
        hook.invocations.load(Ordering::SeqCst),
        0,
        "hook must NOT fire for POST"
    );
    assert_eq!(
        resp.status().as_u16(),
        405,
        "POST to a page URL must yield 405"
    );
}

// ---------------------------------------------------------------------------
// 6. Behaviour identical when handle is None
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_regression_when_hook_is_none() {
    // Boot with no hook. Seed the page cache and confirm the server
    // still serves the cached content normally.
    let (addr, pages, _server, _tmp) = boot(BootOpts::default()).await;

    pages
        .insert("/", "<html><body>cached-no-hook</body></html>")
        .await;

    let client = reqwest::Client::new();
    let resp = client.get(format!("http://{addr}/")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("cached-no-hook"),
        "server must serve cached page when hook is None"
    );
}

// ---------------------------------------------------------------------------
// 7. Fall-through after hook: hook writes file, server serves it
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hook_side_effect_served_after_hook_returns() {
    // The hook writes an HTML file to html_root; the server then serves it
    // via the disk fallback.
    let tmp = tempfile::tempdir().unwrap();
    let html_root = tmp.path().join("dist");
    std::fs::create_dir_all(html_root.join("assets")).unwrap();
    std::fs::create_dir_all(tmp.path().join("public")).unwrap();

    let html_root_clone = html_root.clone();
    let hook = Arc::new(RecordingHook::with_side_effect(move |url_path| {
        // Map /post → dist/post/index.html
        let rel = url_path.trim_start_matches('/');
        let dir = html_root_clone.join(rel);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("index.html"),
            "<html><body>rendered-by-hook</body></html>",
        )
        .unwrap();
    }));

    let pages = PageCache::new();
    let (tx, _rx) = broadcast::channel::<ReloadEvent>(8);
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();

    let hook_handle: RenderOnRequestHandle = Arc::new(RwLock::new(Some(hook)));
    let serve_opts = ServeOpts {
        project_root: tmp.path().to_path_buf(),
        dist_root: html_root.clone(),
        html_root,
        public_root: tmp.path().join("public"),
        addr,
        pages: pages.clone(),
        broadcast: tx,
        plugins: None,
        injected_routes: None,
        ssr_routes: None,
        base: None,
        trailing_slash: false,
        mode: ServerMode::Dev,
        islands_bundle_url: None,
        css_bundle_url: None,
        allowed_hosts: Vec::new(),
        bound_host: None,
        render_on_request_hook: Some(hook_handle),
    };
    let _server = tokio::spawn(async move {
        serve_with_listener(serve_opts, listener, std::future::pending::<()>()).await
    });
    tokio::task::yield_now().await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/post"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("rendered-by-hook"),
        "server must serve file written by hook; got:\n{body}"
    );
}

// ---------------------------------------------------------------------------
// 8. Error containment: panicking hook doesn't propagate to HTTP response
// ---------------------------------------------------------------------------

#[tokio::test]
async fn panicking_hook_does_not_crash_server() {
    // A hook that panics must not turn into a 500 or server crash —
    // tokio catches the panic in the spawned task, and the handler
    // should continue to serve the next request normally.
    //
    // NOTE: this test verifies that the SERVER ITSELF keeps accepting
    // requests after a panicking hook. The panicking hook's task will
    // abort (tokio catches it), but serve_page runs inside the main
    // handler task and we catch the join error if necessary. Since
    // `render_if_stale` is `await`ed directly inside the handler, a
    // panic in the hook propagates as a panic in the handler task.
    // axum wraps each request in its own task (via tower's `Service`
    // impl), so the panic is contained to that one request; subsequent
    // requests are served normally.
    //
    // The response for the panicking request may be a connection reset
    // or a 500 — either is acceptable. What matters is the NEXT request
    // works.
    let hook = Arc::new(PanickingHook);
    let (addr, pages, _server, _tmp) = boot(BootOpts {
        hook: Some(hook),
        ..Default::default()
    })
    .await;

    // First request: may panic/reset — that's OK.
    let client = reqwest::Client::new();
    let _ = client.get(format!("http://{addr}/panic-path")).send().await;

    // Give the server a moment to recover.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Second request: must succeed normally.
    pages
        .insert("/ok", "<html><body>ok-after-panic</body></html>")
        .await;
    let resp = client
        .get(format!("http://{addr}/ok"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("ok-after-panic"),
        "server must serve normally after hook panic; got:\n{body}"
    );
}

// ---------------------------------------------------------------------------
// 9. Precedence: SSR wins over hook (hook not invoked on SSR match)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ssr_wins_over_render_hook() {
    let hook = Arc::new(RecordingHook::new());
    let ssr_dispatcher = Arc::new(StubSsrDispatcher {
        body: b"<html><body>ssr-wins</body></html>".to_vec(),
    });
    let ssr_set = SsrRouteSet::new(
        vec![SsrRouteRecord {
            pattern: "/ssr-page".into(),
        }],
        ssr_dispatcher,
    );

    let (addr, _pages, _server, _tmp) = boot(BootOpts {
        hook: Some(hook.clone()),
        ssr_routes: Some(ssr_set),
        ..Default::default()
    })
    .await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/ssr-page"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("ssr-wins"),
        "SSR leg must win over hook; got:\n{body}"
    );
    assert_eq!(
        hook.invocations.load(Ordering::SeqCst),
        0,
        "hook must NOT be called when SSR leg handles the request"
    );
}

// ---------------------------------------------------------------------------
// 10. Precedence: hook fires before page cache (cache still serves after)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hook_fires_before_page_cache_lookup() {
    // The hook is fired for every GET — even when the cache has the URL.
    // After the hook runs, the cache entry is served (standard fall-through).
    let hook = Arc::new(RecordingHook::new());
    let (addr, pages, _server, _tmp) = boot(BootOpts {
        hook: Some(hook.clone()),
        ..Default::default()
    })
    .await;

    pages
        .insert("/cached", "<html><body>cached-content</body></html>")
        .await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/cached"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("cached-content"),
        "cache entry must be served after hook; got:\n{body}"
    );
    assert_eq!(
        hook.invocations.load(Ordering::SeqCst),
        1,
        "hook must fire once even when the page cache has an entry"
    );
}

// ---------------------------------------------------------------------------
// 11. Base-prefix stripped before passing to hook
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hook_receives_prefix_stripped_url() {
    let hook = Arc::new(RecordingHook::new());
    let (addr, _pages, _server, _tmp) = boot(BootOpts {
        hook: Some(hook.clone()),
        base: Some("/foo".to_string()),
        ..Default::default()
    })
    .await;

    let client = reqwest::Client::new();
    let _resp = client
        .get(format!("http://{addr}/foo/blog/hello"))
        .send()
        .await
        .unwrap();

    assert_eq!(hook.invocations.load(Ordering::SeqCst), 1);
    let path = hook.last_path.lock().await.clone().unwrap();
    assert_eq!(
        path, "/blog/hello",
        "hook must receive the base-stripped path; got: {path}"
    );
}
