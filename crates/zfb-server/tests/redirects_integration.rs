//! Integration tests for the `_redirects` dev-server integration
//! (issue #1546 / epic #1541 Preview Parity).
//!
//! Boots the real axum server with a [`RedirectsHandle`] injected via
//! [`ServeOpts::redirects`] and asserts, at the `zfb-server` router
//! level:
//!
//! 1. **Redirect** — a matched 30x rule redirects with the query
//!    string preserved.
//! 2. **Rewrite** — a matched `200` rule serves the target's content
//!    without changing the client-visible URL/status.
//! 3. **Method gating** — POST bypasses rule matching entirely and
//!    reaches request-time SSR (mirrored by a GET on the same URL,
//!    which DOES redirect, proving the bypass is method-specific).
//! 4. **Non-match** — a request that matches no rule falls through to
//!    the normal page-cache/disk waterfall unchanged.
//! 5. **Live reload** — swapping the shared [`RedirectsHandle`]'s
//!    value (what the dev-server's targeted file watcher does on a
//!    create/edit/delete of `public/_redirects`, see
//!    `crates/zfb/src/commands/dev.rs::spawn_redirects_watch`) changes
//!    the next request's outcome without a server restart. Covers both
//!    "rules reload on edit" and "a file created after boot is picked
//!    up" — from this router's perspective both are the same handle
//!    transition (old ruleset -> new ruleset); the OS-level file-watch
//!    trigger that produces that transition lives in `zfb dev` and is
//!    NOT exercised here (see the wave-4 confirm e2e).
//! 6. **`/_redirects` masking** — the control file itself always 404s,
//!    even when a same-named file sits in `public/` and would
//!    otherwise be servable via the on-disk fallback.
//!
//! Uses the same real-server + ephemeral-port pattern as
//! `render_hook_integration.rs` / `ssr_integration.rs`.

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::sync::Mutex as AsyncMutex;
use zfb_server::livereload::ReloadEvent;
use zfb_server::{
    serve_with_listener, PageCache, Redirects, RedirectsHandle, ServeOpts, ServerMode,
    SsrDispatchError, SsrDispatcher, SsrRequest, SsrResponse, SsrRouteRecord, SsrRouteSet,
    SsrRoutesHandle,
};

// ---------------------------------------------------------------------------
// Test stubs
// ---------------------------------------------------------------------------

/// Records the last request it received and returns a canned response —
/// used to prove a POST reaches SSR instead of being redirected.
struct StubSsrDispatcher {
    last_method: AsyncMutex<Option<String>>,
    canned: SsrResponse,
}

impl StubSsrDispatcher {
    fn new(canned: SsrResponse) -> Self {
        Self {
            last_method: AsyncMutex::new(None),
            canned,
        }
    }
}

#[async_trait]
impl SsrDispatcher for StubSsrDispatcher {
    async fn dispatch(&self, request: SsrRequest) -> Result<SsrResponse, SsrDispatchError> {
        *self.last_method.lock().await = Some(request.method);
        Ok(self.canned.clone())
    }
}

// ---------------------------------------------------------------------------
// Server boot helper
// ---------------------------------------------------------------------------

#[derive(Default)]
struct BootOpts {
    redirects: Option<RedirectsHandle>,
    ssr_routes: Option<SsrRouteSet>,
    /// Extra files to write into `public/` before the server boots
    /// (relative path -> contents), e.g. a raw `_redirects` file to
    /// prove the masking leg wins over the on-disk fallback.
    public_files: Vec<(&'static str, &'static str)>,
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
    for (rel, contents) in &opts.public_files {
        std::fs::write(public_root.join(rel), contents).unwrap();
    }

    let pages = PageCache::new();
    let (tx, _rx) = broadcast::channel::<ReloadEvent>(8);
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();

    let ssr_handle: Option<SsrRoutesHandle> =
        opts.ssr_routes.map(|s| Arc::new(RwLock::new(Some(s))));

    let serve_opts = ServeOpts {
        project_root: tmp.path().to_path_buf(),
        dist_root: dist_root.clone(),
        dev_assets_root: None,
        html_root: dist_root.clone(),
        public_root,
        addr,
        pages: pages.clone(),
        broadcast: tx,
        plugins: None,
        injected_routes: None,
        ssr_routes: ssr_handle,
        base: None,
        trailing_slash: false,
        mode: ServerMode::Dev,
        islands_bundle_url: None,
        css_bundle_url: None,
        allowed_hosts: Vec::new(),
        bound_host: None,
        render_on_request_hook: None,
        redirects: opts.redirects,
    };
    let server = tokio::spawn(async move {
        serve_with_listener(serve_opts, listener, std::future::pending::<()>()).await
    });
    tokio::task::yield_now().await;
    (addr, pages, server, tmp)
}

fn handle_for(rules: &str) -> RedirectsHandle {
    Arc::new(RwLock::new(Redirects::parse(rules)))
}

fn no_follow_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

// ---------------------------------------------------------------------------
// 1. Redirect served with query preserved
// ---------------------------------------------------------------------------

