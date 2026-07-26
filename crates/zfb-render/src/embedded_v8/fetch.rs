//! Asynchronous outbound-HTTP transport for the embedded V8 host
//! (issue #2015, epic #2012 — #1750 part 1 of 2).
//!
//! This is the **Rust transport layer only**. Nothing in
//! `js/web_polyfills.js` calls [`op_zfb_fetch`] yet; sub-issue #2016
//! adapts the JS `fetch` polyfill onto it. Everything the contract
//! (`research/2013-request-time-capability-contract.md`) says is
//! "enforced in Rust" is enforced here: the scheme allowlist, the
//! redirect limit and method-rewriting rules, the wall-clock timeout,
//! the request/response body caps, and the per-dispatch subrequest
//! counter.
//!
//! ## Why the op is asynchronous
//!
//! Guardrail 1 of the epic: **the isolate thread must never park on a
//! socket.** [`op_zfb_fetch`] is declared with `#[op2]` on an `async fn`
//! — deno_core's async-op form — so the returned future is polled by
//! `deno_core`'s event loop instead of blocking the thread that owns
//! the V8 isolate. One hung upstream would otherwise stall every
//! concurrent render on the shared host
//! (`RendererState::embedded_v8_host_mut`).
//!
//! Spelling note (deviation from the letter of the #2013 contract, not
//! its substance): the contract writes `#[op2(async)]`. In the pinned
//! `deno_core 0.399` / `deno_ops 0.275`, `async` is only accepted as a
//! *list* flag (`async(lazy)` / `async(deferred)` / `async(fake)`);
//! bare `#[op2(async)]` fails to parse. The default async form — `#[op2]`
//! on an `async fn` — is exactly the eagerly-polled, event-loop-driven
//! op the contract is asking for. [`op_is_async`] asserts the property
//! mechanically off `OpDecl::is_async` so the spelling cannot silently
//! regress to a synchronous op.
//!
//! ## Transport choice
//!
//! The workspace's existing `reqwest 0.12` with `rustls-tls`, in
//! **non-blocking** mode, on the host's current-thread tokio runtime.
//! Deliberately not `deno_fetch` — see the "Why a polyfill instead of
//! deno_fetch/deno_web" block in this crate's `Cargo.toml`. Automatic
//! decompression is off (none of reqwest's `gzip`/`brotli`/`deflate`/
//! `zstd` features are enabled anywhere in this workspace), so reqwest
//! sends no `accept-encoding` of its own and response bytes are
//! surfaced verbatim — divergence D6, and what keeps `content-length`
//! honest against the response cap.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use deno_core::{op2, CancelFuture, CancelHandle, JsBuffer, OpState};
use deno_error::JsErrorBox;
use serde::{Deserialize, Serialize};

use super::limits;
use crate::dispatch_mode::DispatchMode;

/// Headers the transport owns: dropped from the caller's list and
/// recomputed by the HTTP stack. Silently, not as an error — this
/// mirrors `Headers`' forbidden-header-name behaviour.
///
/// `content-length` is in this list because reqwest derives it from the
/// body we hand it; a caller-supplied value that disagrees would frame
/// the request wrong.
const HOP_BY_HOP_HEADERS: &[&str] = &[
    "host",
    "connection",
    "transfer-encoding",
    "content-length",
    "upgrade",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
];

/// Headers dropped when a redirect rewrites the method to `GET` and
/// discards the body — they describe a payload that no longer exists.
const BODY_DESCRIBING_HEADERS: &[&str] = &[
    "content-length",
    "content-type",
    "content-encoding",
    "content-language",
];

/// Headers stripped when a redirect crosses to a different origin.
const CROSS_ORIGIN_SENSITIVE_HEADERS: &[&str] = &["authorization", "cookie", "proxy-authorization"];

/// The **exact** set of statuses the Fetch standard treats as
/// redirects.
///
/// Every other 3xx — notably `300 Multiple Choices` and
/// `304 Not Modified` — is an ordinary response: returned unchanged in
/// all three redirect modes, never chased, and never rejected by
/// `redirect: "error"`. Treating "any 3xx" as a redirect is the classic
/// bug in this code; `redirect_statuses_are_exactly_the_five` pins it.
const REDIRECT_STATUSES: &[u16] = &[301, 302, 303, 307, 308];

fn is_redirect_status(status: u16) -> bool {
    REDIRECT_STATUSES.contains(&status)
}

/// How the caller wants redirect statuses handled. Mirrors the Fetch
/// standard's `RequestRedirect`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RedirectMode {
    /// Chase up to [`limits::MAX_REDIRECTS`] hops. The default.
    #[default]
    Follow,
    /// Return the redirect response unchanged, `redirected = false`.
    Manual,
    /// A redirect status is a network error.
    Error,
}

