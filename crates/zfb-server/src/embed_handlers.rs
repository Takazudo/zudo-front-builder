//! Route-pattern dispatch table for embed-API HTTP handlers (sub-task
//! 3.1b / issue #372).
//!
//! Embedders register a handler with
//! [`crate::ServerBuilder::with_ssr_handler`] for a URL pattern, and the
//! dev router consults the resulting [`EmbedHandlerSet`] inside
//! [`crate::routes::serve_page`]. The handler signature is intentionally
//! domain-agnostic — see [`EmbedHandler`] — so the same primitive can
//! back any kind of request-time response (markdown, key-value lookups,
//! IPC bridges, … the server doesn't care).
//!
//! ## Pattern grammar
//!
//! Patterns are leading-slash strings. Each segment is one of:
//!
//! - a **literal** (`foo`): matches the same byte sequence exactly,
//! - a **single-segment parameter** (`:name`): matches any non-empty
//!   segment and captures it under `name`,
//! - a **wildcard tail** (`*name`): matches the remaining one or more
//!   segments concatenated with `/`, captured under `name`. Must be the
//!   final segment of the pattern.
//!
//! Examples:
//!
//! - `/users/:id` matches `/users/42` with `{id: "42"}`.
//! - `/files/*rest` matches `/files/a/b.txt` with `{rest: "a/b.txt"}`.
//! - `/health` matches `/health` exactly.
//!
//! Empty captures are rejected (no `//` collapses, no zero-length
//! params) — the matcher returns `None` rather than handing the handler
//! a value the caller could not have written intentionally.
//!
//! ## Grammar divergence (TODO — deep-review PR #376)
//!
//! `zfb_server` exposes TWO public route-pattern grammars by historical
//! accident:
//!
//! - **embed handlers** (this module / `with_ssr_handler`): axum/Hono-
//!   style `:name` / `*tail`.
//! - **SSR route set** ([`crate::ssr::SsrRouteSet`], via
//!   [`crate::injected_routes::pattern_matches`]): `pages/`-style
//!   `[name]` / `[...tail]`.
//!
//! The dev orchestrator translates between the two at the seam (see
//! `crates/zfb/src/commands/dev.rs::colon_template_to_bracket`), but
//! unifying the two grammars at the public API surface is a larger
//! design decision (see the deep-review structure finding on PR #376)
//! and intentionally deferred to a follow-up.
//!
//! ## Storage shape
//!
//! Handlers are erased to a single function-pointer type
//! ([`EmbedHandlerFn`]) so the [`EmbedHandlerSet`] is `Clone` and can
//! live in [`crate::routes::AppState`]. Each registration owns its
//! `Arc<dyn Fn>`; the set is consulted by a linear scan with
//! first-registration-wins semantics, matching the
//! [`crate::injected_routes::InjectedRouteSet`] convention.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use axum::response::{IntoResponse, Response};

/// Captured parameters from a [`pattern_matches`] hit, keyed by the
/// names declared in the pattern.
///
/// Handler authors read params with [`Self::get`]; both single-segment
/// (`:name`) and wildcard-tail (`*name`) captures are stored under the
/// same map (the wildcard value just contains slashes).
///
/// `RouteParams` is intentionally a `BTreeMap` so the iteration order is
/// stable across runs — useful when a handler logs or hashes the
/// captured params for diagnostics.
#[derive(Debug, Clone, Default)]
pub struct RouteParams {
    inner: BTreeMap<String, String>,
}

impl RouteParams {
    /// Construct an empty `RouteParams`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up a parameter by the name declared in the pattern.
    /// Returns `None` when the parameter was not declared (typo or
    /// pattern mismatch).
    pub fn get(&self, name: &str) -> Option<&str> {
        self.inner.get(name).map(|s| s.as_str())
    }

    /// Iterate over `(name, value)` pairs in deterministic order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.inner.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// `true` when the pattern declared no parameters.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Internal: insert a captured value. Public-crate only so the
    /// matcher can populate it.
    pub(crate) fn insert(&mut self, name: String, value: String) {
        self.inner.insert(name, value);
    }
}

/// Boxed-future return type for [`EmbedHandlerFn`]. Type-aliased so the
/// `Pin<Box<dyn Future<…>>>` shape doesn't sprawl through call sites.
pub type EmbedHandlerFuture = Pin<Box<dyn Future<Output = Response> + Send>>;

