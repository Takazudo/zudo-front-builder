//! Host-header allowlist + Origin-check support (issue #931 / #919).
//!
//! ## What this protects against
//!
//! Any site the developer's browser visits can issue requests that
//! reach the dev/preview server via **DNS rebinding**: the attacker's
//! domain re-resolves to the server's IP, the browser happily sends
//! requests with `Host: attacker.example`, and same-origin policy no
//! longer protects the rendered pages or the plugin/SSR fetch handlers.
//! This is **not** limited to LAN binds — a hostile domain can
//! re-resolve to `127.0.0.1` just as easily as to a `192.168.x.x`
//! address, so a loopback-bound server that serves token-bearing or
//! otherwise sensitive responses is readable cross-origin too (issue
//! #1684, raised from zudolab/zzmod#1745). Validating the `Host` header
//! against an allowlist (the same defence Vite ships as
//! `server.allowedHosts`, which Vite also enforces regardless of bind
//! address) closes that hole. The companion Origin check (see
//! [`HostValidation::origin_allowed`], enforced at the dynamic-dispatch
//! sites in [`crate::routes`]) covers CSRF-style cross-origin non-GET
//! requests to SSR/plugin/embed handlers.
//!
//! ## Enforcement
//!
//! Enforcement is **on for every real bind**, loopback included
//! ([`HostValidation::for_bind`]). Only the explicit
//! [`HostValidation::disabled`] constructor — for callers with no bind
//! address in scope (router-level unit tests, synthetic states) — turns
//! every check into a short-circuit "allowed".
//!
//! The always-allowed set is `localhost` (and every `*.localhost`
//! subdomain — RFC 6761 special-use, always loopback, Vite parity),
//! `127.0.0.1`, `[::1]`, the explicitly bound host string (e.g.
//! `--host mydev.local`), and any IP-literal host (the bound IP plus the
//! LAN interface URLs the bind-all startup banner prints — see the
//! matching rules below).
//! Additional hosts come from the user's `allowedHosts` config entries.
//! The common `localhost` / `127.0.0.1` / `[::1]` dev loop therefore
//! keeps working with no config, while a rebinding
//! `Host: attacker.example` — loopback bind or not — gets a 403.
//!
//! ## The companion Origin check on loopback vs LAN
//!
//! The Origin gate ([`crate::plugin_middleware::origin_gate`]) guards
//! non-GET requests to the dynamic-dispatch surfaces. A **present**
//! `Origin` is checked against the same allowlist on every bind — that is
//! the cross-origin / rebinding vector. A **missing** `Origin` differs by
//! bind: rejected on a LAN bind (an untrusted peer could forge a
//! non-browser request) but allowed on a loopback bind, because a browser
//! always attaches `Origin` to a cross-origin non-GET request, so a
//! missing one is local, non-browser tooling (curl, a localhost webhook)
//! that already has full access to loopback — failing it closed would
//! break dev tooling for no rebinding-security gain (issue #1684). See
//! [`HostValidation::allows_missing_origin`]. A missing **Host** still
//! fails closed on every enforcing bind (HTTP/1.1 mandates it).
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
//! - **IP-literal `Host` values (IPv4 or IPv6) are always allowed.** DNS
//!   rebinding — the attack this layer exists for — requires a DNS
//!   *name* the attacker controls; a raw-IP `Host` means the client
//!   addressed the interface directly, e.g. the LAN URLs the startup
//!   banner prints for a bind-all `--host`. Mirrors Vite, and without
//!   it `--host 0.0.0.0` would 403 its own printed
//!   `http://192.168.x.x:port/` URLs by default. **This exemption does
//!   NOT extend to `Origin`** (issue #1770): an Origin is only checked
//!   against the explicit rule set (`match_rules` — the bound IP,
//!   `allowedHosts` entries, and the built-in localhost forms), never
//!   the IP-literal short-circuit. An unrelated IP scanning the LAN and
//!   sending a cross-origin `Origin: http://192.168.x.y` is exactly the
//!   CSRF-style vector the Origin check exists to catch, so it must not
//!   get a free pass just because it's an IP. Dev/preview users add the
//!   IP to `allowedHosts` to re-authorize its Origin; an embed server
//!   (no `allowedHosts` config surface) should bind to a concrete LAN
//!   IP rather than `0.0.0.0` — the bound IP becomes an `Exact` rule
//!   automatically and its Origin is allowed through that rule, not the
//!   removed short-circuit.
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
    /// `false` only for [`HostValidation::disabled`] — every check
    /// short-circuits to "allowed". Every real bind ([`for_bind`]) sets
    /// this `true`, loopback included; see the module docs for why
    /// loopback is no exception (issue #1684).
    ///
    /// [`for_bind`]: HostValidation::for_bind
    enforce: bool,
    /// Drives the 403 body shape (#926 policy: explanatory in Dev,
    /// generic in Preview/Embed).
    mode: ServerMode,
    /// `true` when the server is bound to a loopback interface. The Host
    /// allowlist and the present-Origin allowlist are enforced regardless
    /// (issue #1684), but a **missing** `Origin` on a non-GET request is
    /// allowed on a loopback bind and rejected on a LAN bind — see
    /// [`Self::allows_missing_origin`].
    bind_is_loopback: bool,
    rules: Vec<AllowRule>,
}