/// Tunable limits for one [`perform_fetch`] call.
///
/// Split out from [`limits`] so tests can drive the cap paths with
/// small, fast values while a separate test pins that
/// [`FetchConfig::default`] carries the contract's real numbers and
/// that the rendered error messages quote them.
#[derive(Debug, Clone, Copy)]
pub struct FetchConfig {
    /// Wall-clock deadline for the whole call, including every redirect
    /// hop and the streamed body read.
    pub timeout_ms: u64,
    pub max_redirects: u32,
    pub max_request_body_bytes: usize,
    pub max_response_body_bytes: usize,
    pub max_subrequests: u32,
}

impl Default for FetchConfig {
    fn default() -> Self {
        Self {
            timeout_ms: limits::fetch_timeout_ms(),
            max_redirects: limits::MAX_REDIRECTS,
            max_request_body_bytes: limits::MAX_REQUEST_BODY_BYTES,
            max_response_body_bytes: limits::MAX_RESPONSE_BODY_BYTES,
            max_subrequests: limits::MAX_SUBREQUESTS_PER_DISPATCH,
        }
    }
}

/// Per-dispatch subrequest budget.
///
/// Lives in Rust — inside the host's `OpState` in production — so a
/// `Promise.all` fan-out in bundle code cannot evade it by never
/// yielding to a JS-side counter. **Every redirect hop claims a slot**,
/// matching Cloudflare, where each hop in a chain is its own subrequest.
///
/// ## One counter per dispatch, allocated fresh — never zeroed in place
///
/// [`super::EmbeddedV8RenderHost::begin_dispatch_subrequest_budget`]
/// installs a **brand-new** counter in `OpState` at the start of each
/// dispatch rather than resetting the existing one, and
/// [`state_handles`] clones the `Rc` at op entry.
///
/// That distinction is load-bearing. A handler can start a `fetch`
/// without awaiting it and still return a `Response`, at which point
/// `with_event_loop_promise` finishes while the op is still pending.
/// With a single zeroed-in-place counter, that orphan's remaining
/// redirect hops would spend the **next** dispatch's budget — and its
/// own overspend would be forgiven by the reset. Allocating instead
/// means the orphan keeps charging the counter it started on, which is
/// exactly the dispatch it belongs to, and the incoming dispatch gets a
/// budget nothing else holds a handle to.
///
/// `Cell` rather than `AtomicU32`: the host is `!Send + !Sync` by
/// construction (V8 isolates are thread-pinned), so there is no
/// cross-thread contention to guard against.
#[derive(Debug, Default)]
pub struct SubrequestCounter {
    used: Cell<u32>,
}

impl SubrequestCounter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Subrequests consumed against this counter.
    pub fn used(&self) -> u32 {
        self.used.get()
    }

    /// Consume one slot, or fail if the budget is exhausted. `url` is
    /// the *originally requested* URL, so a chain that overflows on hop
    /// 40 still names the fetch the caller wrote.
    pub(crate) fn claim(&self, url: &str, limit: u32) -> Result<(), FetchError> {
        if self.used.get() >= limit {
            return Err(FetchError::SubrequestLimit {
                url: url.to_string(),
                limit,
            });
        }
        self.used.set(self.used.get() + 1);
        Ok(())
    }
}