/// Erased handler stored inside [`EmbedHandlerSet`]. The shape is
/// `Fn(Request, RouteParams) -> Future<Output = Response>` because the
/// public builder method takes an `async fn` (or async closure) and we
/// box the resulting future once at registration time.
pub type EmbedHandlerFn =
    Arc<dyn Fn(Request<Body>, RouteParams) -> EmbedHandlerFuture + Send + Sync + 'static>;

/// One registered handler — pattern plus the boxed dispatch closure.
#[derive(Clone)]
pub struct EmbedHandler {
    pub pattern: String,
    pub handler: EmbedHandlerFn,
}

/// Bundle of embed-API handlers passed into the dev server through
/// [`crate::routes::AppState`]. Cloned cheaply (the underlying
/// `Arc<Vec<_>>` is shared).
#[derive(Clone, Default)]
pub struct EmbedHandlerSet {
    handlers: Arc<Vec<EmbedHandler>>,
}

impl EmbedHandlerSet {
    /// Construct from a list of handlers. First-registered wins on
    /// overlapping patterns.
    pub fn new(handlers: Vec<EmbedHandler>) -> Self {
        Self {
            handlers: Arc::new(handlers),
        }
    }

    /// `true` when no handlers were registered.
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    /// Number of registered handlers.
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Find the first handler whose pattern matches `url_path` and
    /// return it together with the captured params. Returns `None`
    /// when no pattern claims the path.
    ///
    /// `url_path` should be a leading-slash absolute path with the
    /// dev-server mount prefix already stripped (i.e. the same shape a
    /// production deployment would see).
    pub fn find_match(&self, url_path: &str) -> Option<(EmbedHandlerFn, RouteParams)> {
        for h in self.handlers.iter() {
            if let Some(params) = pattern_matches(&h.pattern, url_path) {
                return Some((Arc::clone(&h.handler), params));
            }
        }
        None
    }
}

/// Wrap an async-closure-shaped handler into the boxed-future form the
/// dispatch table stores. Used internally by
/// [`crate::ServerBuilder::with_ssr_handler`] and exposed at
/// crate-private visibility for the same module's tests.
pub(crate) fn erase_handler<F, Fut, R>(handler: F) -> EmbedHandlerFn
where
    F: Fn(Request<Body>, RouteParams) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: IntoResponse + 'static,
{
    Arc::new(move |req, params| {
        let fut = handler(req, params);
        Box::pin(async move { fut.await.into_response() }) as EmbedHandlerFuture
    })
}

/// Test-only erasure helper exposed for the precedence smoke test in
/// `tests/embed_ssr_smoke.rs`, which composes an [`EmbedHandlerSet`]
/// alongside an [`crate::ssr::SsrRouteSet`] (the public builder does
/// not yet accept the SSR set, so the test reaches under it). Not part
/// of the stable public surface — `#[doc(hidden)]` keeps it out of the
/// rendered API docs.
#[doc(hidden)]
pub fn erase_handler_for_test<F, Fut, R>(handler: F) -> EmbedHandlerFn
where
    F: Fn(Request<Body>, RouteParams) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: IntoResponse + 'static,
{
    erase_handler(handler)
}

