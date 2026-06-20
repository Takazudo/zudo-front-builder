//! Request-time SSR dispatch for `prerender = false` routes (Gap 1, #367).
//!
//! Build-time SSG covers the default case (a page exports `prerender =
//! true` or omits the flag); the rendered HTML lives in the
//! [`crate::routes::PageCache`] and is served byte-for-byte by the
//! dev router. Pages that opt out of prerendering — `export const
//! prerender = false` — have no precomputed body. In production those
//! routes are served by the Cloudflare adapter's `_worker.js` at
//! request time. Without this module the dev server would 404 them,
//! drift dev away from prod, and break the "what I see in `zfb dev` is
//! what Cloudflare ships" parity claim.
//!
//! ## Wire shape
//!
//! `zfb-server` does NOT depend on `zfb-render` / `zfb-build`. The bin
//! crate (`crates/zfb/src/commands/dev.rs`) owns the embedded V8 host
//! and implements the [`SsrDispatcher`] trait below; the server takes
//! the trait object through [`crate::ServeOpts::ssr_dispatcher`]. This
//! mirrors how [`crate::plugin_middleware::DevMiddlewareDispatcher`]
//! is plumbed through.
//!
//! ## Precedence
//!
//! The dev router's order of attempts is (highest first):
//!
//! 1. plugin dev-middleware (longest-prefix match),
//! 2. **request-time SSR** (this module — matched via [`SsrRouteSet`]),
//! 3. in-memory page cache (SSG output),
//! 4. `<dist>/...` on-disk fallback,
//! 5. `<public>/...` on-disk fallback,
//! 6. dev 404 body.
//!
//! SSR slots in between plugins and the static page cache. A
//! `prerender = false` page never lands in the page cache because the
//! bin crate filters it out of the SSG render set; without this layer
//! the dist fallback may still surface a stale snapshot if a previous
//! `zfb build` left one on disk, which is exactly the dev/prod
//! divergence this sub-task closes.
//!
//! ## Threading model (mirrors `v8_host_adapter.rs`)
//!
//! The implementation of [`SsrDispatcher`] in the bin crate parks the
//! V8 host on a dedicated OS thread with a current-thread tokio
//! runtime and routes calls through a channel (see
//! `crates/zfb/src/v8_host_adapter.rs::ThreadedV8Host`). This module
//! has no direct knowledge of that thread — it only knows that
//! [`SsrDispatcher::dispatch`] is `async` and returns when the dispatch
//! has finished.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use crate::injected_routes::pattern_matches;

/// Shared live handle to the active [`SsrRouteSet`] (issue #807).
///
/// `None` (outer) means "no SSR routes configured for this server."
/// When `Some`, the inner `RwLock` holds the current `SsrRouteSet` and
/// is updated after each dev-tick renderer reload so newly-added or
/// removed `prerender = false` routes are reflected immediately without a
/// dev-server restart.
///
/// Mirrors [`crate::IslandsBundleUrl`] / [`crate::CssBundleUrl`]: one
/// writer (the per-tick reload callback) vs many short-lived readers
/// (every inbound HTTP request that may match an SSR route).
pub type SsrRoutesHandle = Arc<RwLock<Option<SsrRouteSet>>>;

/// One SSR-eligible route, identified by URL pattern.
///
/// The pattern grammar is the same `pages/`-style shape consumed by
/// [`crate::injected_routes::pattern_matches`]: literal segments,
/// `[name]` for a single segment, `[...rest]` for a catch-all. Static
/// pages compile to a literal pattern (`/dynamic`), so the same match
/// helper works for both static and dynamic SSR routes.
#[derive(Debug, Clone)]
pub struct SsrRouteRecord {
    /// URL pattern (e.g. `/dynamic`, `/api/[handler]`). Stored verbatim
    /// from the route-universe entry the bin crate hands over.
    pub pattern: String,
}

