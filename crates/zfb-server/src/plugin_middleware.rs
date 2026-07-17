//! Plugin dev-middleware integration (Sub 3 / #108).
//!
//! `zfb-server` itself does not depend on `zfb-build::PluginHost` — it
//! accepts a [`DevMiddlewareDispatcher`] trait object instead. The
//! dev command (`crates/zfb/src/commands/dev.rs`) plumbs the host
//! through a small adapter and the server only sees:
//!
//! - the list of `(path-prefix, handler-id)` registrations declared by
//!   user plugins, and
//! - a [`DevMiddlewareDispatcher::dispatch`] callback that takes a
//!   `(handler_id, request)` and returns the plugin's response.
//!
//! The dev router matches each incoming request's path against the
//! longest-prefix registration. A handler that returns
//! [`PluginPassthrough`] signals "I did not handle this request" — the
//! server then falls through to its built-in routes (the page cache,
//! `/__zfb/livereload.js`, etc.) so dev-middleware can layer cleanly
//! over the dev surface without owning the whole URL space.
//!
//! ## Mode-aware dispatch seam (issue #1544)
//!
//! [`dispatch_plugin`], [`origin_gate`], [`plugin_error_response`], and
//! [`PLUGIN_BODY_LIMIT_BYTES`] were extracted out of `routes.rs` so the
//! dev router and the preview server (`zfb preview` in
//! `crates/zfb/src/commands/preview.rs`, wired through
//! `crates/zfb/src/commands/plugins.rs`) share ONE implementation
//! instead of a drifting reimplementation. Every piece
//! here is mode-aware — it takes a [`crate::ServerMode`] (or a
//! [`crate::host_validation::HostValidation`] that already carries one)
//! and adjusts behaviour accordingly (verbose vs. generic error bodies)
//! — but otherwise has no dependency on `routes::AppState`, so it is
//! callable standalone from a router that never builds one.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use zfb_types::escape_html;

/// One registered dev-middleware handler.
#[derive(Debug, Clone)]
pub struct PluginRegistration {
    /// URL path prefix this handler claims. Matched as a longest
    /// prefix; a registration on `/doc-history` matches
    /// `/doc-history`, `/doc-history/`, and `/doc-history/foo`.
    pub path: String,
    /// Opaque token the dispatcher uses to identify the handler in the
    /// underlying plugin host.
    pub handler_id: String,
    /// Plugin display name. Surfaced in error responses so a 5xx from
    /// a plugin handler points at the offending plugin.
    pub plugin: String,
}

/// One HTTP request shipped from the dev server to a plugin handler.
#[derive(Debug, Clone)]
pub struct PluginRequest {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: Option<String>,
}

/// One HTTP response returned from a plugin handler.
///
/// `headers` is an ordered `Vec<(name, value)>` multimap (not a map) so
/// multi-valued response headers — most notably multiple `Set-Cookie` —
/// survive to the wire: the dev router `append`s each entry onto the
/// `http::HeaderMap` rather than `insert`ing (which collapses duplicates
/// to last-value-wins). The request side ([`PluginRequest::headers`])
/// stays a `HashMap` — request headers don't carry the duplicate-value
/// concern here.
#[derive(Debug, Clone)]
pub struct PluginResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// Response body. UTF-8 text by default; if `body_encoding` is
    /// `"base64"` the bytes are decoded before being written.
    pub body: String,
    pub body_encoding: PluginResponseEncoding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginResponseEncoding {
    Utf8,
    Base64,
}

impl PluginResponseEncoding {
    pub fn parse(s: &str) -> Self {
        match s {
            "base64" => Self::Base64,
            _ => Self::Utf8,
        }
    }
}

/// Outcome of dispatching one request to a plugin handler.
#[derive(Debug, Clone)]
pub enum PluginDispatchOutcome {
    /// Handler returned a real response — the server should send it.
    Response(PluginResponse),
    /// Handler returned `undefined` (passthrough) — the server should
    /// fall through to its built-in routes.
    Passthrough,
}