/// What the JS side asks the transport to do. The body travels as a
/// separate op argument (see [`op_zfb_fetch`]) rather than a field here,
/// so a 100 MB payload never round-trips through `serde_json`-shaped
/// JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchRequestSpec {
    /// Absolute URL. Relative URLs are the JS side's problem — by the
    /// time the op is reached the polyfill has already resolved against
    /// the base.
    pub url: String,
    /// HTTP method. Uppercased here; the JS side has already rejected
    /// `GET`/`HEAD` with a body.
    pub method: String,
    /// Ordered `[name, value]` pairs, never a map — duplicate request
    /// headers must survive the boundary in both directions.
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub redirect: RedirectMode,
    /// Whether `body` carries a payload at all. Distinguishes
    /// `body: null` from a zero-length body, which the buffer argument
    /// alone cannot.
    #[serde(default)]
    pub has_body: bool,
    /// Caller-requested deadline in milliseconds, from
    /// `AbortSignal.timeout(ms)` on the JS side (issue #2016).
    ///
    /// It can only ever **narrow** [`FetchConfig::timeout_ms`], never
    /// widen it — see [`effective_timeout_ms`]. The host has no event
    /// loop timers (`setTimeout` in `js/web_polyfills.js` is
    /// microtask-backed and ignores its delay), so a JS-side timer
    /// could not honour `AbortSignal.timeout` at all; routing it
    /// through the Rust deadline is what makes the signal real, and it
    /// takes the same cancellation path — the future is dropped, which
    /// closes the socket.
    ///
    /// `u32` rather than `u64` on purpose: `serde_v8` decodes a `u64`
    /// from a JS **BigInt**, not from an ordinary `Number`, so a `u64`
    /// here would reject every value the JS layer can naturally send
    /// with `invalid_argument`. `u32` milliseconds is ~49 days, far
    /// past any deadline that could narrow the host's own — and the JS
    /// side clamps to the range before sending.
    #[serde(default)]
    pub timeout_ms: Option<u32>,
    /// Token the JS side minted for this call so it can cancel it
    /// later through [`op_zfb_fetch_cancel`] (epic #2012 review fix).
    ///
    /// Without it an abort could only settle the caller's promise: the
    /// op's future is owned by `deno_core`'s event loop and JS cannot
    /// drop it, so the transport ran on to the wall-clock deadline with
    /// its subrequest slot spent and its response still buffering. The
    /// id is registered against a [`CancelHandle`] on the op's first
    /// poll — which `deno_core` performs eagerly, inside the `op(...)`
    /// call itself — so any abort that arrives afterwards finds it.
    ///
    /// `None` means "no cancellation channel": the deadline is then the
    /// only thing that can end the call, which is exactly the pre-fix
    /// behaviour and is what a caller passing no `AbortSignal` gets.
    #[serde(default)]
    pub cancel_id: Option<u32>,
}

/// The mode of the dispatch currently running on this host.
///
/// **The trust boundary for guardrail 4 lives here.** `__zfb.mode` and
/// the polyfill's reader are advisory: bundle code shares the realm
/// with them and can reach the raw op regardless. This cell is in
/// `OpState`, where no JS value can point at it, and
/// [`op_zfb_fetch`] consults it before anything else.
///
/// Installed per dispatch by
/// [`super::EmbeddedV8RenderHost::install_dispatch_mode`], and reset to
/// [`DispatchMode::BuildTime`] before any module evaluation. Absent
/// (a runtime built without this extension) reads as `BuildTime`, the
/// denying default.
#[derive(Debug, Clone, Copy, Default)]
pub struct DispatchModeState(pub DispatchMode);

/// In-flight fetches that carry a cancellation token, keyed by the id
/// the JS side minted for them.
///
/// `RefCell<HashMap<..>>` rather than anything atomic for the same
/// reason [`SubrequestCounter`] uses `Cell`: the host is `!Send +
/// !Sync` by construction.
#[derive(Debug, Default)]
pub struct CancelRegistry {
    handles: RefCell<HashMap<u32, Rc<CancelHandle>>>,
}

impl CancelRegistry {
    /// Register a fresh handle for `id` and hand it back. A duplicate
    /// id replaces the older entry — the JS side mints ids from a
    /// monotonic counter, so that can only happen after a wrap, by
    /// which point the older call is long gone.
    pub(crate) fn register(&self, id: u32) -> Rc<CancelHandle> {
        let handle = Rc::new(CancelHandle::new());
        self.handles.borrow_mut().insert(id, handle.clone());
        handle
    }

    pub(crate) fn forget(&self, id: u32) {
        self.handles.borrow_mut().remove(&id);
    }

    /// Cancel the in-flight fetch registered under `id`, if any. An
    /// unknown id is a no-op: the call may have already finished, and a
    /// late abort must never be an error.
    pub(crate) fn cancel(&self, id: u32) {
        if let Some(handle) = self.handles.borrow_mut().remove(&id) {
            handle.cancel();
        }
    }

