//! Numeric limits for the embedded V8 host's request-time capabilities
//! (issue #2015, epic #2012).
//!
//! **This module is the single source of truth.** The contract
//! (`research/2013-request-time-capability-contract.md`, "Numeric
//! constants — one source of truth") requires the JS polyfill to read
//! these values back out of Rust via `globalThis.__zfb.limits` rather
//! than hardcoding a second copy that can drift. [`limits_js_literal`]
//! (issue #2016) renders them for that injection; `js/globals_shim.js`
//! publishes the result and `js/web_polyfills.js` reads it.
//!
//! Every value here is a *decision*, not an inherited default — see the
//! contract's "Deliberate divergences from production workerd" table
//! (rows D1, D2, D9) for why each number is what it is and what would
//! change it.

/// URL schemes the outbound `fetch` transport will open a socket for.
///
/// Loopback and private addresses are deliberately **reachable**
/// (divergence D5): production Cloudflare Workers cannot reach them,
/// but local dev talking to a local API is the point of the feature,
/// and the epic's guardrail 3 mandates loopback test servers.
pub const ALLOWED_FETCH_SCHEMES: &[&str] = &["http", "https"];

/// Maximum number of redirect hops chased under `redirect: "follow"`.
/// The WHATWG Fetch standard's own redirect count; Cloudflare documents
/// no divergence.
pub const MAX_REDIRECTS: u32 = 20;

/// Wall-clock deadline applied to a single `fetch`, in milliseconds.
///
/// Overridable at runtime — and *only* at host boot — via
/// [`ZFB_SSR_FETCH_TIMEOUT_MS_ENV`]; see [`fetch_timeout_ms`].
pub const DEFAULT_FETCH_TIMEOUT_MS: u64 = 30_000;

/// Maximum outbound request body size, in bytes (100 MB).
pub const MAX_REQUEST_BODY_BYTES: usize = 104_857_600;

/// Maximum buffered response body size, in bytes (100 MB).
///
/// Enforced by counting bytes **as they stream in**, aborting the
/// connection the moment the cap is crossed — the host buffers whole
/// bodies in memory (no `ReadableStream`, divergence D3), so an
/// unbounded response is an OOM of the developer's machine.
pub const MAX_RESPONSE_BODY_BYTES: usize = 104_857_600;

/// Maximum number of outbound `fetch` calls — **including every
/// redirect hop** — permitted during a single `dispatch_fetch`.
///
/// Anchored on Cloudflare's smallest documented per-invocation
/// subrequest limit (Workers Free = 50) so anything that passes locally
/// fits every plan (divergence D9).
pub const MAX_SUBREQUESTS_PER_DISPATCH: u32 = 50;

/// Byte quota for a single `crypto.getRandomValues` call, measured on
/// `byteLength` rather than element count. Consumed by the Web Crypto
/// wave (#2017), not by the fetch transport — it lives here because the
/// contract names this module as the one source of truth for every
/// request-time limit.
pub const MAX_RANDOM_BYTES_PER_CALL: usize = 65_536;

/// Environment variable that overrides [`DEFAULT_FETCH_TIMEOUT_MS`].
/// The **only** overridable limit in this module.
pub const ZFB_SSR_FETCH_TIMEOUT_MS_ENV: &str = "ZFB_SSR_FETCH_TIMEOUT_MS";

/// Resolve the per-`fetch` wall-clock deadline, in milliseconds.
///
/// Read from [`ZFB_SSR_FETCH_TIMEOUT_MS_ENV`] **once at host boot** and
/// memoised: a mid-run environment mutation must not make two fetches in
/// the same render disagree about their deadline. A non-numeric or `0`
/// value is ignored with a warning on stderr and
/// [`DEFAULT_FETCH_TIMEOUT_MS`] stands.
pub fn fetch_timeout_ms() -> u64 {
    use std::sync::OnceLock;
    static RESOLVED: OnceLock<u64> = OnceLock::new();
    *RESOLVED.get_or_init(|| {
        let raw = match std::env::var(ZFB_SSR_FETCH_TIMEOUT_MS_ENV) {
            Ok(raw) => raw,
            Err(_) => return DEFAULT_FETCH_TIMEOUT_MS,
        };
        match resolve_fetch_timeout_ms(Some(raw.as_str())) {
            Ok(ms) => ms,
            Err(reason) => {
                eprintln!(
                    "[zfb-render] warning: ignoring {ZFB_SSR_FETCH_TIMEOUT_MS_ENV}={raw:?} \
                     ({reason}); using the default {DEFAULT_FETCH_TIMEOUT_MS}ms"
                );
                DEFAULT_FETCH_TIMEOUT_MS
            }
        }
    })
}