impl HostValidation {
    /// Validation that allows everything. For callers that have no bind
    /// address in scope (router-level unit tests, synthetic states).
    pub fn disabled() -> Self {
        Self {
            enforce: false,
            mode: ServerMode::Dev,
            // Irrelevant while `enforce` is false (every check
            // short-circuits before reading it); pick the safe value.
            bind_is_loopback: false,
            rules: Vec::new(),
        }
    }

    /// Build the validation state for a server bound to `bind_ip`.
    ///
    /// Enforcement is always on (issue #1684 — loopback binds are DNS
    /// rebinding targets too); use [`HostValidation::disabled`] for the
    /// explicit opt-out.
    ///
    /// - `bind_ip` joins the always-allowed set as an IP literal (e.g.
    ///   `--host 192.168.1.5` reached as `http://192.168.1.5:3000`; for
    ///   a loopback bind it is `127.0.0.1` / `::1`, which the IP-literal
    ///   allow rule already covers).
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
        // Issue #1684: enforce on every bind, loopback included. A
        // hostile domain can DNS-rebind to 127.0.0.1 as easily as to a
        // LAN address, so a loopback-bound server is readable
        // cross-origin without this check. The localhost forms added to
        // the always-allowed set below keep the default dev loop working
        // with no config. `HostValidation::disabled()` remains the only
        // way to opt a real router out.
        let enforce = true;
        let bind_is_loopback = bind_ip.is_loopback();
        let mut rules = vec![
            // `localhost` AND every `*.localhost` subdomain
            // (`app.localhost`, …). RFC 6761 reserves `.localhost` as
            // special-use: resolvers always map it to loopback, so it is
            // never an attacker-controllable rebinding name — hence safe
            // to allow on every bind. Mirrors Vite's default allowlist,
            // and matters most for the embed server (no `allowedHosts`
            // escape hatch) and subdomain-per-tenant local dev.
            AllowRule::Suffix("localhost".to_string()),
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
            bind_is_loopback,
            rules,
        }
    }

    /// Whether checks are active — `true` for every [`for_bind`] server
    /// (loopback included, issue #1684), `false` only for
    /// [`HostValidation::disabled`].
    ///
    /// [`for_bind`]: HostValidation::for_bind
    pub fn is_enforced(&self) -> bool {
        self.enforce
    }

    /// Whether a **missing** `Origin` header on a non-GET request to a
    /// dynamic-dispatch surface is allowed through (issue #1684).
    ///
    /// A missing `Origin` is allowed on a **loopback** bind and rejected
    /// on a **LAN** bind. Rationale: a browser always attaches `Origin`
    /// to a cross-origin non-GET request, so a *missing* `Origin` can
    /// never be a browser-driven DNS-rebinding/CSRF request — it means a
    /// local, non-browser client (curl, a test script, a localhost
    /// webhook). On a loopback bind every such client is already a
    /// local process with full access, so failing closed buys no
    /// rebinding protection while breaking common dev tooling; on a LAN
    /// bind the same request could come from an untrusted peer, so it
    /// still fails closed. A *present* `Origin` is always checked against
    /// the allowlist regardless of bind (that is the actual rebinding /
    /// cross-origin vector), and a missing **Host** still fails closed on
    /// every enforcing bind.
    pub fn allows_missing_origin(&self) -> bool {
        self.enforce && self.bind_is_loopback
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
        // IP-literal hosts are always allowed (Vite parity — see the
        // module docs): rebinding attacks ride on DNS names, and the
        // bind-all startup banner prints raw-IP LAN URLs that must work
        // without manual `allowedHosts` entries. This short-circuit is
        // `Host`-only — `origin_allowed` deliberately does not share it
        // (issue #1770, see the module docs).
        if host.parse::<IpAddr>().is_ok() {
            return true;
        }
        self.match_rules(&host)
    }

    /// Check an already-normalised host (lowercase, port/brackets
    /// stripped — see [`host_without_port`]) against the configured
    /// `Exact`/`Suffix` rules only. No IP-literal short-circuit: this is
    /// the shared core [`host_allowed`] ORs with the IP-literal rule,
    /// and [`origin_allowed`] uses alone (issue #1770 — an IP-literal
    /// `Origin` must name an explicit rule, unlike an IP-literal
    /// `Host`).
    ///
    /// [`host_allowed`]: Self::host_allowed
    /// [`origin_allowed`]: Self::origin_allowed
    fn match_rules(&self, host: &str) -> bool {
        self.rules.iter().any(|rule| match rule {
            // Beyond the plain string comparison, also compare through
            // `IpAddr` when both sides parse as one: an `allowedHosts`
            // entry like `2001:0db8::1` and a browser-serialized Origin
            // host `2001:db8::1` denote the same address but differ
            // textually (leading zeros, `::` compression). Now that
            // IP-literal Origins require an explicit rule (issue #1770,
            // no more IP-literal short-circuit), this re-authorization
            // path must not depend on the config author having typed the
            // canonical form.
            AllowRule::Exact(e) => {
                *e == host
                    || matches!(
                        (e.parse::<IpAddr>(), host.parse::<IpAddr>()),
                        (Ok(a), Ok(b)) if a == b
                    )
            }
            AllowRule::Suffix(s) => {
                host == *s
                    || (host.len() > s.len()
                        && host.ends_with(s.as_str())
                        && host.as_bytes()[host.len() - s.len() - 1] == b'.')
            }
        })
    }

    /// Check a raw `Origin` header value (`https://example.com:3000`)
    /// against the allowlist's explicit rules. Always `true` when
    /// enforcement is off. `Origin: null` (opaque origins) and
    /// unparseable values fail closed — a sandboxed-iframe POST to a
    /// LAN-exposed dev server is exactly the cross-origin shape this
    /// guards against.
    ///
    /// Unlike [`host_allowed`], this does **not** apply the IP-literal
    /// always-allow short-circuit (issue #1770) — an IP-literal Origin
    /// must match an explicit rule (bound IP, `allowedHosts` entry, or a
    /// built-in localhost form). See the module docs' matching-rules
    /// section for the rationale.
    ///
    /// [`host_allowed`]: Self::host_allowed
    pub fn origin_allowed(&self, origin: &str) -> bool {
        if !self.enforce {
            return true;
        }
        let trimmed = origin.trim();
        let Some((_, rest)) = trimmed.split_once("://") else {
            return false;
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
        let Some(host) = host_without_port(authority) else {
            return false;
        };
        self.match_rules(&host)
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
        // Fail closed on trailing bytes after `]`: only an empty remainder
        // or a `:port` suffix is a well-formed bracketed authority.
        // Accepting `[::1]evil.test` would hand the trailing garbage a free
        // pass through the always-allowed IP-literal rule.
        let after = &rest[end + 1..];
        let valid_port = after
            .strip_prefix(':')
            .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
        if !(after.is_empty() || valid_port) {
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

/// Build the `403 Forbidden` served when a LAN-exposed server receives
/// a request with no `Host` header at all. Non-browser tools (e.g.
/// raw TCP clients, some proxies) may omit it, but a browser never does
/// for HTTP/1.1 — failing closed here is safe.
pub(crate) fn missing_host_forbidden_response(mode: ServerMode) -> Response {
    let detail = "LAN-exposed server requires a Host header; request had none";
    forbidden_response("missing Host header", detail, mode)
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

/// Build the `403 Forbidden` served when a LAN-exposed server receives
/// a non-GET request with no `Origin` header. Browsers always send
/// `Origin` on cross-origin non-GET requests, so absence implies a
/// non-browser client bypassing CORS — fail closed to block it.
pub(crate) fn missing_origin_forbidden_response(mode: ServerMode) -> Response {
    let detail =
        "LAN-exposed server requires an Origin header on non-GET requests; request had none";
    forbidden_response("missing Origin header", detail, mode)
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
/// When `validation` is not enforced — only [`HostValidation::disabled`]
/// now, since every real bind enforces (issue #1684) — the router is
/// returned unchanged, mirroring
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
/// allowlist. The layer only runs on enforcing routers, so a request
/// with no host information at all fails closed (403) — browsers always
/// send `Host`, so absence implies a non-browser client that could
/// bypass the allowlist by omitting it.
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
            // When enforcement is on (LAN-exposed bind), a missing Host
            // header is a protocol violation — browsers always send one.
            // Fail closed so non-browser LAN clients cannot bypass the
            // allowlist by omitting the header entirely.
            if validation.is_enforced() {
                return missing_host_forbidden_response(validation.mode());
            }
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

    #[test]
    fn localhost_subdomains_are_always_allowed() {
        // `*.localhost` is RFC 6761 special-use (always resolves to
        // loopback, never an attacker-controllable rebinding name), so
        // it rides the built-in suffix rule on every bind — Vite parity.
        let v = enforcing(&[]);
        assert!(v.host_allowed("app.localhost"));
        assert!(v.host_allowed("app.localhost:3000"));
        assert!(v.host_allowed("tenant.api.localhost:8080"));
        // A non-boundary suffix must NOT ride the rule.
        assert!(!v.host_allowed("notlocalhost"));
        assert!(!v.host_allowed("evil-localhost"));
        assert!(!v.host_allowed("localhost.evil.test"));
    }

    #[test]
    fn malformed_bracketed_authorities_fail_closed() {
        let v = enforcing(&[]);
        // Trailing garbage after `]` must not ride the IP-literal allow rule.
        assert!(!v.host_allowed("[::1]evil.test"));
        assert!(!v.host_allowed("[2001:db8::1]anything"));
        assert!(!v.host_allowed("[::1]:"));
        assert!(!v.host_allowed("[::1]:80x"));
        assert!(!v.host_allowed("[::1]:8080extra"));
        // Well-formed shapes keep working.
        assert!(v.host_allowed("[::1]:8080"));
        assert!(v.host_allowed("[::1]"));
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

    // --- matcher: IP literals (always allowed) --------------------------

    #[test]
    fn ip_literal_hosts_are_always_allowed_when_enforcing() {
        // Vite parity: raw-IP Hosts can't be DNS-rebound, and the
        // bind-all startup banner prints exactly these URLs.
        let v = enforcing(&[]);
        assert!(v.host_allowed("192.168.1.9"));
        assert!(v.host_allowed("192.168.1.9:3000"));
        assert!(v.host_allowed("[2001:db8::1]"));
        assert!(v.host_allowed("[2001:db8::1]:3000"));
        // Names still need an allowlist entry.
        assert!(!v.host_allowed("evil.test:3000"));
    }

    // --- matcher: IPv6 bracket/port parsing -----------------------------

    #[test]
    fn ipv6_literals_parse_with_or_without_brackets_and_port() {
        // Config entries may be spelled with or without brackets; the
        // Host side may carry a port. (IP literals pass the always-allow
        // rule anyway — this pins the bracket/port normalisation that
        // the rule depends on.)
        let v = enforcing(&["[2001:db8::1]"]);
        assert!(v.host_allowed("[2001:db8::1]"));
        assert!(v.host_allowed("[2001:db8::1]:3000"));
        let v2 = enforcing(&["2001:db8::1"]);
        assert!(v2.host_allowed("[2001:db8::1]:3000"));
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
        let v = HostValidation::for_bind(LAN_IP, Some("mydev.local"), &[], ServerMode::Dev);
        assert!(v.is_enforced());
        assert!(v.host_allowed("192.168.1.5:3000"));
        assert!(v.host_allowed("mydev.local:3000"));
        assert!(!v.host_allowed("evil.test"));
    }

    // --- matcher: loopback bind now enforces (issue #1684) -------------

    #[test]
    fn loopback_bind_enforces_validation() {
        // Issue #1684: loopback binds used to short-circuit to "allowed";
        // they now enforce the same allowlist as a LAN bind (a hostile
        // domain can DNS-rebind to 127.0.0.1). The localhost forms stay
        // allowed via the built-in set + IP-literal rule; a rebinding
        // name and its Origin are rejected.
        for ip in [
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        ] {
            let v = HostValidation::for_bind(ip, None, &[], ServerMode::Dev);
            assert!(v.is_enforced());
            assert!(v.host_allowed("localhost"));
            assert!(v.host_allowed("localhost:3000"));
            assert!(v.host_allowed("127.0.0.1:3000"));
            assert!(v.host_allowed("[::1]:3000"));
            // The DNS-rebinding host the attacker controls is rejected.
            assert!(!v.host_allowed("absolutely.anything.example"));
            assert!(!v.origin_allowed("https://absolutely.anything.example"));
        }
    }

    #[test]
    fn loopback_bind_honors_allowed_hosts_and_bound_host() {
        // The `allowedHosts` escape hatch and the explicitly bound host
        // string work on a loopback bind exactly as on a LAN bind — an
        // /etc/hosts alias pointing a name at 127.0.0.1 is the case the
        // 403 body's `allowedHosts` hint exists for.
        let v = HostValidation::for_bind(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            Some("mydev.local"),
            &["allowed.test".to_string()],
            ServerMode::Dev,
        );
        assert!(v.is_enforced());
        assert!(v.host_allowed("mydev.local:3000"));
        assert!(v.host_allowed("allowed.test:8080"));
        assert!(!v.host_allowed("evil.test"));
    }

    #[test]
    fn allows_missing_origin_only_on_enforcing_loopback_binds() {
        // Loopback bind → a missing Origin on a non-GET request is local
        // tooling and is allowed (issue #1684).
        let loopback =
            HostValidation::for_bind(IpAddr::V4(Ipv4Addr::LOCALHOST), None, &[], ServerMode::Dev);
        assert!(loopback.allows_missing_origin());
        // LAN bind → a missing Origin fails closed.
        let lan = HostValidation::for_bind(LAN_IP, None, &[], ServerMode::Dev);
        assert!(!lan.allows_missing_origin());
        // Disabled → not enforcing at all, so the missing-Origin gate is
        // never reached; the accessor reports false.
        assert!(!HostValidation::disabled().allows_missing_origin());
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
    fn origin_allowed_rejects_unrelated_ip_literal_origins() {
        // Issue #1770: `origin_allowed` shares only `match_rules` with
        // `host_allowed`, not the IP-literal always-allow short-circuit.
        // An unrelated LAN IP scanning the network must not ride a free
        // pass just because it's an IP literal — that's exactly the
        // CSRF-style cross-origin vector this check exists to catch.
        let v = enforcing(&[]);
        assert!(!v.origin_allowed("http://192.168.1.9:3000"));
        assert!(!v.origin_allowed("http://[2001:db8::7]:3000"));
        // Built-in localhost-form rules (`127.0.0.1`, `::1`) still cover
        // their own Origins — those are explicit `Exact` rules, not the
        // IP-literal short-circuit.
        assert!(v.origin_allowed("http://[::1]:3000"));
        assert!(v.origin_allowed("http://127.0.0.1:3000"));
        assert!(!v.origin_allowed("https://evil.test"));
    }

    #[test]
    fn origin_allowed_allows_the_bound_ip_via_its_explicit_rule() {
        // The bound IP is seeded as an `Exact` rule by `for_bind`, so its
        // Origin is allowed through `match_rules` — not the removed
        // IP-literal short-circuit. This is the embed-server remedy: bind
        // to a concrete LAN IP instead of `0.0.0.0` (issue #1770).
        let v = HostValidation::for_bind(LAN_IP, None, &[], ServerMode::Dev);
        assert!(v.origin_allowed("http://192.168.1.5:3000"));
        // An unrelated IP not covered by any rule is still rejected.
        assert!(!v.origin_allowed("http://192.168.1.9:3000"));
    }

    #[test]
    fn allowed_hosts_noncanonical_ipv6_entry_reauthorizes_canonical_origin() {
        // Codex review finding (issue #1770 review pass): a config author
        // may write an IPv6 `allowedHosts` entry in a non-canonical form
        // (leading zeros, no `::` compression) while browsers always
        // serialize the Origin host canonically. `match_rules` must
        // compare through `IpAddr`, not just the raw string, or this
        // re-authorization path silently fails for anything but an
        // exact textual match.
        let v = enforcing(&["2001:0db8:0000:0000:0000:0000:0000:0001"]);
        assert!(v.origin_allowed("http://[2001:db8::1]:3000"));
        assert!(v.host_allowed("[2001:db8::1]:3000"));
        // An unrelated address is still rejected.
        assert!(!v.origin_allowed("http://[2001:db8::2]:3000"));
    }

    #[test]
    fn allowed_hosts_ip_entry_reauthorizes_its_origin() {
        // NEW (issue #1770): adding an IP literal to `allowedHosts`
        // creates an explicit `Exact` rule, which re-authorizes both its
        // Host (already true before the split) and now its Origin too —
        // the documented dev/preview remedy for the regression this
        // split accepts.
        let v = enforcing(&["192.168.1.9"]);
        assert!(v.host_allowed("192.168.1.9:3000"));
        assert!(v.origin_allowed("http://192.168.1.9:3000"));
        // A sibling IP not in `allowedHosts` keeps failing the Origin
        // check (though it still passes the Host check via the
        // IP-literal short-circuit).
        assert!(v.host_allowed("192.168.1.10:3000"));
        assert!(!v.origin_allowed("http://192.168.1.10:3000"));
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
        let v =
            HostValidation::for_bind(ANY_IP, None, &["allowed.test".to_string()], ServerMode::Dev);
        let router = apply_host_validation_layer(test_router(), v);
        let (status, body) = status_for(router, Some("allowed.test:3000")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "ok");
    }

    #[tokio::test]
    async fn layer_rejects_requests_without_host_header_when_enforcing() {
        // When the server is LAN-exposed (enforcing), a missing Host
        // header must 403 — browsers always send one, so absence implies
        // a non-browser client that could bypass the allowlist check.
        let v = HostValidation::for_bind(ANY_IP, None, &[], ServerMode::Dev);
        let router = apply_host_validation_layer(test_router(), v);
        let (status, body) = status_for(router, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("missing Host header"), "body: {body}");
    }

    #[tokio::test]
    async fn layer_rejects_requests_without_host_header_on_loopback_bind() {
        // Issue #1684: a loopback bind now enforces, so a missing Host
        // header fails closed (403) exactly as a LAN bind does — no more
        // free pass for the default `localhost` case.
        let v = HostValidation::for_bind(
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            None,
            &[],
            ServerMode::Dev,
        );
        let router = apply_host_validation_layer(test_router(), v);
        let (status, body) = status_for(router, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("missing Host header"), "body: {body}");
    }

    #[tokio::test]
    async fn layer_allows_requests_without_host_header_when_disabled() {
        // The only non-enforcing path left (issue #1684): an explicit
        // `disabled()` validation. The layer is a no-op, so a missing
        // Host is allowed through.
        let router = apply_host_validation_layer(test_router(), HostValidation::disabled());
        let (status, _) = status_for(router, None).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn layer_is_noop_when_disabled() {
        let router = apply_host_validation_layer(test_router(), HostValidation::disabled());
        let (status, _) = status_for(router, Some("evil.test")).await;
        assert_eq!(status, StatusCode::OK);
    }
}
