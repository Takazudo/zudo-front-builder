//! Host-header allowlist + Origin-check support (issue #931 / #919).
//!
//! ## What this protects against
//!
//! Once the dev/preview server is bound to a non-localhost interface
//! (`--host 0.0.0.0`, the bare `--host` LAN shortcut, or `host` in
//! `zfb.config.ts`), any site the developer's browser visits can issue
//! requests that reach the server via **DNS rebinding**: the attacker's
//! domain re-resolves to the LAN/loopback IP, the browser happily sends
//! requests with `Host: attacker.example`, and same-origin policy no
//! longer protects the rendered pages or the plugin/SSR fetch handlers.
//! Validating the `Host` header against an allowlist (the same defence
//! Vite ships as `server.allowedHosts`) closes that hole. The companion
//! Origin check (see [`HostValidation::origin_allowed`], enforced at the
//! dynamic-dispatch sites in [`crate::routes`]) covers CSRF-style
//! cross-origin non-GET requests to SSR/plugin/embed handlers.
//!
//! ## Defaults
//!
//! - **Bound to a loopback interface (the default):** validation is
//!   disabled entirely — [`apply_host_validation_layer`] returns the
//!   router unchanged and every Origin is accepted. Zero behaviour
//!   change for the common `localhost` case.
//! - **Bound to a non-loopback interface:** enforcement is on. The
//!   always-allowed set is `localhost`, `127.0.0.1`, `[::1]`, the bound
//!   IP itself, and the explicitly bound host string (e.g.
//!   `--host mydev.local`). Additional hosts come from the user's
//!   `allowedHosts` config entries.
//!
//! ## Matching rules (config entries)
//!
//! - Entries match the request's host **exactly**, case-insensitively.
//! - The port is stripped from the request's `Host` header before
//!   comparison (`example.com:3000` matches the entry `example.com`).
//! - IPv6 literals are compared without brackets — `[::1]:3000`,
//!   `[::1]`, and `::1` all normalise to `::1`, and config entries may
//!   be written with or without brackets.
//! - A leading-dot entry (`.example.com`) matches the bare domain AND
//!   every subdomain (`example.com`, `api.example.com`) — but never a
//!   non-boundary suffix like `notexample.com`. Mirrors Vite.
//!
//! Disallowed hosts get a `403` whose body follows the #926 policy:
//! explanatory in Dev mode, generic in Preview/Embed (detail goes to
//! the server log instead).

use std::net::IpAddr;
use std::sync::Arc;

use axum::extract::Request;
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;
use zfb_types::escape_html;

use crate::ServerMode;

/// One allowlist rule, pre-normalised (lowercase, brackets stripped).
#[derive(Clone, Debug)]
enum AllowRule {
    /// Matches the host exactly.
    Exact(String),
    /// From a leading-dot config entry: matches the stored domain
    /// itself and any subdomain of it.
    Suffix(String),
}

/// Immutable host-allowlist state shared by the Host-header middleware
/// and the Origin checks at the dynamic-dispatch sites.
///
/// Cheap to clone relative to request volume (it is cloned once per
/// router build, not per request — the middleware holds an [`Arc`]).
#[derive(Clone, Debug)]
pub struct HostValidation {
    /// `false` when the server is bound to a loopback interface — every
    /// check short-circuits to "allowed" so the default `localhost`
    /// setup sees zero behaviour change.
    enforce: bool,
    /// Drives the 403 body shape (#926 policy: explanatory in Dev,
    /// generic in Preview/Embed).
    mode: ServerMode,
    rules: Vec<AllowRule>,
}

impl HostValidation {
    /// Validation that allows everything. For callers that have no bind
    /// address in scope (router-level unit tests, synthetic states).
    pub fn disabled() -> Self {
        Self {
            enforce: false,
            mode: ServerMode::Dev,
            rules: Vec::new(),
        }
    }