/// Bundle of SSR routes + the dispatcher used to render them. Cloned
/// cheaply (the heavy state lives behind `Arc`s).
#[derive(Clone)]
pub struct SsrRouteSet {
    pub records: Arc<Vec<SsrRouteRecord>>,
    pub dispatcher: Arc<dyn SsrDispatcher>,
}

impl SsrRouteSet {
    /// Build a new set from a list of route patterns and a dispatcher.
    pub fn new(records: Vec<SsrRouteRecord>, dispatcher: Arc<dyn SsrDispatcher>) -> Self {
        Self {
            records: Arc::new(records),
            dispatcher,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Return the first registered record whose pattern matches
    /// `url_path`. The check uses the same wildcard grammar as
    /// [`crate::injected_routes::pattern_matches`].
    pub fn find_match(&self, url_path: &str) -> Option<&SsrRouteRecord> {
        self.records
            .iter()
            .find(|rec| pattern_matches(&rec.pattern, url_path))
    }
}

/// One HTTP request shipped from the dev router into the SSR
/// dispatcher. Trimmed shape vs.
/// [`crate::plugin_middleware::PluginRequest`] because the V8 bundle's
/// fetch handler builds its own `Request` object inside the JS
/// runtime; we just need the URL/method/headers/body to construct it.
#[derive(Debug, Clone)]
pub struct SsrRequest {
    pub method: String,
    /// Full path-and-query the dev server received (e.g.
    /// `/dynamic?since=42`). The dispatcher prepends a synthetic
    /// origin before handing it to the V8 host.
    pub url_path: String,
    /// Header map. Values are best-effort UTF-8 — non-UTF-8 values are
    /// dropped on the way in (see
    /// [`crate::plugin_middleware::PluginRequest`] for the same
    /// trade-off).
    pub headers: BTreeMap<String, String>,
    /// Request body. Bytes so binary POST bodies (file uploads, etc.)
    /// round-trip without a UTF-8 coercion.
    pub body: Vec<u8>,
}

/// One HTTP response returned from the SSR dispatcher.
///
/// `status` + `body` + `content_type` map directly onto the dev
/// router's response builder. Additional headers in `headers` are
/// merged after `Content-Type` so a handler can stamp custom cache
/// directives, cookies, etc. — the server never overrides these.
///
/// `headers` is an ordered `Vec<(name, value)>` multimap rather than a
/// map, so multi-valued headers (notably multiple `Set-Cookie`) survive:
/// the dev router `append`s each entry onto the `http::HeaderMap` instead
/// of `insert`ing (which would collapse duplicates to last-value-wins).
#[derive(Debug, Clone, Default)]
pub struct SsrResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Error returned by [`SsrDispatcher::dispatch`] when the underlying
/// render fails. Mirrors the shape of
/// [`crate::plugin_middleware::PluginDispatchError`] so the surrounding
/// error-response plumbing in the dev router is symmetric.
#[derive(Debug, thiserror::Error)]
#[error("ssr dispatch failed for `{url_path}`: {message}")]
pub struct SsrDispatchError {
    pub url_path: String,
    pub message: String,
}

/// Trait implemented by the bin crate's V8-host adapter. The dev router
/// holds a `Arc<dyn SsrDispatcher>` and never sees the underlying V8
/// isolate directly.
///
/// ## `Send + Sync` requirement
///
/// The dev server is multi-threaded (axum on a tokio multi-thread
/// runtime). The dispatcher must be safe to share across handler
/// tasks. The concrete impl in `crates/zfb/src/commands/dev.rs`
/// achieves this by wrapping the `!Send` V8 isolate behind an mpsc
/// channel; the wrapper itself only holds clones of the sender and is
/// trivially `Send + Sync`.
#[async_trait]
pub trait SsrDispatcher: Send + Sync {
    /// Dispatch one request through the embedded V8 host and return
    /// the resulting response.
    async fn dispatch(&self, request: SsrRequest) -> Result<SsrResponse, SsrDispatchError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::Mutex;

    /// Tiny stub dispatcher used by routing-precedence tests. Records
    /// the last request it received and returns a canned response.
    pub struct StubDispatcher {
        pub last: Mutex<Option<SsrRequest>>,
        pub canned: SsrResponse,
    }

    impl StubDispatcher {
        pub fn new(canned: SsrResponse) -> Self {
            Self {
                last: Mutex::new(None),
                canned,
            }
        }
    }

    #[async_trait]
    impl SsrDispatcher for StubDispatcher {
        async fn dispatch(&self, request: SsrRequest) -> Result<SsrResponse, SsrDispatchError> {
            *self.last.lock().await = Some(request);
            Ok(self.canned.clone())
        }
    }

    fn rec(pattern: &str) -> SsrRouteRecord {
        SsrRouteRecord {
            pattern: pattern.into(),
        }
    }

    fn empty_set() -> SsrRouteSet {
        SsrRouteSet::new(
            Vec::new(),
            Arc::new(StubDispatcher::new(SsrResponse::default())),
        )
    }

    #[test]
    fn literal_pattern_matches_exact_url() {
        let s = SsrRouteSet::new(
            vec![rec("/dynamic")],
            Arc::new(StubDispatcher::new(SsrResponse::default())),
        );
        assert!(s.find_match("/dynamic").is_some());
        assert!(s.find_match("/static").is_none());
        assert!(s.find_match("/dynamic/extra").is_none());
    }

    #[test]
    fn bracketed_pattern_matches_single_segment() {
        let s = SsrRouteSet::new(
            vec![rec("/api/[handler]")],
            Arc::new(StubDispatcher::new(SsrResponse::default())),
        );
        assert!(s.find_match("/api/echo").is_some());
        assert!(s.find_match("/api/echo/extra").is_none());
        assert!(s.find_match("/api").is_none());
    }

    #[test]
    fn empty_set_never_matches() {
        let s = empty_set();
        assert!(s.find_match("/anything").is_none());
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
    }

    // ---- scan-order dispatch parity (#814) ---------------------------------
    //
    // `find_match` is first-match over `records`, and the bin crate fills
    // `records` in `zfb_router` scan order (most specific first). These pin
    // that a static-prefixed optional catchall, ordered ahead of a sibling
    // top-level dynamic SSR route by the per-segment sort, wins the URLs the
    // bundled Hono runtime would also send to it — dev/prod parity.

    fn set(patterns: &[&str]) -> SsrRouteSet {
        SsrRouteSet::new(
            patterns.iter().map(|p| rec(p)).collect(),
            Arc::new(StubDispatcher::new(SsrResponse::default())),
        )
    }

    #[test]
    fn optional_catchall_wins_bare_prefix_over_top_level_dynamic() {
        // Scan order for `pages/docs/[[...slug]].tsx` + `pages/[id].tsx`:
        // the static-prefixed catchall sorts first. `/docs` (the
        // zero-segment case of the optional catchall) must dispatch to it,
        // not to `/[id]`.
        let s = set(&["/docs/[[...slug]]", "/[id]"]);
        assert_eq!(
            s.find_match("/docs").map(|r| r.pattern.as_str()),
            Some("/docs/[[...slug]]"),
        );
        // A non-`/docs` single segment still falls through to `/[id]`.
        assert_eq!(
            s.find_match("/other").map(|r| r.pattern.as_str()),
            Some("/[id]"),
        );
    }

    #[test]
    fn optional_catchall_wins_nested_path_over_dynamic_pair() {
        // Scan order for `pages/docs/[[...slug]].tsx` +
        // `pages/[lang]/[slug].tsx`: the static-prefixed catchall sorts
        // first. `/docs/a` must dispatch to it, not to `/[lang]/[slug]`.
        let s = set(&["/docs/[[...slug]]", "/[lang]/[slug]"]);
        assert_eq!(
            s.find_match("/docs/a").map(|r| r.pattern.as_str()),
            Some("/docs/[[...slug]]"),
        );
        // A two-segment URL with a different prefix falls through.
        assert_eq!(
            s.find_match("/en/intro").map(|r| r.pattern.as_str()),
            Some("/[lang]/[slug]"),
        );
    }
}