/// Match `url_path` against `pattern` per the grammar documented at the
/// module level. Returns the captured params on a hit, or `None` on
/// any mismatch (including grammatical violations like an empty
/// segment).
///
/// Both inputs are interpreted as leading-slash paths; a missing
/// leading slash is tolerated.
pub fn pattern_matches(pattern: &str, url_path: &str) -> Option<RouteParams> {
    // Strip a single leading slash on each side so the split below
    // doesn't yield a leading empty segment.
    let pattern_body = pattern.trim_start_matches('/');
    let path_body = url_path.trim_start_matches('/');

    let p_segs: Vec<&str> = if pattern_body.is_empty() {
        Vec::new()
    } else {
        pattern_body.split('/').collect()
    };
    let u_segs: Vec<&str> = if path_body.is_empty() {
        Vec::new()
    } else {
        path_body.split('/').collect()
    };

    let mut params = RouteParams::new();

    for (idx, p) in p_segs.iter().enumerate() {
        if let Some(name) = p.strip_prefix('*') {
            // Wildcard tail must be the last pattern segment.
            if idx != p_segs.len() - 1 {
                return None;
            }
            if name.is_empty() {
                return None;
            }
            // Wildcard must capture at least one non-empty segment.
            let rest = &u_segs[idx..];
            if rest.iter().any(|s| s.is_empty()) || rest.is_empty() {
                return None;
            }
            params.insert(name.to_string(), rest.join("/"));
            return Some(params);
        }

        if let Some(name) = p.strip_prefix(':') {
            if name.is_empty() {
                return None;
            }
            // Single-segment param must consume exactly one non-empty
            // segment.
            if idx >= u_segs.len() {
                return None;
            }
            let seg = u_segs[idx];
            if seg.is_empty() {
                return None;
            }
            params.insert(name.to_string(), seg.to_string());
            continue;
        }

        // Literal segment.
        if idx >= u_segs.len() || u_segs[idx] != *p {
            return None;
        }
    }

    // No wildcard absorbed the tail — the URL must have used exactly
    // as many segments as the pattern.
    if u_segs.len() != p_segs.len() {
        return None;
    }
    Some(params)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    fn matched(pattern: &str, url: &str) -> RouteParams {
        pattern_matches(pattern, url).unwrap_or_else(|| panic!("expected {pattern} to match {url}"))
    }

    #[test]
    fn literal_pattern_matches_exact() {
        assert!(pattern_matches("/health", "/health").is_some());
        assert!(pattern_matches("/health", "/healthz").is_none());
        assert!(pattern_matches("/health", "/").is_none());
        assert!(pattern_matches("/", "/").is_some());
    }

    #[test]
    fn single_segment_param_captures_one_segment() {
        let params = matched("/users/:id", "/users/42");
        assert_eq!(params.get("id"), Some("42"));
        assert!(pattern_matches("/users/:id", "/users").is_none());
        assert!(pattern_matches("/users/:id", "/users/").is_none());
        assert!(pattern_matches("/users/:id", "/users/42/extra").is_none());
    }

    #[test]
    fn wildcard_tail_captures_remaining_segments() {
        let params = matched("/files/*rest", "/files/a/b/c.txt");
        assert_eq!(params.get("rest"), Some("a/b/c.txt"));
        let params = matched("/files/*rest", "/files/one");
        assert_eq!(params.get("rest"), Some("one"));
        assert!(pattern_matches("/files/*rest", "/files").is_none());
        assert!(pattern_matches("/files/*rest", "/files/").is_none());
    }

    #[test]
    fn wildcard_must_be_last_segment() {
        assert!(pattern_matches("/files/*rest/tail", "/files/a/tail").is_none());
    }

    #[test]
    fn mixed_pattern_captures_multiple_params() {
        let params = matched(
            "/projects/:proj/refs/*rest",
            "/projects/zfb/refs/heads/main",
        );
        assert_eq!(params.get("proj"), Some("zfb"));
        assert_eq!(params.get("rest"), Some("heads/main"));
    }

    #[test]
    fn empty_param_name_is_rejected() {
        assert!(pattern_matches("/x/:", "/x/foo").is_none());
        assert!(pattern_matches("/x/*", "/x/foo").is_none());
    }

    #[test]
    fn empty_set_never_matches() {
        let set = EmbedHandlerSet::default();
        assert!(set.find_match("/anything").is_none());
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
    }

    #[tokio::test]
    async fn erased_handler_forwards_request_and_params() {
        let handler = erase_handler(|req: Request<Body>, params: RouteParams| async move {
            let path = req.uri().path().to_string();
            let id = params.get("id").unwrap_or("none").to_string();
            (StatusCode::OK, format!("{path}|{id}"))
        });
        let req = Request::builder()
            .uri("/users/42")
            .body(Body::empty())
            .unwrap();
        let mut params = RouteParams::new();
        params.insert("id".into(), "42".into());

        let resp = handler(req, params).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"/users/42|42");
    }

    #[tokio::test]
    async fn handler_set_dispatches_first_match() {
        let h1 = erase_handler(|_req, _params: RouteParams| async { (StatusCode::OK, "first") });
        let h2 = erase_handler(|_req, _params: RouteParams| async { (StatusCode::OK, "second") });
        let set = EmbedHandlerSet::new(vec![
            EmbedHandler {
                pattern: "/foo".into(),
                handler: h1,
            },
            EmbedHandler {
                pattern: "/foo".into(),
                handler: h2,
            },
        ]);
        let (handler, _params) = set.find_match("/foo").expect("hit");
        let req = Request::builder().uri("/foo").body(Body::empty()).unwrap();
        let resp = handler(req, RouteParams::new()).await;
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"first");
    }
}