    /// Number of registered in-flight cancellable fetches. Tests use it
    /// to prove the registry does not leak an entry per call.
    pub fn len(&self) -> usize {
        self.handles.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The deadline actually applied to a call: the host's own, narrowed by
/// any caller-requested one.
///
/// Deliberately a `min`, never an override. A bundle that asks for
/// `AbortSignal.timeout(10 * 60 * 1000)` does **not** get to sit on the
/// single SSR V8 thread for ten minutes — divergence D1 exists because
/// one hung `fetch` wedges the whole dev server, and only the operator
/// (via `ZFB_SSR_FETCH_TIMEOUT_MS`) may raise the ceiling. A requested
/// `0` is treated as "as soon as possible" (1 ms) rather than
/// "no deadline".
pub(crate) fn effective_timeout_ms(config_ms: u64, requested_ms: Option<u64>) -> u64 {
    match requested_ms {
        Some(requested) => config_ms.min(requested.max(1)),
        None => config_ms,
    }
}

/// The materialised response handed back across the host boundary.
///
/// `headers` is an ordered `Array<[name, value]>` and **never a map**:
/// collapsing it would merge repeated `set-cookie` values, which is the
/// one header shape that must survive intact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchOutcome {
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    /// The **final** URL after any followed redirects.
    pub url: String,
    /// `true` when at least one redirect hop was followed.
    pub redirected: bool,
    pub body: Vec<u8>,
}

impl FetchOutcome {
    /// First value for `name` (lowercase), if present.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Every value for `name` (lowercase), in wire order. The accessor
    /// that proves repeated `set-cookie` survived.
    pub fn header_all(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .collect()
    }
}

/// Serde-facing mirror of [`FetchOutcome`] for the op's return value.
/// `body` becomes a `Uint8Array` on the JS side via
/// [`deno_core::ToJsBuffer`].
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchResponseSpec {
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    pub url: String,
    pub redirected: bool,
    pub body: deno_core::ToJsBuffer,
}

impl From<FetchOutcome> for FetchResponseSpec {
    fn from(outcome: FetchOutcome) -> Self {
        Self {
            status: outcome.status,
            status_text: outcome.status_text,
            headers: outcome.headers,
            url: outcome.url,
            redirected: outcome.redirected,
            body: outcome.body.into(),
        }
    }
}

/// Every way the transport can fail.
///
/// The `Display` text of each variant is the **exact** message the
/// contract's error column specifies — `error_messages_match_the_2013_contract`
/// pins all of them, because #2016's JS layer and #2018's diagnostics
/// both quote these strings.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    /// Scheme outside [`limits::ALLOWED_FETCH_SCHEMES`], checked before
    /// any socket is opened. The message shape is workerd's / Chromium's.
    #[error("Fetch API cannot load: {url}")]
    DisallowedScheme { url: String },
    /// URL the `url` crate cannot parse. The contract has no row for
    /// this; it shares the scheme rejection's message because both mean
    /// "this is not a URL this host will load", and that is the shape
    /// workerd emits for a malformed URL too.
    #[error("Fetch API cannot load: {url}")]
    InvalidUrl { url: String },
    #[error("fetch({url}): too many redirects (limit {limit})")]
    TooManyRedirects { url: String, limit: u32 },
    #[error("fetch({url}): redirect not allowed (redirect mode is \"error\")")]
    RedirectNotAllowed { url: String },
    #[error("fetch({url}): request body exceeds the {limit}-byte limit")]
    RequestBodyTooLarge { url: String, limit: usize },
    #[error("fetch({url}): response body exceeds the {limit}-byte limit")]
    ResponseBodyTooLarge { url: String, limit: usize },
    #[error(
        "fetch({url}): timed out after {timeout_ms}ms (zfb embedded-runtime request-time limit; \
         production Cloudflare Workers has no per-subrequest timeout)"
    )]
    Timeout { url: String, timeout_ms: u64 },
    #[error("fetch({url}): exceeded the {limit}-subrequest limit for a single request")]
    SubrequestLimit { url: String, limit: u32 },
    /// Any DNS, TCP, or TLS failure. `cause` is the transport's own
    /// message.
    #[error("fetch({url}): {cause}")]
    Transport { url: String, cause: String },
    /// Build-time render asked for a socket. **This is the enforcement
    /// point for guardrail 4 of epic #2012**, not the JS polyfill's
    /// matching rejection: `globalThis.Deno.core.ops.op_zfb_fetch` is
    /// reachable from bundle code, so a denial that lives only in
    /// `js/web_polyfills.js` is a denial the bundle can walk around.
    ///
    /// The wording is byte-identical to the polyfill's own build-time
    /// rejection on purpose — the developer sees one message for one
    /// policy, whichever layer caught it.
    #[error(
        "fetch() called from SSG runtime (url={url}). The embedded V8 host does not support \
         outgoing network requests during build-time render. Move the data fetch to a build \
         step or a runtime-only branch."
    )]
    BuildTimeDenied { url: String },
    /// The caller's `AbortSignal` fired while the request was in
    /// flight, so the transport future was dropped and the socket
    /// closed (contract row "Abort").
    ///
    /// The JS side has normally already rejected the caller's promise
    /// with the signal's own reason by the time this arrives, and
    /// discards it; the variant exists so the op resolves rather than
    /// hanging, and so a cancellation is never mistaken for a
    /// transport failure in a log.
    #[error("fetch({url}): aborted by the caller's AbortSignal")]
    Aborted { url: String },
    /// The op itself could not run — the shared client is missing from
    /// `OpState` because the runtime is shutting down or was built
    /// without the extension. **Never** resolved into a synthetic empty
    /// `Response`: a silent empty body is indistinguishable from a real
    /// `200` with no content, which is precisely the dev/prod divergence
    /// this epic exists to remove.
    #[error("fetch({url}): embedded host transport unavailable: {detail}")]
    HostUnavailable { url: String, detail: String },
}