#[tokio::test]
async fn redirect_served_with_query_preserved() {
    let (addr, _pages, _server, _tmp) = boot(BootOpts {
        redirects: Some(handle_for("/old /new 301\n")),
        ..Default::default()
    })
    .await;

    let client = no_follow_client();
    let resp = client
        .get(format!("http://{addr}/old?x=1&y=2"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::MOVED_PERMANENTLY);
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert_eq!(location, "/new?x=1&y=2", "query string must be preserved");
}

// ---------------------------------------------------------------------------
// 2. 200 rewrite serves the target's content
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rewrite_serves_target_content_with_200() {
    let (addr, pages, _server, _tmp) = boot(BootOpts {
        redirects: Some(handle_for("/old /new 200\n")),
        ..Default::default()
    })
    .await;
    pages
        .insert("/new", "<html><body>target</body></html>")
        .await;

    let client = no_follow_client();
    let resp = client
        .get(format!("http://{addr}/old"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("target"),
        "expected the rewrite target's body, got: {body}"
    );
}

// ---------------------------------------------------------------------------
// 3. POST bypasses rules and reaches SSR; GET on the same URL redirects
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_bypasses_rules_and_hits_ssr_while_get_still_redirects() {
    let dispatcher = Arc::new(StubSsrDispatcher::new(SsrResponse {
        status: 200,
        headers: vec![("content-type".into(), "text/plain".into())],
        body: b"ssr response".to_vec(),
    }));
    let ssr_routes = SsrRouteSet::new(
        vec![SsrRouteRecord {
            pattern: "/old".into(),
        }],
        dispatcher.clone(),
    );
    let (addr, _pages, _server, _tmp) = boot(BootOpts {
        redirects: Some(handle_for("/old /new 301\n")),
        ssr_routes: Some(ssr_routes),
        ..Default::default()
    })
    .await;

    let client = no_follow_client();

    // POST must reach the SSR dispatcher, not get redirected.
    let post_resp = client
        .post(format!("http://{addr}/old"))
        .send()
        .await
        .unwrap();
    assert_eq!(post_resp.status(), reqwest::StatusCode::OK);
    let body = post_resp.bytes().await.unwrap();
    assert_eq!(body.as_ref(), b"ssr response");
    assert_eq!(
        dispatcher.last_method.lock().await.as_deref(),
        Some("POST"),
        "SSR dispatcher must have been reached by the POST"
    );

    // GET on the exact same URL must still redirect — the bypass above
    // is method-specific, not a broken rule match.
    let get_resp = client
        .get(format!("http://{addr}/old"))
        .send()
        .await
        .unwrap();
    assert_eq!(get_resp.status(), reqwest::StatusCode::MOVED_PERMANENTLY);
}

// ---------------------------------------------------------------------------
// 4. Non-match falls through the waterfall unchanged
// ---------------------------------------------------------------------------

#[tokio::test]
async fn non_match_falls_through_to_dev_404() {
    let (addr, _pages, _server, _tmp) = boot(BootOpts {
        redirects: Some(handle_for("/old /new 301\n")),
        ..Default::default()
    })
    .await;

    let client = no_follow_client();
    let resp = client
        .get(format!("http://{addr}/totally-unrelated"))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND,
        "a non-matching URL must fall through to the normal dev 404, not be swallowed \
         by the redirects leg"
    );
}

// ---------------------------------------------------------------------------
// 5. Live reload: swapping the handle changes the next request's outcome
//    (covers both "rules reload on edit" and "file created after boot" —
//    see the module docs above for why they collapse to the same case
//    at this layer).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn handle_swap_picks_up_new_rules_without_restart() {
    // Empty at boot — simulates either "no public/_redirects file yet"
    // (the create-after-boot case) or a ruleset about to be replaced by
    // an edit.
    let handle: RedirectsHandle = Arc::new(RwLock::new(Redirects::default()));
    let (addr, _pages, _server, _tmp) = boot(BootOpts {
        redirects: Some(Arc::clone(&handle)),
        ..Default::default()
    })
    .await;

    let client = no_follow_client();

    // Before the "watcher" fires: no rules, so the request falls through.
    let before = client
        .get(format!("http://{addr}/old"))
        .send()
        .await
        .unwrap();
    assert_eq!(before.status(), reqwest::StatusCode::NOT_FOUND);

    // Simulate what `spawn_redirects_watch`'s reload callback does on a
    // create/edit event: re-parse and swap the handle's value in place.
    {
        let mut guard = handle.write().unwrap();
        *guard = Redirects::parse("/old /new 302\n");
    }

    let after = client
        .get(format!("http://{addr}/old"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        after.status(),
        reqwest::StatusCode::FOUND,
        "the swapped-in ruleset must take effect on the very next request, \
         with no dev-server restart"
    );
}

// ---------------------------------------------------------------------------
// 6. `/_redirects` itself always 404s, even when a same-named file in
//    `public/` would otherwise be servable via the disk fallback.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn redirects_file_itself_is_hidden_even_when_present_on_disk() {
    let (addr, _pages, _server, _tmp) = boot(BootOpts {
        // No engine wired at all — the masking leg must be unconditional,
        // not gated on `redirects` being `Some`.
        redirects: None,
        public_files: vec![("_redirects", "/old /new 301\n")],
        ..Default::default()
    })
    .await;

    let client = no_follow_client();
    let resp = client
        .get(format!("http://{addr}/_redirects"))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND,
        "the raw _redirects source file must never be servable at its own URL"
    );
    let body = resp.text().await.unwrap();
    assert!(
        !body.contains("/old /new 301"),
        "the raw rule-file contents must never leak into the response body"
    );
}