/// Shape implemented by adapters that own the underlying plugin host
/// (typically the long-lived `PluginHost` from `zfb-build`).
#[async_trait]
pub trait DevMiddlewareDispatcher: Send + Sync {
    async fn dispatch(
        &self,
        handler_id: &str,
        request: PluginRequest,
    ) -> Result<PluginDispatchOutcome, PluginDispatchError>;
}

/// Error returned by [`DevMiddlewareDispatcher::dispatch`] when the
/// underlying plugin host fails (e.g. the plugin handler threw, or the
/// host process died).
#[derive(Debug, thiserror::Error)]
#[error("plugin `{plugin}` dev-middleware failed: {message}")]
pub struct PluginDispatchError {
    pub plugin: String,
    pub message: String,
}

/// Bundle of registrations + dispatcher passed into the dev server.
/// Cloned cheaply (the heavy state lives behind `Arc`s); each route
/// handler keeps a reference for the lifetime of its task.
#[derive(Clone)]
pub struct DevMiddlewareSet {
    pub registrations: Arc<Vec<PluginRegistration>>,
    pub dispatcher: Arc<dyn DevMiddlewareDispatcher>,
}

impl DevMiddlewareSet {
    pub fn empty(dispatcher: Arc<dyn DevMiddlewareDispatcher>) -> Self {
        Self {
            registrations: Arc::new(Vec::new()),
            dispatcher,
        }
    }

    /// Find the handler whose registered path is the longest prefix of
    /// `url_path`. Returns `None` when no plugin claims this URL.
    pub fn find_match(&self, url_path: &str) -> Option<&PluginRegistration> {
        // Longest-prefix wins so a plugin can claim `/api/foo` and a
        // separate plugin can claim `/api`. Iterate the registration
        // list and pick the longest match — registration count per
        // project is small, no need for a trie here.
        let mut best: Option<&PluginRegistration> = None;
        for reg in self.registrations.iter() {
            if path_matches_prefix(url_path, &reg.path)
                && best.map(|b| b.path.len() < reg.path.len()).unwrap_or(true)
            {
                best = Some(reg);
            }
        }
        best
    }
}

/// True iff `url_path` is exactly `prefix` or starts with `prefix/`.
/// Avoids the classic `"foox".starts_with("foo")` false-positive.
///
/// Trailing slashes are normalised first so a registration at `/foo/`
/// matches a request to `/foo`, `/foo/`, or `/foo/bar` — the operator
/// shouldn't have to think about whether a plugin spelled the prefix
/// with or without a trailing slash. Matching the empty prefix `/` is
/// allowed and behaves like a catch-all.
pub fn path_matches_prefix(url_path: &str, prefix: &str) -> bool {
    let url = strip_trailing_slash(url_path);
    let pref = strip_trailing_slash(prefix);
    // Empty `pref` after trimming = a literal `/` registration; treat
    // as a catch-all that matches every URL.
    if pref.is_empty() {
        return true;
    }
    if url == pref {
        return true;
    }
    if let Some(rest) = url.strip_prefix(pref) {
        return rest.starts_with('/');
    }
    false
}

fn strip_trailing_slash(s: &str) -> &str {
    // Keep the leading `/` so `"/"` collapses to `""` (the catch-all
    // signal in `path_matches_prefix`). Stripping a trailing slash
    // off `"/foo/"` yields `"/foo"` which matches `"/foo"` exactly.
    if s.len() > 1 && s.ends_with('/') {
        &s[..s.len() - 1]
    } else if s == "/" {
        ""
    } else {
        s
    }
}

// ---------------------------------------------------------------------
// Mode-aware dispatch seam (issue #1544) — extracted from `routes.rs`
// so dev and preview share one implementation.
// ---------------------------------------------------------------------

/// Request body cap applied to the dynamic dispatch surfaces (plugin
/// dev-middleware, and — by the same posture — any other route that
/// extracts a full `Bytes` body with no size guard of its own). 2 MiB
/// is generous enough for any legitimate dev-middleware POST payload
/// while preventing unbounded memory buffering.
pub const PLUGIN_BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;