    /// Build the validation state for a server bound to `bind_ip`.
    ///
    /// - `bind_ip` decides enforcement: loopback → disabled (the
    ///   default-bind short-circuit), anything else → enforced.
    /// - `bound_host` is the host string the user explicitly bound
    ///   (`--host mydev.local` / `host` in config) — always allowed so
    ///   the URL the server tells the user to open never 403s.
    /// - `allowed_hosts` are the user's `allowedHosts` config entries.
    pub fn for_bind(
        bind_ip: IpAddr,
        bound_host: Option<&str>,
        allowed_hosts: &[String],
        mode: ServerMode,
    ) -> Self {
        let enforce = !bind_ip.is_loopback();
        let mut rules = vec![
            AllowRule::Exact("localhost".to_string()),
            AllowRule::Exact("127.0.0.1".to_string()),
            // Normalised form of `[::1]` (brackets are stripped on both
            // sides of every comparison).
            AllowRule::Exact("::1".to_string()),
            // The bound IP itself (e.g. `--host 192.168.1.5` reached as
            // `http://192.168.1.5:3000`). For unspecified binds this
            // stores `0.0.0.0` / `::`, which no real browser sends —
            // harmless.
            AllowRule::Exact(bind_ip.to_string().to_ascii_lowercase()),
        ];
        if let Some(raw) = bound_host {
            if let Some(host) = host_without_port(raw) {
                rules.push(AllowRule::Exact(host));
            }
        }
        for entry in allowed_hosts {
            if let Some(rule) = parse_config_entry(entry) {
                rules.push(rule);
            }
        }
        Self {
            enforce,
            mode,
            rules,
        }
    }

    /// Whether checks are active (i.e. the server is bound to a
    /// non-loopback interface).
    pub fn is_enforced(&self) -> bool {
        self.enforce
    }

    /// The server mode the 403 bodies are shaped for.
    pub fn mode(&self) -> ServerMode {
        self.mode
    }

    /// Check a raw `Host` header value (`example.com:3000`,
    /// `[::1]:3000`, …) against the allowlist. Always `true` when
    /// enforcement is off; `false` for unparseable values (fail
    /// closed).
    pub fn host_allowed(&self, host_header: &str) -> bool {
        if !self.enforce {
            return true;
        }
        let Some(host) = host_without_port(host_header) else {
            return false;
        };
        self.rules.iter().any(|rule| match rule {
            AllowRule::Exact(e) => *e == host,
            AllowRule::Suffix(s) => {
                host == *s
                    || (host.len() > s.len()
                        && host.ends_with(s.as_str())
                        && host.as_bytes()[host.len() - s.len() - 1] == b'.')
            }
        })
    }

    /// Check a raw `Origin` header value (`https://example.com:3000`)
    /// against the same allowlist. Always `true` when enforcement is
    /// off. `Origin: null` (opaque origins) and unparseable values fail
    /// closed — a sandboxed-iframe POST to a LAN-exposed dev server is
    /// exactly the cross-origin shape this guards against.
    pub fn origin_allowed(&self, origin: &str) -> bool {
        if !self.enforce {
            return true;
        }
        let trimmed = origin.trim();
        let Some((_, rest)) = trimmed.split_once("://") else {
            return false;
        };
        let authority = rest
            .split(['/', '?', '#'])
            .next()
            .unwrap_or(rest);
        self.host_allowed(authority)
    }
}