impl FetchError {
    /// The JS error class this maps to. Everything is a `TypeError` (a
    /// Fetch "network error") except the deadline, which the contract
    /// gives `name = "TimeoutError"` so `err.name` checks behave as they
    /// would against a real `DOMException` (divergence D4).
    pub fn js_error_class(&self) -> &'static str {
        match self {
            FetchError::Timeout { .. } => "TimeoutError",
            _ => "TypeError",
        }
    }

    fn into_js_error_box(self) -> JsErrorBox {
        JsErrorBox::new(self.js_error_class(), self.to_string())
    }
}

/// Render a transport error and every error in its `source` chain.
///
/// reqwest's own `Display` for a connection failure is often just
/// "error sending request for url (...)" with the actionable detail
/// ("Connection refused") one `source()` down, so a bare `{e}` would
/// throw away the part a developer needs.
fn transport_cause(err: &reqwest::Error) -> String {
    let mut parts = vec![err.to_string()];
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(err);
    while let Some(cur) = source {
        parts.push(cur.to_string());
        source = cur.source();
    }
    parts.join(": ")
}

/// Build the shared non-blocking client.
///
/// `redirect::Policy::none()` is load-bearing: the transport chases
/// redirects **itself** so it can count every hop against the subrequest
/// budget and apply the standard's method-rewriting and
/// header-stripping rules, none of which reqwest's own policy exposes.
pub fn build_fetch_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .use_rustls_tls()
        .build()
        .map_err(|e| e.to_string())
}

/// The origin tuple redirects are compared against for auth stripping.
fn origin_of(url: &reqwest::Url) -> (String, Option<String>, Option<u16>) {
    (
        url.scheme().to_string(),
        url.host_str().map(|h| h.to_ascii_lowercase()),
        url.port_or_known_default(),
    )
}

/// Parse and scheme-check a URL **before any socket is opened**.
fn parse_and_check_scheme(raw: &str, requested_url: &str) -> Result<reqwest::Url, FetchError> {
    let parsed = reqwest::Url::parse(raw).map_err(|_| FetchError::InvalidUrl {
        url: requested_url.to_string(),
    })?;
    if !limits::ALLOWED_FETCH_SCHEMES.contains(&parsed.scheme()) {
        // Name the URL the *caller* asked for, so a disallowed scheme
        // reached via a redirect still points at the fetch they wrote.
        return Err(FetchError::DisallowedScheme {
            url: requested_url.to_string(),
        });
    }
    Ok(parsed)
}

/// Apply the standard's redirect method-rewriting rules.
///
/// Returns the method for the next hop and whether the body is dropped:
///
/// | status | method | result |
/// | --- | --- | --- |
/// | 303 | anything but `GET`/`HEAD` | `GET`, body dropped |
/// | 303 | `GET`/`HEAD` | preserved |
/// | 301/302 | `POST` | `GET`, body dropped |
/// | 301/302 | anything else | preserved |
/// | 307/308 | anything | preserved |
fn rewrite_method_for_redirect(status: u16, method: &reqwest::Method) -> (reqwest::Method, bool) {
    let is_get_or_head = *method == reqwest::Method::GET || *method == reqwest::Method::HEAD;
    match status {
        303 if !is_get_or_head => (reqwest::Method::GET, true),
        301 | 302 if *method == reqwest::Method::POST => (reqwest::Method::GET, true),
        _ => (method.clone(), false),
    }
}

/// Issue `spec` and return the materialised response.
///
/// This is the testable core: [`op_zfb_fetch`] is a thin wrapper that
/// resolves the client and counter out of `OpState` and calls this. The
/// whole call — every redirect hop and the streamed body read — sits
/// inside one wall-clock deadline; the timeout **drops the in-flight
/// future**, which is what closes the socket.
pub async fn perform_fetch(
    client: &reqwest::Client,
    counter: &SubrequestCounter,
    config: &FetchConfig,
    spec: &FetchRequestSpec,
    body: Vec<u8>,
) -> Result<FetchOutcome, FetchError> {
    let timeout_ms = effective_timeout_ms(config.timeout_ms, spec.timeout_ms.map(u64::from));
    let deadline = Duration::from_millis(timeout_ms);
    match tokio::time::timeout(
        deadline,
        perform_fetch_inner(client, counter, config, spec, body),
    )
    .await
    {
        Ok(result) => result,
        // Dropping `perform_fetch_inner`'s future here cancels the
        // in-flight reqwest request and closes the socket — the same
        // cancellation path a caller-supplied abort signal takes when it
        // drops the op's future first. Whichever fires first wins.
        Err(_elapsed) => Err(FetchError::Timeout {
            url: spec.url.clone(),
            timeout_ms,
        }),
    }
}