/// Build the [`DefaultBodyLimit`] layer for [`PLUGIN_BODY_LIMIT_BYTES`].
/// A small convenience so callers building their own router (dev's
/// `build_router` and preview's future router alike) apply the exact
/// same cap via `.layer(plugin_middleware::body_limit_layer())` instead
/// of re-deriving the byte count.
pub fn body_limit_layer() -> DefaultBodyLimit {
    DefaultBodyLimit::max(PLUGIN_BODY_LIMIT_BYTES)
}

/// Origin gate for non-GET/HEAD requests reaching the dynamic dispatch
/// surfaces — plugin dev-middleware, embed handlers, request-time SSR
/// (issue #931 / #919). Returns `Some(403)` when host validation is
/// enforced (every real bind, loopback included since issue #1684) and
/// either:
///
/// - the request carries an `Origin` whose host fails the same allowlist
///   the Host-header layer uses (the DNS-rebinding / cross-origin vector
///   — rejected on every bind), or
/// - the `Origin` header is absent on a non-GET request **and** the bind
///   is non-loopback (LAN). A missing `Origin` can never be a browser
///   cross-origin request, so it is not a rebinding vector; on a LAN bind
///   it is failed closed anyway (an untrusted peer could forge a
///   non-browser request), but on a loopback bind it is allowed — see
///   [`HostValidation::allows_missing_origin`].
///
/// Returns `None` (allow) when:
///
/// - the method is GET/HEAD (safe methods rely on the Host check),
/// - the `Origin` is absent on a **loopback** bind (local non-browser
///   tooling — issue #1684), or
/// - validation is not enforced (only [`HostValidation::disabled`] now —
///   loopback binds otherwise enforce the same gate as LAN binds).
///
/// [`HostValidation::disabled`]: crate::host_validation::HostValidation::disabled
/// [`HostValidation::allows_missing_origin`]: crate::host_validation::HostValidation::allows_missing_origin
///
/// Static read paths (`/assets`, dist/public fallbacks, livereload) are
/// exempt by construction — this helper is only invoked at the dynamic-
/// dispatch call sites in `serve_page` (and, from wave 3, preview's
/// equivalent call sites). The response body's verbosity is driven by
/// `host_validation.mode()` — no separate mode parameter needed since a
/// [`crate::host_validation::HostValidation`] always carries the mode it
/// was built for.
pub fn origin_gate(
    host_validation: &crate::host_validation::HostValidation,
    method: &Method,
    headers: &HeaderMap,
) -> Option<Response> {
    if matches!(*method, Method::GET | Method::HEAD) {
        return None;
    }
    if !host_validation.is_enforced() {
        return None;
    }
    let Some(value) = headers.get(header::ORIGIN) else {
        // A non-GET request that omits the Origin header cannot be a
        // browser cross-origin request (browsers always send Origin on
        // cross-origin non-GET), so it is never a DNS-rebinding/CSRF
        // vector. On a LAN bind it could still be an untrusted non-browser
        // peer bypassing CORS — fail closed (403). On a loopback bind
        // (issue #1684) the only clients are local processes that already
        // own localhost, so allow it: failing closed would break common
        // dev tooling (curl/webhooks POSTing without an Origin) for no
        // rebinding-security gain.
        if host_validation.allows_missing_origin() {
            return None;
        }
        return Some(crate::host_validation::missing_origin_forbidden_response(
            host_validation.mode(),
        ));
    };
    // Present-but-unreadable (non-ASCII) and disallowed origins both
    // fail closed.
    let allowed = value
        .to_str()
        .map(|origin| host_validation.origin_allowed(origin))
        .unwrap_or(false);
    if allowed {
        return None;
    }
    let shown = value.to_str().unwrap_or("<non-ASCII>");
    Some(crate::host_validation::origin_forbidden_response(
        shown,
        host_validation.mode(),
    ))
}