/// Render every constant above as a JSON object literal, for
/// substitution into `js/globals_shim.js` at host boot (issue #2016).
///
/// This function exists so the JS side never carries a second copy of
/// these numbers — a hardcoded duplicate in JS is a **rejected design**
/// (contract, "Numeric constants — one source of truth"), because it
/// can drift from Rust silently while every test still passes. Keys are
/// the camelCase spellings of the constant names;
/// `js_visible_limits_match_the_rust_constants` in
/// `embedded_v8/js_fetch_tests.rs` reads them back out of a live
/// isolate and compares against the constants themselves.
///
/// The resolved-at-boot deadline ([`fetch_timeout_ms`]) is deliberately
/// NOT included: it is an environment-dependent value, not a constant,
/// and the JS layer never needs it — every wall-clock deadline is
/// enforced in Rust.
pub fn limits_js_literal() -> String {
    serde_json::json!({
        "allowedFetchSchemes": ALLOWED_FETCH_SCHEMES,
        "maxRedirects": MAX_REDIRECTS,
        "defaultFetchTimeoutMs": DEFAULT_FETCH_TIMEOUT_MS,
        "maxRequestBodyBytes": MAX_REQUEST_BODY_BYTES,
        "maxResponseBodyBytes": MAX_RESPONSE_BODY_BYTES,
        "maxSubrequestsPerDispatch": MAX_SUBREQUESTS_PER_DISPATCH,
        "maxRandomBytesPerCall": MAX_RANDOM_BYTES_PER_CALL,
    })
    .to_string()
}

/// Pure core of [`fetch_timeout_ms`], split out so the parse and
/// rejection rules are testable without mutating the process
/// environment (which would race every other test in the binary).
///
/// `Err(reason)` means "ignore this value and warn"; the reason string
/// is what the warning quotes.
pub(crate) fn resolve_fetch_timeout_ms(raw: Option<&str>) -> std::result::Result<u64, String> {
    let Some(raw) = raw else {
        return Ok(DEFAULT_FETCH_TIMEOUT_MS);
    };
    let trimmed = raw.trim();
    let parsed: u64 = trimmed
        .parse()
        .map_err(|_| format!("not a non-negative integer: {trimmed:?}"))?;
    if parsed == 0 {
        return Err("must be greater than zero".to_string());
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The contract's constants table is the acceptance criterion for
    /// this module. Pin every value: #2016 injects them into JS, and a
    /// silent edit here would change request-time behaviour with no
    /// other test noticing.
    #[test]
    fn constants_match_the_2013_contract_table() {
        assert_eq!(ALLOWED_FETCH_SCHEMES, &["http", "https"]);
        assert_eq!(MAX_REDIRECTS, 20);
        assert_eq!(DEFAULT_FETCH_TIMEOUT_MS, 30_000);
        assert_eq!(MAX_REQUEST_BODY_BYTES, 104_857_600);
        assert_eq!(MAX_RESPONSE_BODY_BYTES, 104_857_600);
        assert_eq!(MAX_SUBREQUESTS_PER_DISPATCH, 50);
        assert_eq!(MAX_RANDOM_BYTES_PER_CALL, 65_536);
    }

    /// The injected literal must carry every constant, under the exact
    /// key `web_polyfills.js` reads. A missing key would surface in JS
    /// as `undefined`, which compares false against every numeric
    /// limit — i.e. a silently disabled JS-side check.
    #[test]
    fn the_js_literal_carries_every_constant_under_its_camel_case_key() {
        let parsed: serde_json::Value =
            serde_json::from_str(&limits_js_literal()).expect("the literal is valid JSON");
        assert_eq!(
            parsed["allowedFetchSchemes"],
            serde_json::json!(["http", "https"])
        );
        assert_eq!(parsed["maxRedirects"], serde_json::json!(MAX_REDIRECTS));
        assert_eq!(
            parsed["defaultFetchTimeoutMs"],
            serde_json::json!(DEFAULT_FETCH_TIMEOUT_MS)
        );
        assert_eq!(
            parsed["maxRequestBodyBytes"],
            serde_json::json!(MAX_REQUEST_BODY_BYTES)
        );
        assert_eq!(
            parsed["maxResponseBodyBytes"],
            serde_json::json!(MAX_RESPONSE_BODY_BYTES)
        );
        assert_eq!(
            parsed["maxSubrequestsPerDispatch"],
            serde_json::json!(MAX_SUBREQUESTS_PER_DISPATCH)
        );
        assert_eq!(
            parsed["maxRandomBytesPerCall"],
            serde_json::json!(MAX_RANDOM_BYTES_PER_CALL)
        );
    }

    #[test]
    fn absent_override_yields_the_default() {
        assert_eq!(resolve_fetch_timeout_ms(None), Ok(DEFAULT_FETCH_TIMEOUT_MS));
    }

    #[test]
    fn a_positive_integer_override_is_honoured() {
        assert_eq!(resolve_fetch_timeout_ms(Some("1500")), Ok(1500));
        assert_eq!(resolve_fetch_timeout_ms(Some("  1500  ")), Ok(1500));
    }

    #[test]
    fn non_numeric_and_zero_overrides_are_rejected_so_the_default_stands() {
        assert!(resolve_fetch_timeout_ms(Some("soon")).is_err());
        assert!(resolve_fetch_timeout_ms(Some("")).is_err());
        assert!(resolve_fetch_timeout_ms(Some("-1")).is_err());
        assert!(resolve_fetch_timeout_ms(Some("1.5")).is_err());
        assert!(resolve_fetch_timeout_ms(Some("0")).is_err());
    }
}