async fn perform_fetch_inner(
    client: &reqwest::Client,
    counter: &SubrequestCounter,
    config: &FetchConfig,
    spec: &FetchRequestSpec,
    body: Vec<u8>,
) -> Result<FetchOutcome, FetchError> {
    let requested_url = spec.url.as_str();

    // Request-body cap first: rejecting before the URL is even resolved
    // keeps an oversized payload from touching the network at all. (The
    // JS layer checks this too — defence in depth per the contract.)
    if spec.has_body && body.len() > config.max_request_body_bytes {
        return Err(FetchError::RequestBodyTooLarge {
            url: requested_url.to_string(),
            limit: config.max_request_body_bytes,
        });
    }

    let mut current_url = parse_and_check_scheme(requested_url, requested_url)?;
    let mut current_method =
        reqwest::Method::from_bytes(spec.method.to_ascii_uppercase().as_bytes()).map_err(|e| {
            FetchError::Transport {
                url: requested_url.to_string(),
                cause: format!("invalid HTTP method {:?}: {e}", spec.method),
            }
        })?;
    let mut current_headers = sanitized_request_headers(&spec.headers, requested_url)?;
    let mut current_body = if spec.has_body { Some(body) } else { None };
    let mut hops: u32 = 0;

    loop {
        counter.claim(requested_url, config.max_subrequests)?;

        let mut builder = client
            .request(current_method.clone(), current_url.clone())
            .headers(current_headers.clone());
        if let Some(bytes) = &current_body {
            builder = builder.body(bytes.clone());
        }
        let response = builder.send().await.map_err(|e| FetchError::Transport {
            url: requested_url.to_string(),
            cause: transport_cause(&e),
        })?;

        let status = response.status().as_u16();
        if is_redirect_status(status) {
            match spec.redirect {
                RedirectMode::Error => {
                    return Err(FetchError::RedirectNotAllowed {
                        url: requested_url.to_string(),
                    })
                }
                RedirectMode::Manual => {
                    // Returned unchanged, and explicitly NOT marked
                    // `redirected` — no hop was followed.
                    return read_response(
                        response,
                        current_url.as_str(),
                        false,
                        requested_url,
                        config,
                    )
                    .await;
                }
                RedirectMode::Follow => {}
            }

            // A redirect status with no usable `Location` is not a
            // redirect the standard can chase; it becomes an ordinary
            // response.
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let Some(location) = location else {
                return read_response(
                    response,
                    current_url.as_str(),
                    hops > 0,
                    requested_url,
                    config,
                )
                .await;
            };

            if hops >= config.max_redirects {
                return Err(FetchError::TooManyRedirects {
                    url: requested_url.to_string(),
                    limit: config.max_redirects,
                });
            }

            let next_url_raw = current_url
                .join(&location)
                .map_err(|_| FetchError::InvalidUrl {
                    url: requested_url.to_string(),
                })?;
            let next_url = parse_and_check_scheme(next_url_raw.as_str(), requested_url)?;

            let (next_method, drop_body) = rewrite_method_for_redirect(status, &current_method);
            if drop_body {
                current_body = None;
                for name in BODY_DESCRIBING_HEADERS {
                    current_headers.remove(*name);
                }
            }
            if origin_of(&current_url) != origin_of(&next_url) {
                for name in CROSS_ORIGIN_SENSITIVE_HEADERS {
                    current_headers.remove(*name);
                }
            }
            current_method = next_method;
            current_url = next_url;
            hops += 1;
            continue;
        }

        // Every non-redirect status — including 300 and 304 — is an
        // ordinary response.
        return read_response(
            response,
            current_url.as_str(),
            hops > 0,
            requested_url,
            config,
        )
        .await;
    }
}

/// Drop hop-by-hop names and build the outbound `HeaderMap`.
fn sanitized_request_headers(
    headers: &[(String, String)],
    requested_url: &str,
) -> Result<reqwest::header::HeaderMap, FetchError> {
    let mut map = reqwest::header::HeaderMap::new();
    for (name, value) in headers {
        let lower = name.to_ascii_lowercase();
        if HOP_BY_HOP_HEADERS.contains(&lower.as_str()) {
            continue;
        }
        let header_name =
            reqwest::header::HeaderName::from_bytes(lower.as_bytes()).map_err(|e| {
                FetchError::Transport {
                    url: requested_url.to_string(),
                    cause: format!("invalid header name {name:?}: {e}"),
                }
            })?;
        let header_value =
            reqwest::header::HeaderValue::from_str(value).map_err(|e| FetchError::Transport {
                url: requested_url.to_string(),
                cause: format!("invalid value for header {name:?}: {e}"),
            })?;
        map.append(header_name, header_value);
    }
    Ok(map)
}