/// Build the HTML 5xx response served when a plugin dev-middleware
/// handler fails to dispatch. Mode-gated per issue #926: Dev mode shows
/// the underlying plugin error so the developer sees it immediately;
/// Preview/Embed show a generic body only (detail goes to the server
/// log) so internal error text never leaks to a client.
pub fn plugin_error_response(message: &str, mode: crate::ServerMode) -> Response {
    let body = if matches!(mode, crate::ServerMode::Dev) {
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>zfb dev — plugin error</title></head><body><h1>Plugin dev-middleware error</h1><pre>{}</pre></body></html>",
            escape_html(message),
        )
    } else {
        tracing::error!(message, "plugin dev-middleware error");
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Internal Server Error</title></head><body><h1>Internal Server Error</h1></body></html>".to_string()
    };
    let mut resp = (
        StatusCode::INTERNAL_SERVER_ERROR,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        )],
        body,
    )
        .into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

/// Result of a plugin dev-middleware dispatch attempt. The caller folds
/// `Passthrough` into its own regular fallback lookup (the dev
/// page-cache, or preview's static-file lookup); everything else
/// short-circuits with a Response.
pub enum PluginDispatchAttempt {
    Responded(Response),
    Passthrough,
    Errored(Response),
}

/// Convert an axum [`HeaderMap`] into the flat string map shape the
/// plugin host wire protocol expects. Header values that are not valid
/// UTF-8 are dropped — the JS-side handler receives a string-keyed
/// object and cannot represent arbitrary bytes. Multi-valued headers
/// keep the last seen value (the protocol does not currently model
/// repeated headers; see the dev-middleware contract in
/// `crates/zfb/js/plugin-host.mjs`).
fn headermap_to_string_map(headers: &HeaderMap) -> HashMap<String, String> {
    let mut out = HashMap::with_capacity(headers.len());
    for (name, value) in headers.iter() {
        if let Ok(v) = value.to_str() {
            out.insert(name.as_str().to_string(), v.to_string());
        }
    }
    out
}

/// Convert an inbound request body (already drained into [`Bytes`]) to
/// the `Option<String>` shape the plugin host wire protocol expects.
/// Empty bodies become `None`; non-UTF-8 bodies are dropped (see the
/// note in [`dispatch_plugin`] about binary uploads being a separate
/// extension).
fn body_bytes_to_utf8_string(body: &Bytes) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    std::str::from_utf8(body).ok().map(|s| s.to_string())
}