/// Extract the host portion of a `host[:port]` authority string,
/// normalised to lowercase with IPv6 brackets stripped. Returns `None`
/// for values with no recoverable host (empty, lone brackets, …).
fn host_without_port(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(rest) = raw.strip_prefix('[') {
        // Bracketed IPv6 literal, optionally followed by `:port`.
        let end = rest.find(']')?;
        let inner = &rest[..end];
        if inner.is_empty() {
            return None;
        }
        return Some(inner.to_ascii_lowercase());
    }
    // More than one colon without brackets = a bare IPv6 literal (which
    // cannot carry a port); exactly one colon = `host:port`.
    let host = match raw.matches(':').count() {
        1 => raw.split(':').next().unwrap_or(raw),
        _ => raw,
    };
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

/// Normalise one `allowedHosts` config entry into a rule. Empty /
/// whitespace-only entries (and a bare `"."`) are dropped — config-load
/// validation in the `zfb` crate rejects them earlier with a clear
/// message, so this is belt-and-braces for embed callers.
fn parse_config_entry(entry: &str) -> Option<AllowRule> {
    let e = entry.trim();
    if e.is_empty() {
        return None;
    }
    if let Some(suffix) = e.strip_prefix('.') {
        let s = suffix.trim();
        if s.is_empty() {
            return None;
        }
        return Some(AllowRule::Suffix(s.to_ascii_lowercase()));
    }
    // Accept IPv6 entries with or without brackets — comparisons run on
    // the bracket-stripped form.
    let bare = e
        .strip_prefix('[')
        .and_then(|r| r.strip_suffix(']'))
        .unwrap_or(e);
    Some(AllowRule::Exact(bare.to_ascii_lowercase()))
}

/// Build the `403 Forbidden` served for a disallowed `Host` header.
/// Body shape follows the #926 policy: explanatory in Dev mode (names
/// the host and the `allowedHosts` knob), generic in Preview/Embed
/// (detail goes to the server log).
pub(crate) fn host_forbidden_response(host: &str, mode: ServerMode) -> Response {
    let detail = format!(
        "request Host {host:?} is not in the allowed-hosts set; add it to `allowedHosts` in zfb.config.ts"
    );
    forbidden_response("blocked Host header", &detail, mode)
}

/// Build the `403 Forbidden` served for a cross-origin non-GET request
/// to a dynamic (SSR/plugin/embed) dispatch surface. Same #926 body
/// policy as [`host_forbidden_response`].
pub(crate) fn origin_forbidden_response(origin: &str, mode: ServerMode) -> Response {
    let detail = format!(
        "cross-origin request blocked: Origin {origin:?} is not in the allowed-hosts set; add its host to `allowedHosts` in zfb.config.ts"
    );
    forbidden_response("blocked cross-origin request", &detail, mode)
}

fn forbidden_response(title: &str, detail: &str, mode: ServerMode) -> Response {
    // Dev mode: verbose body with the rejected value + remediation hint.
    // Preview/Embed: generic body only; full detail is logged
    // server-side so clients never see internal info (#926 policy).
    let body = if matches!(mode, ServerMode::Dev) {
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>zfb dev \u{2014} 403</title></head><body><h1>403 \u{2014} {}</h1><pre>{}</pre></body></html>",
            escape_html(title),
            escape_html(detail),
        )
    } else {
        tracing::warn!(detail, "{title}");
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Forbidden</title></head><body><h1>Forbidden</h1></body></html>".to_string()
    };
    let mut resp = (
        StatusCode::FORBIDDEN,
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

/// Apply the Host-header validation layer to a router.
///
/// When `validation` is not enforced (loopback bind / `disabled()`)
/// the router is returned unchanged so the default `localhost` paths
/// pay nothing — mirrors
/// [`crate::middleware::apply_request_extension_layer`]'s empty
/// short-circuit.
///
/// Public (not `pub(crate)`) because `zfb preview`'s static-mode router
/// lives in the bin crate and needs the same protection.
pub fn apply_host_validation_layer(router: Router, validation: HostValidation) -> Router {
    if !validation.is_enforced() {
        return router;
    }
    let state = Arc::new(validation);
    router.layer(from_fn_with_state(state, validate_host_middleware))
}

/// The middleware closure: reject requests whose `Host` header (or
/// `:authority`, for HTTP/2-shaped requests with no Host) fails the
/// allowlist. Requests with no host information at all are allowed
/// with a warning — browsers always send `Host`, so such a request
/// cannot be a browser-driven rebinding attack.
async fn validate_host_middleware(
    axum::extract::State(validation): axum::extract::State<Arc<HostValidation>>,
    req: Request,
    next: Next,
) -> Response {
    let raw_host = match req.headers().get(header::HOST) {
        Some(value) => match value.to_str() {
            Ok(v) => Some(v.to_string()),
            // A Host header that is not valid visible-ASCII is bogus —
            // fail closed.
            Err(_) => {
                return host_forbidden_response("<non-ASCII>", validation.mode());
            }
        },
        None => req.uri().authority().map(|a| a.as_str().to_string()),
    };
    match raw_host {
        Some(host) => {
            if validation.host_allowed(&host) {
                next.run(req).await
            } else {
                host_forbidden_response(&host, validation.mode())
            }
        }
        None => {
            tracing::warn!(
                uri = %req.uri(),
                "request without Host header reached the host-validation layer; allowing"
            );
            next.run(req).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::routing::get;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use tower::ServiceExt; // for `Router::oneshot`.

    const LAN_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5));
    const ANY_IP: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);

    fn enforcing(allowed: &[&str]) -> HostValidation {
        let entries: Vec<String> = allowed.iter().map(|s| s.to_string()).collect();
        HostValidation::for_bind(ANY_IP, None, &entries, ServerMode::Dev)
    }

    // --- matcher: exact entries -------------------------------------

    #[test]
    fn exact_entry_matches_and_others_do_not() {
        let v = enforcing(&["example.com"]);
        assert!(v.host_allowed("example.com"));
        assert!(!v.host_allowed("evil.com"));
        // Substring / superstring shapes must not match an exact entry.
        assert!(!v.host_allowed("subdomain.example.com"));
        assert!(!v.host_allowed("example.com.evil.com"));
    }

    #[test]
    fn builtin_localhost_forms_always_allowed_when_enforcing() {
        let v = enforcing(&[]);
        assert!(v.host_allowed("localhost"));
        assert!(v.host_allowed("localhost:3000"));
        assert!(v.host_allowed("127.0.0.1"));
        assert!(v.host_allowed("127.0.0.1:3000"));
        assert!(v.host_allowed("[::1]"));
        assert!(v.host_allowed("[::1]:3000"));
        assert!(!v.host_allowed("evil.test"));
    }

    // --- matcher: dot-suffix entries ---------------------------------

    #[test]
    fn dot_suffix_entry_matches_bare_domain_and_subdomains() {
        let v = enforcing(&[".example.com"]);
        assert!(v.host_allowed("example.com"));
        assert!(v.host_allowed("api.example.com"));
        assert!(v.host_allowed("deep.api.example.com"));
        // No dot boundary — must NOT match.
        assert!(!v.host_allowed("notexample.com"));
        // Suffix on the wrong side.
        assert!(!v.host_allowed("example.com.evil.com"));
    }

    // --- matcher: port stripping --------------------------------------

    #[test]
    fn port_is_stripped_before_comparison() {
        let v = enforcing(&["example.com"]);
        assert!(v.host_allowed("example.com:3000"));
        assert!(v.host_allowed("example.com:80"));
        assert!(!v.host_allowed("evil.com:3000"));
    }

    // --- matcher: IPv6 literals ---------------------------------------

    #[test]
    fn ipv6_literals_match_with_or_without_brackets_and_port() {
        let v = enforcing(&["[2001:db8::1]"]);
        assert!(v.host_allowed("[2001:db8::1]"));
        assert!(v.host_allowed("[2001:db8::1]:3000"));
        // Unbracketed config spelling must behave identically.
        let v2 = enforcing(&["2001:db8::1"]);
        assert!(v2.host_allowed("[2001:db8::1]:3000"));
        assert!(!v2.host_allowed("[2001:db8::2]:3000"));
    }

    // --- matcher: case-insensitivity -----------------------------------

    #[test]
    fn hostname_comparison_is_case_insensitive() {
        let v = enforcing(&["Example.COM", ".Sub.Example.ORG"]);
        assert!(v.host_allowed("EXAMPLE.com:8080"));
        assert!(v.host_allowed("API.SUB.EXAMPLE.org"));
        assert!(!v.host_allowed("EVIL.com"));
    }

    // --- matcher: bound host / bound IP --------------------------------

    #[test]
    fn bound_host_and_bind_ip_are_always_allowed() {
        let v = HostValidation::for_bind(
            LAN_IP,
            Some("mydev.local"),
            &[],
            ServerMode::Dev,
        );
        assert!(v.is_enforced());
        assert!(v.host_allowed("192.168.1.5:3000"));
        assert!(v.host_allowed("mydev.local:3000"));
        assert!(!v.host_allowed("evil.test"));
    }

    // --- matcher: loopback short-circuit -------------------------------

    #[test]
    fn loopback_bind_disables_enforcement_entirely() {
        for ip in [
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        ] {
            let v = HostValidation::for_bind(ip, None, &[], ServerMode::Dev);
            assert!(!v.is_enforced());
            assert!(v.host_allowed("absolutely.anything.example"));
            assert!(v.origin_allowed("https://absolutely.anything.example"));
        }
    }

    // --- matcher: malformed values fail closed -------------------------

    #[test]
    fn malformed_hosts_fail_closed_when_enforcing() {
        let v = enforcing(&["example.com"]);
        assert!(!v.host_allowed(""));
        assert!(!v.host_allowed("   "));
        assert!(!v.host_allowed("["));
        assert!(!v.host_allowed("[]"));
        assert!(!v.host_allowed(":3000"));
    }

    // --- origin matcher --------------------------------------------------

    #[test]
    fn origin_allowed_parses_scheme_and_port() {
        let v = enforcing(&["example.com", ".sub.test"]);
        assert!(v.origin_allowed("https://example.com"));
        assert!(v.origin_allowed("http://example.com:3000"));
        assert!(v.origin_allowed("http://api.sub.test:8080"));
        assert!(v.origin_allowed("http://localhost:5173"));
        assert!(!v.origin_allowed("https://evil.com"));
        // Opaque / unparseable origins fail closed.
        assert!(!v.origin_allowed("null"));
        assert!(!v.origin_allowed(""));
    }

    #[test]
    fn origin_allowed_handles_ipv6_origins() {
        let v = enforcing(&[]);
        assert!(v.origin_allowed("http://[::1]:3000"));
        assert!(!v.origin_allowed("http://[2001:db8::7]:3000"));
    }

    // --- layer behaviour ---------------------------------------------------

    fn test_router() -> Router {
        Router::new().route("/", get(|| async { "ok" }))
    }

    async fn status_for(router: Router, host: Option<&str>) -> (StatusCode, String) {
        let mut builder = HttpRequest::builder().uri("/");
        if let Some(h) = host {
            builder = builder.header(header::HOST, h);
        }
        let resp = router
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    #[tokio::test]
    async fn layer_rejects_disallowed_host_with_dev_body() {
        let v = HostValidation::for_bind(ANY_IP, None, &[], ServerMode::Dev);
        let router = apply_host_validation_layer(test_router(), v);
        let (status, body) = status_for(router, Some("evil.test:3000")).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        // Dev mode: explanatory body naming the host + the config knob.
        assert!(body.contains("evil.test"), "body: {body}");
        assert!(body.contains("allowedHosts"), "body: {body}");
    }

    #[tokio::test]
    async fn layer_rejects_disallowed_host_with_generic_body_in_preview() {
        let v = HostValidation::for_bind(ANY_IP, None, &[], ServerMode::Preview);
        let router = apply_host_validation_layer(test_router(), v);
        let (status, body) = status_for(router, Some("evil.test:3000")).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        // Preview/Embed: generic body only — no host echo, no knob hint.
        assert!(!body.contains("evil.test"), "body: {body}");
        assert!(!body.contains("allowedHosts"), "body: {body}");
        assert!(body.contains("Forbidden"), "body: {body}");
    }

    #[tokio::test]
    async fn layer_passes_allowed_host_through() {
        let v = HostValidation::for_bind(
            ANY_IP,
            None,
            &["allowed.test".to_string()],
            ServerMode::Dev,
        );
        let router = apply_host_validation_layer(test_router(), v);
        let (status, body) = status_for(router, Some("allowed.test:3000")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "ok");
    }

    #[tokio::test]
    async fn layer_allows_requests_without_host_header() {
        let v = HostValidation::for_bind(ANY_IP, None, &[], ServerMode::Dev);
        let router = apply_host_validation_layer(test_router(), v);
        let (status, _) = status_for(router, None).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn layer_is_noop_when_not_enforced() {
        let router =
            apply_host_validation_layer(test_router(), HostValidation::disabled());
        let (status, _) = status_for(router, Some("evil.test")).await;
        assert_eq!(status, StatusCode::OK);
    }
}