/// Buffer the response body, enforcing the cap **as bytes arrive**.
async fn read_response(
    mut response: reqwest::Response,
    final_url: &str,
    redirected: bool,
    requested_url: &str,
    config: &FetchConfig,
) -> Result<FetchOutcome, FetchError> {
    let status = response.status();
    // Known limitation, deliberately not worked around: this is the
    // CANONICAL reason phrase for the status code, not the bytes the
    // server actually sent. hyper discards the HTTP/1 reason phrase
    // during parsing and reqwest exposes no accessor for it, so a
    // custom `200 Wibble` surfaces as `"OK"`, and an HTTP/2 response —
    // which carries no reason phrase at all — surfaces as `"OK"` rather
    // than the empty string a browser reports. Recovering the real
    // phrase would mean replacing the HTTP stack, which #2015
    // explicitly forbids; emitting `""` for everything instead would
    // lose the correct value in the overwhelmingly common case. The
    // #2013 contract's response row asks for `statusText` to be
    // surfaced and does not specify reason-phrase fidelity.
    let status_text = status.canonical_reason().unwrap_or("").to_string();
    let headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_ascii_lowercase(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();

    // A declared `content-length` above the cap is rejected before a
    // single body byte is read. Dropping `response` here closes the
    // connection.
    if let Some(declared) = response.content_length() {
        if declared > config.max_response_body_bytes as u64 {
            return Err(FetchError::ResponseBodyTooLarge {
                url: requested_url.to_string(),
                limit: config.max_response_body_bytes,
            });
        }
    }

    let mut body = Vec::new();
    // Streamed, chunk by chunk — NOT `response.bytes()`, which would
    // buffer the whole payload before anyone could object to its size.
    // The moment the running total crosses the cap we return, dropping
    // `response` and with it the connection. This is the
    // resource-exhaustion guard (guardrail 6): a hostile or broken
    // upstream cannot make the host allocate past the cap.
    loop {
        let chunk = response.chunk().await.map_err(|e| FetchError::Transport {
            url: requested_url.to_string(),
            cause: transport_cause(&e),
        })?;
        let Some(chunk) = chunk else { break };
        if body.len() + chunk.len() > config.max_response_body_bytes {
            return Err(FetchError::ResponseBodyTooLarge {
                url: requested_url.to_string(),
                limit: config.max_response_body_bytes,
            });
        }
        body.extend_from_slice(&chunk);
    }

    Ok(FetchOutcome {
        status: status.as_u16(),
        status_text,
        headers,
        url: final_url.to_string(),
        redirected,
        body,
    })
}

/// The shared non-blocking client, parked in `OpState` at extension
/// init so every op call reuses one connection pool.
pub struct FetchClient(pub reqwest::Client);

/// Resolve the shared client + counter out of `OpState`.
///
/// A missing entry is a **host-op failure** — the runtime is shutting
/// down, or the extension was never installed — and rejects. It must
/// never fall back to a fresh client or a synthetic response.
fn state_handles(
    state: &Rc<std::cell::RefCell<OpState>>,
    url: &str,
) -> Result<(reqwest::Client, Rc<SubrequestCounter>, Rc<CancelRegistry>), FetchError> {
    let state = state.borrow();
    let client = state
        .try_borrow::<FetchClient>()
        .ok_or_else(|| FetchError::HostUnavailable {
            url: url.to_string(),
            detail: "outbound HTTP client is not installed in this runtime".to_string(),
        })?
        .0
        .clone();
    let counter = state
        .try_borrow::<Rc<SubrequestCounter>>()
        .ok_or_else(|| FetchError::HostUnavailable {
            url: url.to_string(),
            detail: "subrequest counter is not installed in this runtime".to_string(),
        })?
        .clone();
    let cancels = state
        .try_borrow::<Rc<CancelRegistry>>()
        .ok_or_else(|| FetchError::HostUnavailable {
            url: url.to_string(),
            detail: "cancellation registry is not installed in this runtime".to_string(),
        })?
        .clone();
    Ok((client, counter, cancels))
}

/// The mode of the dispatch currently on the isolate, read out of
/// `OpState`. Absent means [`DispatchMode::BuildTime`] — the denying
/// default, so a runtime that never installed the state cannot become
/// the one that grants network access.
fn current_dispatch_mode(state: &Rc<std::cell::RefCell<OpState>>) -> DispatchMode {
    state
        .borrow()
        .try_borrow::<DispatchModeState>()
        .map(|m| m.0)
        .unwrap_or_default()
}

/// Issue one outbound HTTP request. **Asynchronous by construction** —
/// see this module's header for why that is non-negotiable.
///
/// `body` is a `Uint8Array`; `spec.has_body` says whether it means
/// anything (an empty payload and "no payload" are different requests).
///
/// ## This op — not the polyfill — is the trust boundary
///
/// `globalThis.Deno.core.ops.op_zfb_fetch` is reachable from bundle
/// code, so every policy that matters is checked here, before the
/// transport is entered:
///
/// 1. **Build-time denial** (guardrail 4). Consulted first, off
///    [`DispatchModeState`] in `OpState`, which no JS value can reach.
/// 2. **Request-body ceiling**, checked against the still-borrowed
///    `JsBuffer` so an oversized payload is refused *before* it is
///    copied into a `Vec` — `#[buffer(copy)]` would have performed that
///    allocation during argument decoding, which is exactly the
///    allocation the cap exists to prevent.
#[op2]
#[serde]
pub async fn op_zfb_fetch(
    state: Rc<std::cell::RefCell<OpState>>,
    #[serde] spec: FetchRequestSpec,
    #[buffer] body: JsBuffer,
) -> Result<FetchResponseSpec, JsErrorBox> {
    if current_dispatch_mode(&state) != DispatchMode::RequestTime {
        return Err(FetchError::BuildTimeDenied {
            url: spec.url.clone(),
        }
        .into_js_error_box());
    }
    let (client, counter, cancels) =
        state_handles(&state, &spec.url).map_err(FetchError::into_js_error_box)?;
    let config = FetchConfig::default();
    if spec.has_body && body.len() > config.max_request_body_bytes {
        return Err(FetchError::RequestBodyTooLarge {
            url: spec.url.clone(),
            limit: config.max_request_body_bytes,
        }
        .into_js_error_box());
    }
    let body = body.to_vec();

    // Registered on this first poll, which `deno_core` runs eagerly
    // inside the JS `op(...)` call — so the id is live before the
    // caller's `await` can yield, and any later abort finds it.
    let handle = spec.cancel_id.map(|id| cancels.register(id));
    let transport = perform_fetch(&client, &counter, &config, &spec, body);
    let result = match handle {
        Some(handle) => match transport.or_cancel(handle).await {
            Ok(result) => result,
            // Dropping `transport` here is the whole point: it closes
            // the socket instead of letting the request run on to the
            // wall-clock deadline with its result destined for the bin.
            Err(_canceled) => Err(FetchError::Aborted {
                url: spec.url.clone(),
            }),
        },
        None => transport.await,
    };
    if let Some(id) = spec.cancel_id {
        cancels.forget(id);
    }
    Ok(result.map_err(FetchError::into_js_error_box)?.into())
}

/// Cancel the in-flight [`op_zfb_fetch`] registered under `cancel_id`.
///
/// Synchronous and infallible: an abort must take effect on the turn it
/// happens, and an id that has already completed is a no-op rather than
/// an error. It grants no capability — the only thing it can do is stop
/// a request the same isolate started — so it is deliberately NOT
/// mode-gated.
#[op2(fast)]
pub fn op_zfb_fetch_cancel(state: &mut OpState, #[smi] cancel_id: u32) {
    if let Some(cancels) = state.try_borrow::<Rc<CancelRegistry>>() {
        cancels.cancel(cancel_id);
    }
}

deno_core::extension!(
    zfb_fetch,
    ops = [op_zfb_fetch, op_zfb_fetch_cancel],
    state = |state| {
        // A client that cannot be built leaves `FetchClient` absent, so
        // every op call reports the host-op failure above rather than
        // panicking the isolate at boot.
        if let Ok(client) = build_fetch_client() {
            state.put(FetchClient(client));
        }
        state.put(Rc::new(SubrequestCounter::new()));
        state.put(Rc::new(CancelRegistry::default()));
        // Build-time until a dispatch says otherwise. A host that only
        // ever evaluates modules (config eval, `paths()`) therefore
        // never leaves the denying default.
        state.put(DispatchModeState(DispatchMode::BuildTime));
    },
);

/// Whether [`op_zfb_fetch`] is registered as an **async** op.
///
/// Read straight off deno_core's own `OpDecl`, so this cannot be
/// satisfied by a comment or a name: if the op ever became synchronous —
/// the one regression that would put a blocking network call on the
/// isolate thread — this flips to `false`.
pub fn op_is_async() -> bool {
    // `#[op2]` rewrites the annotated fn into a `const fn() -> OpDecl`,
    // so calling it yields the declaration deno_core itself registers.
    op_zfb_fetch().is_async
}

#[cfg(test)]
mod tests;