/// Build a [`PluginRequest`] for the given URL/method/headers/body and
/// dispatch it.
///
/// The `HeaderMap` → string-map and `Bytes` → `Option<String>` wire-shape
/// conversions happen internally so this seam is self-contained: both dev
/// (`serve_page`) and preview call it with the raw axum request pieces and
/// never touch the plugin-host wire encoding (issue #1544).
///
/// Issue #230: the request method, headers, and body are forwarded
/// verbatim so plugin handlers can implement non-GET endpoints (form
/// submissions, save actions, sidecar API proxies). Non-UTF-8 request
/// bodies are dropped here — dev-middleware bodies are line-protocol
/// JSON over stdio to the plugin host, and the wire shape only
/// supports UTF-8 strings. Binary uploads are a separate extension.
pub async fn dispatch_plugin(
    set: &DevMiddlewareSet,
    reg: &PluginRegistration,
    url_path: &str,
    method: &Method,
    headers: &HeaderMap,
    body: &Bytes,
    mode: crate::ServerMode,
) -> PluginDispatchAttempt {
    let req = PluginRequest {
        method: method.as_str().to_string(),
        url: url_path.to_string(),
        headers: headermap_to_string_map(headers),
        body: body_bytes_to_utf8_string(body),
    };
    match set.dispatcher.dispatch(&reg.handler_id, req).await {
        Ok(PluginDispatchOutcome::Response(resp)) => {
            // Decode body and build an axum response. The plugin host
            // pre-validates the status code; clamp out-of-range to 500
            // so a misbehaving handler can't synthesise an invalid
            // response.
            let status =
                StatusCode::from_u16(resp.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let body_bytes: Vec<u8> = match resp.body_encoding {
                // Plugin handlers may opt into binary bodies via
                // `bodyEncoding: "base64"`. Use the standard crate
                // rather than rolling our own decoder so all the edge
                // cases (padding, whitespace, URL-safe alphabet) are
                // handled correctly.
                PluginResponseEncoding::Base64 => {
                    use base64::Engine as _;
                    match base64::engine::general_purpose::STANDARD.decode(resp.body.as_bytes()) {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            let msg = format!(
                                "plugin `{}` returned an invalid base64 body: {}",
                                reg.plugin, e
                            );
                            return PluginDispatchAttempt::Errored(plugin_error_response(
                                &msg, mode,
                            ));
                        }
                    }
                }
                PluginResponseEncoding::Utf8 => resp.body.into_bytes(),
            };
            let mut builder = Response::builder().status(status);
            // `resp.headers` is an ordered `Vec<(name, value)>`; iterating
            // it and calling `Builder::header` (which *appends*) preserves
            // duplicate names, so multiple `Set-Cookie` entries from the
            // plugin each reach the wire instead of collapsing.
            for (k, v) in resp.headers {
                // The body is reconstructed Rust-side (base64 decode /
                // into_bytes), so any Content-Length / Transfer-Encoding the
                // plugin returned is stale; Connection is hop-by-hop. Drop
                // them and let hyper recompute framing (matches dispatch_ssr).
                let lower = k.to_ascii_lowercase();
                if matches!(
                    lower.as_str(),
                    "content-length" | "transfer-encoding" | "connection"
                ) {
                    continue;
                }
                if let Ok(value) = HeaderValue::try_from(v) {
                    builder = builder.header(k, value);
                }
            }
            // Cache busting matches the rest of the dev server — plugin
            // responses are dev-only artefacts; never let a browser
            // cache a stale plugin emission across reloads.
            builder = builder.header(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            match builder.body(axum::body::Body::from(body_bytes)) {
                Ok(resp) => PluginDispatchAttempt::Responded(resp),
                Err(e) => PluginDispatchAttempt::Errored(plugin_error_response(
                    &format!("failed to build response from plugin `{}`: {e}", reg.plugin,),
                    mode,
                )),
            }
        }
        Ok(PluginDispatchOutcome::Passthrough) => PluginDispatchAttempt::Passthrough,
        Err(err) => PluginDispatchAttempt::Errored(plugin_error_response(
            &format!(
                "plugin `{}` dev-middleware failed: {}",
                err.plugin, err.message,
            ),
            mode,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_prefix_wins() {
        struct Noop;
        #[async_trait]
        impl DevMiddlewareDispatcher for Noop {
            async fn dispatch(
                &self,
                _id: &str,
                _r: PluginRequest,
            ) -> Result<PluginDispatchOutcome, PluginDispatchError> {
                Ok(PluginDispatchOutcome::Passthrough)
            }
        }
        let set = DevMiddlewareSet {
            registrations: Arc::new(vec![
                PluginRegistration {
                    path: "/api".into(),
                    handler_id: "h1".into(),
                    plugin: "a".into(),
                },
                PluginRegistration {
                    path: "/api/foo".into(),
                    handler_id: "h2".into(),
                    plugin: "b".into(),
                },
            ]),
            dispatcher: Arc::new(Noop),
        };
        assert_eq!(set.find_match("/api/foo").unwrap().handler_id, "h2");
        assert_eq!(set.find_match("/api/bar").unwrap().handler_id, "h1");
        assert!(set.find_match("/other").is_none());
    }

    #[test]
    fn prefix_does_not_overmatch() {
        // `/foox` must not match a `/foo` registration.
        assert!(!path_matches_prefix("/foox", "/foo"));
        assert!(path_matches_prefix("/foo", "/foo"));
        assert!(path_matches_prefix("/foo/bar", "/foo"));
    }

    #[test]
    fn trailing_slashes_are_normalised_in_both_directions() {
        // Registration with trailing slash matches request without it.
        assert!(path_matches_prefix("/foo", "/foo/"));
        // Request with trailing slash matches registration without it.
        assert!(path_matches_prefix("/foo/", "/foo"));
        // Both with trailing slash.
        assert!(path_matches_prefix("/foo/", "/foo/"));
        // Sub-paths still match.
        assert!(path_matches_prefix("/foo/bar", "/foo/"));
    }

    #[test]
    fn root_prefix_is_a_catchall() {
        // A plugin registering `/` claims every URL — used as a
        // fallback / "catch-everything" surface.
        assert!(path_matches_prefix("/", "/"));
        assert!(path_matches_prefix("/anything", "/"));
        assert!(path_matches_prefix("/anything/nested", "/"));
    }

    #[test]
    fn body_bytes_to_utf8_string_drops_empty_and_non_utf8() {
        // Empty body collapses to None.
        assert!(body_bytes_to_utf8_string(&Bytes::new()).is_none());
        // UTF-8 round-trips.
        assert_eq!(
            body_bytes_to_utf8_string(&Bytes::from("hello")),
            Some("hello".to_string())
        );
        // Non-UTF-8 is dropped (binary upload is a future extension).
        let bad = Bytes::from(vec![0xff, 0xfe, 0xfd]);
        assert!(body_bytes_to_utf8_string(&bad).is_none());
    }

    // --- origin_gate: loopback vs LAN (issue #1684) ---------------------

    fn loopback_validation() -> crate::host_validation::HostValidation {
        crate::host_validation::HostValidation::for_bind(
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            None,
            &[],
            crate::ServerMode::Dev,
        )
    }

    fn lan_validation() -> crate::host_validation::HostValidation {
        crate::host_validation::HostValidation::for_bind(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 5)),
            None,
            &[],
            crate::ServerMode::Dev,
        )
    }

    #[test]
    fn origin_gate_missing_origin_allowed_on_loopback_rejected_on_lan() {
        // Issue #1684: a non-GET request with no Origin header is local
        // non-browser tooling (never a browser cross-origin/rebinding
        // request). Allowed on a loopback bind, failed closed on a LAN
        // bind where an untrusted peer could forge it.
        let empty = HeaderMap::new();
        assert!(
            origin_gate(&loopback_validation(), &Method::POST, &empty).is_none(),
            "loopback bind must allow a non-GET request with no Origin (local tooling)"
        );
        assert_eq!(
            origin_gate(&lan_validation(), &Method::POST, &empty).map(|r| r.status()),
            Some(StatusCode::FORBIDDEN),
            "LAN bind must fail closed on a non-GET request with no Origin"
        );
    }

    #[test]
    fn origin_gate_rejects_present_cross_origin_even_on_loopback() {
        // A present, cross-origin Origin IS the rebinding / CSRF vector —
        // rejected on every enforcing bind, loopback included.
        let mut cross = HeaderMap::new();
        cross.insert(header::ORIGIN, "https://attacker.example".parse().unwrap());
        assert_eq!(
            origin_gate(&loopback_validation(), &Method::POST, &cross).map(|r| r.status()),
            Some(StatusCode::FORBIDDEN),
            "loopback bind must reject a present cross-origin Origin"
        );
        // A same-origin (localhost) Origin passes on a loopback bind.
        let mut same = HeaderMap::new();
        same.insert(header::ORIGIN, "http://localhost:3000".parse().unwrap());
        assert!(
            origin_gate(&loopback_validation(), &Method::POST, &same).is_none(),
            "loopback bind must allow a same-origin localhost Origin"
        );
        // GET is always allowed — safe method, covered by the Host check.
        assert!(origin_gate(&loopback_validation(), &Method::GET, &cross).is_none());
    }

    #[test]
    fn origin_gate_disabled_validation_allows_everything() {
        // `disabled()` short-circuits before the bind check.
        let disabled = crate::host_validation::HostValidation::disabled();
        let mut cross = HeaderMap::new();
        cross.insert(header::ORIGIN, "https://attacker.example".parse().unwrap());
        assert!(origin_gate(&disabled, &Method::POST, &cross).is_none());
        assert!(origin_gate(&disabled, &Method::POST, &HeaderMap::new()).is_none());
    }
}
