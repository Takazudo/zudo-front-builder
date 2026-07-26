//! The #2016 acceptance matrix for the JS `fetch` / `Request` /
//! `Response` adaptation onto the request-time transport.
//!
//! Every case runs **real bundle JS in a real V8 isolate**, dispatched
//! through the production `dispatch_fetch` seam with a real
//! [`DispatchMode`], and — where a socket is involved — against a
//! deterministic loopback server. Guardrail 3 of epic #2012 is binding:
//! never the public internet. Ports come from the OS (`127.0.0.1:0`).
//!
//! Level 3/4 on the zfb ladder. The transport's own behaviour matrix
//! (scheme allowlist, redirect rules, caps, deadline, subrequest
//! budget) is Rust-side and is covered by `fetch/tests.rs`; nothing
//! here re-tests it. What these tests own is the JS adaptation: which
//! branch runs, what each rejection SAYS, and whether the shapes that
//! cross the boundary survive intact.
//!
//! ## The rule this file exists to defend
//!
//! A request-time failure must never report itself as an SSG policy
//! denial. [`the_ssg_denial_message_appears_on_exactly_one_code_path`]
//! pins that mechanically against the polyfill source, and every
//! request-time rejection asserted below is additionally checked for
//! the absence of the SSG wording — a message that merely "looks
//! different" is not enough.
//!
//! Blind spots, stated plainly:
//!
//! - Nothing here exercises the Rust call site that chooses
//!   `RequestTime` in production (`crates/zfb/src/ssr_adapter.rs`);
//!   that lives in `zfb`'s own tests.
//! - A mid-flight abort now genuinely cancels the transport (epic
//!   #2012 review fix 2): the JS abort listener calls
//!   `op_zfb_fetch_cancel`, which drops the op's future and closes the
//!   socket. `an_abort_mid_body_cancels_the_transport_and_closes_the_socket`
//!   asserts BOTH halves — the caller-side rejection and the server
//!   seeing the connection go — with the host deliberately held alive,
//!   since dropping it would close the socket regardless.
//! - `AbortSignal.timeout(ms)` is enforced by the Rust deadline, whose
//!   own coverage is in `fetch/tests.rs`
//!   (`a_caller_requested_deadline_is_the_one_the_transport_enforces`).

use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncWriteExt;

use super::*;
use crate::embedded_v8::loopback_test_server::{ok_response, redirect_response, LoopbackServer};

/// Everything here is bounded, so a regression that makes a fetch hang
/// reports a clear failure instead of running to nextest's
/// `terminate-after`.
const BOUND: Duration = Duration::from_secs(30);

/// The **exact** build-time denial, minus the URL. Guardrail 4: this
/// wording is deliberate policy that survives the epic intact, so it is
/// asserted byte-for-byte rather than by substring.
const SSG_DENIAL_TAIL: &str = "). The embedded V8 host does not support outgoing network requests during build-time render. Move the data fetch to a build step or a runtime-only branch.";

fn expected_ssg_denial(url: &str) -> String {
    format!("fetch() called from SSG runtime (url={url}{SSG_DENIAL_TAIL}")
}

/// The request-time rejection for a `ReadableStream` body, minus the
/// URL — kept as a literal so the assertion cannot be satisfied by a
/// reworded message that merely happens to mention streams.
const STREAM_BODY_TAIL: &str =
    "): ReadableStream request bodies are not supported by the zfb embedded runtime";

/// The transport's own deadline message for a 200 ms limit, minus the
/// `fetch(<url>)` prefix. Asserted verbatim because the whole point of
/// the test that uses it is that the message survives the op boundary
/// — a substring check would pass on a truncated one.
const TRANSPORT_TIMEOUT_TAIL: &str = ": timed out after 200ms (zfb embedded-runtime request-time limit; production Cloudflare Workers has no per-subrequest timeout)";

/// Run `script` as the body of an `async` IIFE inside a bundle's
/// `fetch` handler and return whatever string it produced.
///
/// The handler catches, so a script that throws yields a readable
/// `UNCAUGHT:<name>:<message>` rather than failing the dispatch with a
/// stack trace.
///
/// `pub(crate)` because `js_crypto_tests.rs` (issue #2018) drives the
/// Web Crypto surface through the same seam; keeping one helper means
/// both matrices exercise the identical production dispatch path.
pub(crate) async fn probe(script: &str, mode: DispatchMode) -> String {
    probe_with_limits(script, mode, serde_json::json!({})).await
}

/// [`probe`], but with the host's JS-visible limits booted with
/// `overrides` merged over the real constants.
///
/// Exists because `__zfb.limits` is frozen (epic #2012 review fix 5):
/// bundle code used to be able to raise a cap and wave an oversized
/// payload past the JS pre-check, and the same mutability was what let
/// these tests lower one from inside the probe script. The cap now
/// moves where a real deployment would move it — at host boot, from
/// Rust — so the assertions below are unchanged; only where the number
/// comes from is.
pub(crate) async fn probe_with_limits(
    script: &str,
    mode: DispatchMode,
    overrides: serde_json::Value,
) -> String {
    let bundle = format!(
        r#"
        export default {{
          async fetch(request) {{
            let out;
            try {{
              out = await (async () => {{ {script} }})();
            }} catch (e) {{
              out = "UNCAUGHT:" + (e && e.name) + ":" + (e && e.message);
            }}
            return new Response(String(out), {{ status: 200 }});
          }},
        }};
        "#
    );
    let mut host = EmbeddedV8RenderHost::with_limits_override(overrides).expect("host boot");
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute the probe bundle");
    let mut request = HttpRequestLike::get("http://zfb.local/");
    request.mode = mode;
    let response = tokio::time::timeout(BOUND, host.dispatch_fetch(request))
        .await
        .expect("the probe dispatch settles within 30s")
        .expect("dispatch");
    assert_eq!(response.status, 200, "the probe handler always answers 200");
    response
        .body_utf8()
        .expect("probe body is UTF-8")
        .to_string()
}

/// JS helper text, prepended to scripts that report an expected
/// rejection: `describe(e)` renders `<name>|<message>`, which is what
/// the assertions below compare against.
pub(crate) const DESCRIBE: &str = r#"
  const describe = (e) => String(e && e.name) + "|" + String(e && e.message);
  const expectReject = async (fn) => {
    try {
      const r = await fn();
      return "RESOLVED|" + String(r && r.status);
    } catch (e) {
      return describe(e);
    }
  };
"#;

/// Guardrail 4's own regression test: build-time render still denies
/// network access, with the byte-identical pre-epic message, and
/// **without touching a socket**.
///
/// The loopback server is live throughout precisely so the request
/// count can prove the denial happened in JS before any op call. This
/// is the test that fails if the mode reader is perturbed to report
/// request-time.
#[tokio::test]
async fn build_time_fetch_is_denied_with_the_byte_identical_ssg_message_and_no_socket() {
    let server = LoopbackServer::spawn_static(ok_response("must never be reached")).await;
    let url = server.url("/data");
    let script = format!(
        r#"{DESCRIBE}
           const url = {url:?};
           try {{
             await fetch(url);
             return "RESOLVED (the SSG denial is gone)";
           }} catch (e) {{
             return String(e.message);
           }}"#
    );

    let got = probe(&script, DispatchMode::BuildTime).await;
    assert_eq!(
        got,
        expected_ssg_denial(&url),
        "the build-time SSG denial must stay byte-identical (guardrail 4)"
    );
    assert_eq!(
        server.request_count(),
        0,
        "the build-time denial must happen in JS before any op call — no socket may be opened"
    );
}

/// The request-time branch performs a REAL request and adapts the
/// transport's outcome onto the `Response` shim.
#[tokio::test]
async fn request_time_fetch_reaches_the_server_and_adapts_the_response() {
    let server = LoopbackServer::spawn_static(ok_response("hello from loopback")).await;
    let url = server.url("/greeting");
    let script = format!(
        r#"const r = await fetch({url:?});
           const text = await r.text();
           return JSON.stringify({{
             status: r.status,
             statusText: r.statusText,
             ok: r.ok,
             type: r.type,
             url: r.url,
             redirected: r.redirected,
             bodyIsNull: r.body === null,
             contentType: r.headers.get("content-type"),
             text,
           }});"#
    );

    let got = probe(&script, DispatchMode::RequestTime).await;
    let parsed: serde_json::Value = serde_json::from_str(&got).unwrap_or_else(|e| {
        panic!("request-time fetch did not produce a JSON report ({e}); got: {got}")
    });
    assert_eq!(parsed["status"], 200);
    assert_eq!(parsed["statusText"], "OK");
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["type"], "default");
    assert_eq!(parsed["url"], url);
    assert_eq!(parsed["redirected"], false);
    assert_eq!(parsed["contentType"], "text/plain");
    assert_eq!(parsed["text"], "hello from loopback");
    assert_eq!(
        parsed["bodyIsNull"], true,
        "`response.body` is always null in this host — there is no ReadableStream (divergence D3)"
    );
    assert_eq!(server.request_count(), 1);
}

/// Method, header order, duplicate request headers and the body all
/// cross the boundary verbatim.
#[tokio::test]
async fn request_time_fetch_sends_the_method_headers_and_body_verbatim() {
    let server = LoopbackServer::spawn_static(ok_response("ok")).await;
    let url = server.url("/submit");
    let script = format!(
        r#"const r = await fetch({url:?}, {{
             method: "post",
             headers: [
               ["x-trace", "one"],
               ["x-trace", "two"],
               ["content-type", "application/json"],
             ],
             body: JSON.stringify({{ hello: "world" }}),
           }});
           return String(r.status);"#
    );

    let got = probe(&script, DispatchMode::RequestTime).await;
    assert_eq!(got, "200", "the probe reported: {got}");

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let seen = &requests[0];
    assert_eq!(
        seen.method, "POST",
        "the method is uppercased, not rewritten"
    );
    assert_eq!(seen.target, "/submit");
    assert_eq!(
        seen.headers
            .iter()
            .filter(|(k, _)| k == "x-trace")
            .map(|(_, v)| v.as_str())
            .collect::<Vec<_>>(),
        vec!["one", "two"],
        "duplicate REQUEST headers must survive the boundary too — the outbound side must not \
         comma-join them through the Headers sort-and-combine view"
    );
    assert_eq!(seen.header("content-type"), Some("application/json"));
    assert_eq!(seen.body, br#"{"hello":"world"}"#.to_vec());
}

/// `GET`/`HEAD` with a body is a `TypeError`, rejected before any
/// socket — and the message is the standard's, not an SSG denial.
#[tokio::test]
async fn get_and_head_with_a_body_are_rejected_before_any_socket() {
    let server = LoopbackServer::spawn_static(ok_response("must never be reached")).await;
    let url = server.url("/data");
    let script = format!(
        r#"{DESCRIBE}
           const url = {url:?};
           const get = await expectReject(() => fetch(url, {{ method: "GET", body: "x" }}));
           const head = await expectReject(() => fetch(url, {{ method: "HEAD", body: "x" }}));
           // A body-less GET on the same URL still works, so the
           // rejection above is about the BODY, not the fixture.
           const plain = await fetch(url);
           return JSON.stringify({{ get, head, plain: plain.status }});"#
    );

    let got = probe(&script, DispatchMode::RequestTime).await;
    let parsed: serde_json::Value =
        serde_json::from_str(&got).unwrap_or_else(|e| panic!("expected JSON ({e}); got: {got}"));
    assert_eq!(
        parsed["get"],
        "TypeError|Request with GET/HEAD method cannot have body."
    );
    assert_eq!(
        parsed["head"],
        "TypeError|Request with GET/HEAD method cannot have body."
    );
    assert_eq!(parsed["plain"], 200);
    assert_eq!(
        server.request_count(),
        1,
        "only the body-less GET may reach the server"
    );
}

/// A `ReadableStream` body is unsupported at request time, and says so
/// **in request-time terms** — never the SSG denial.
///
/// The stream is duck-typed because no stream type exists in this host
/// at all; that is exactly the condition being reported.
#[tokio::test]
async fn a_readable_stream_body_is_rejected_with_a_request_time_message() {
    let server = LoopbackServer::spawn_static(ok_response("must never be reached")).await;
    let url = server.url("/upload");
    let script = format!(
        r#"{DESCRIBE}
           const stream = {{ getReader() {{ throw new Error("never called"); }} }};
           return await expectReject(() =>
             fetch({url:?}, {{ method: "POST", body: stream }}),
           );"#
    );

    let got = probe(&script, DispatchMode::RequestTime).await;
    assert_eq!(got, format!("TypeError|fetch({url}{STREAM_BODY_TAIL}"));
    assert!(
        !got.contains("SSG runtime"),
        "an unsupported REQUEST-TIME capability must never report itself as the build-time SSG \
         denial — that misdiagnosis is the defect epic #2012 exists to fix; got: {got}"
    );
    assert_eq!(server.request_count(), 0);
}

/// An already-aborted signal rejects **without opening a socket**, both
/// via `AbortSignal.abort()` and via a controller aborted beforehand,
/// and `signal.reason` is used as the rejection value when set.
#[tokio::test]
async fn an_already_aborted_signal_rejects_without_opening_a_socket() {
    let server = LoopbackServer::spawn_static(ok_response("must never be reached")).await;
    let url = server.url("/data");
    let script = format!(
        r#"{DESCRIBE}
           const url = {url:?};

           const viaStatic = await expectReject(() =>
             fetch(url, {{ signal: AbortSignal.abort() }}),
           );

           const controller = new AbortController();
           controller.abort();
           const viaController = await expectReject(() => fetch(url, {{ signal: controller.signal }}));

           // A caller-supplied reason is what the promise rejects with.
           const custom = new Error("user cancelled the checkout poll");
           custom.name = "CancelledByUser";
           const viaReason = await expectReject(() =>
             fetch(url, {{ signal: AbortSignal.abort(custom) }}),
           );

           // `Request.signal`, not just `init.signal` (contract row
           // "Abort" names both).
           const carried = new Request(url, {{ signal: AbortSignal.abort() }});
           const viaRequest = await expectReject(() => fetch(carried));

           return JSON.stringify({{ viaStatic, viaController, viaReason, viaRequest }});"#
    );

    let got = probe(&script, DispatchMode::RequestTime).await;
    let parsed: serde_json::Value =
        serde_json::from_str(&got).unwrap_or_else(|e| panic!("expected JSON ({e}); got: {got}"));
    assert_eq!(parsed["viaStatic"], "AbortError|The operation was aborted.");
    assert_eq!(
        parsed["viaController"],
        "AbortError|The operation was aborted."
    );
    assert_eq!(
        parsed["viaReason"], "CancelledByUser|user cancelled the checkout poll",
        "`signal.reason` must be the rejection value when the caller set one"
    );
    assert_eq!(
        parsed["viaRequest"],
        "AbortError|The operation was aborted."
    );
    assert_eq!(
        server.request_count(),
        0,
        "an already-aborted signal must reject before a socket is opened"
    );
}

/// A mid-flight abort settles the caller's promise promptly with an
/// `AbortError`.
///
/// Deliberately NOT claiming more than that: the op's future belongs to
/// the Rust event loop and cannot be dropped from JS, so the socket
/// closes on the transport's own deadline rather than on the abort.
/// The server here never answers, so the ONLY way this test can settle
/// is the abort path — a race with a real response cannot mask a
/// regression.
#[tokio::test]
async fn an_abort_that_arrives_mid_flight_settles_the_caller() {
    let server = LoopbackServer::spawn(|_req, stream| async move {
        let _keep_the_socket_open = stream;
        std::future::pending::<()>().await;
    })
    .await;
    let url = server.url("/never");
    let script = format!(
        r#"{DESCRIBE}
           const controller = new AbortController();
           const inFlight = fetch({url:?}, {{ signal: controller.signal }});
           controller.abort();
           return await expectReject(() => inFlight);"#
    );

    let got = probe(&script, DispatchMode::RequestTime).await;
    assert_eq!(got, "AbortError|The operation was aborted.");
}

/// Repeated `set-cookie` survives the transport, the op boundary, and
/// the `Headers` shim — the one header shape a map-shaped bridge would
/// silently destroy.
#[tokio::test]
async fn duplicate_set_cookie_headers_survive_the_boundary() {
    let raw = concat!(
        "HTTP/1.1 200 OK\r\n",
        "content-type: text/plain\r\n",
        "set-cookie: a=1; Path=/; Expires=Wed, 21 Oct 2026 07:28:00 GMT\r\n",
        "set-cookie: b=2; Path=/\r\n",
        "content-length: 2\r\n",
        "connection: close\r\n",
        "\r\n",
        "ok",
    );
    let server = LoopbackServer::spawn_static(raw.as_bytes().to_vec()).await;
    let url = server.url("/login");
    let script = format!(
        r#"const r = await fetch({url:?});
           return JSON.stringify({{ cookies: r.headers.getSetCookie() }});"#
    );

    let got = probe(&script, DispatchMode::RequestTime).await;
    let parsed: serde_json::Value =
        serde_json::from_str(&got).unwrap_or_else(|e| panic!("expected JSON ({e}); got: {got}"));
    assert_eq!(
        parsed["cookies"],
        serde_json::json!([
            "a=1; Path=/; Expires=Wed, 21 Oct 2026 07:28:00 GMT",
            "b=2; Path=/"
        ]),
        "both set-cookie values must arrive uncombined and in wire order — an object-shaped \
         header bridge would have collapsed them to the last one"
    );
}

/// After a followed redirect, `response.url` is the FINAL url and
/// `redirected` is `true`.
#[tokio::test]
async fn response_url_and_redirected_reflect_the_followed_redirect() {
    let final_body = Arc::new(ok_response("arrived"));
    let server = LoopbackServer::spawn(move |req, mut stream| {
        let final_body = final_body.clone();
        async move {
            let bytes = if req.target == "/start" {
                redirect_response(302, "Found", "/final")
            } else {
                final_body.as_ref().clone()
            };
            let _ = stream.write_all(&bytes).await;
            let _ = stream.shutdown().await;
        }
    })
    .await;
    let start = server.url("/start");
    let script = format!(
        r#"const r = await fetch({start:?});
           return JSON.stringify({{
             url: r.url,
             redirected: r.redirected,
             text: await r.text(),
           }});"#
    );

    let got = probe(&script, DispatchMode::RequestTime).await;
    let parsed: serde_json::Value =
        serde_json::from_str(&got).unwrap_or_else(|e| panic!("expected JSON ({e}); got: {got}"));
    assert_eq!(parsed["url"], server.url("/final"));
    assert_eq!(parsed["redirected"], true);
    assert_eq!(parsed["text"], "arrived");
}

/// `blob()` and `formData()` remain unimplemented — but their message
/// names the embedded runtime, not the SSG policy, on BOTH halves of
/// the interface. A request-time caller told "SSG runtime" would go
/// hunting for a build-step fix that does not exist.
#[tokio::test]
async fn blob_and_form_data_report_the_embedded_runtime_not_the_ssg_policy() {
    let server = LoopbackServer::spawn_static(ok_response("payload")).await;
    let url = server.url("/data");
    let script = format!(
        r#"{DESCRIBE}
           const r = await fetch({url:?});
           const blob = await expectReject(() => r.blob());
           const formData = await expectReject(() => r.formData());
           const requestBlob = await expectReject(() =>
             new Request({url:?}, {{ method: "POST", body: "x" }}).blob(),
           );
           return JSON.stringify({{ blob, formData, requestBlob }});"#
    );

    let got = probe(&script, DispatchMode::RequestTime).await;
    let parsed: serde_json::Value =
        serde_json::from_str(&got).unwrap_or_else(|e| panic!("expected JSON ({e}); got: {got}"));
    assert_eq!(
        parsed["blob"],
        "Error|response.blob() is not implemented in the zfb embedded runtime"
    );
    assert_eq!(
        parsed["formData"],
        "Error|response.formData() is not implemented in the zfb embedded runtime"
    );
    assert_eq!(
        parsed["requestBlob"],
        "Error|request.blob() is not implemented in the zfb embedded runtime"
    );
    assert!(
        !got.contains("SSG runtime"),
        "these are unimplemented on BOTH paths, so neither message may name the build-time \
         policy; got: {got}"
    );
}

/// The JS-visible limits are the Rust constants — read out of a live
/// isolate and compared against `limits.rs` itself.
///
/// This is the mechanical enforcement of "one source of truth": a
/// second hardcoded copy in JS is a rejected design, and this test is
/// what would catch one being reintroduced, because a JS-side literal
/// would keep passing while `limits.rs` moved on.
#[tokio::test]
async fn js_visible_limits_are_the_rust_constants() {
    let got = probe(
        "return JSON.stringify(globalThis.__zfb.limits);",
        DispatchMode::RequestTime,
    )
    .await;
    let parsed: serde_json::Value =
        serde_json::from_str(&got).unwrap_or_else(|e| panic!("expected JSON ({e}); got: {got}"));

    assert_eq!(
        parsed["allowedFetchSchemes"],
        serde_json::json!(limits::ALLOWED_FETCH_SCHEMES)
    );
    assert_eq!(
        parsed["maxRedirects"],
        serde_json::json!(limits::MAX_REDIRECTS)
    );
    assert_eq!(
        parsed["defaultFetchTimeoutMs"],
        serde_json::json!(limits::DEFAULT_FETCH_TIMEOUT_MS)
    );
    assert_eq!(
        parsed["maxRequestBodyBytes"],
        serde_json::json!(limits::MAX_REQUEST_BODY_BYTES)
    );
    assert_eq!(
        parsed["maxResponseBodyBytes"],
        serde_json::json!(limits::MAX_RESPONSE_BODY_BYTES)
    );
    assert_eq!(
        parsed["maxSubrequestsPerDispatch"],
        serde_json::json!(limits::MAX_SUBREQUESTS_PER_DISPATCH)
    );
    assert_eq!(
        parsed["maxRandomBytesPerCall"],
        serde_json::json!(limits::MAX_RANDOM_BYTES_PER_CALL)
    );
    // The one non-constant in the object: the RESOLVED deadline, which
    // `ZFB_SSR_FETCH_TIMEOUT_MS` may have moved. The JS layer uses it
    // to tell a caller-fired deadline from a host-fired one.
    assert_eq!(
        parsed["fetchTimeoutMs"],
        serde_json::json!(limits::fetch_timeout_ms())
    );
}

/// The JS-side request-body cap is READ from the injected limits rather
/// than hardcoded, and its message quotes whatever number it read.
///
/// Proved by lowering `maxRequestBodyBytes` to 8 bytes for this
/// isolate: a hardcoded 100 MB literal in the polyfill would let the
/// 9-byte body through, so this fails if the JS copy of the constant
/// ever comes back. The genuine 100 MB cap is enforced in Rust (see
/// `fetch/tests.rs`) — allocating a real 100 MB buffer here would prove
/// nothing extra at considerable cost.
#[tokio::test]
async fn the_request_body_cap_is_read_from_the_injected_limits() {
    let server = LoopbackServer::spawn_static(ok_response("must never be reached")).await;
    let url = server.url("/upload");
    let script = format!(
        r#"{DESCRIBE}
           const injected = globalThis.__zfb.limits.maxRequestBodyBytes;
           return JSON.stringify({{
             injected,
             message: await expectReject(() =>
               fetch({url:?}, {{ method: "POST", body: "123456789" }}),
             ),
           }});"#
    );

    let got = probe_with_limits(
        &script,
        DispatchMode::RequestTime,
        serde_json::json!({ "maxRequestBodyBytes": 8 }),
    )
    .await;
    let parsed: serde_json::Value =
        serde_json::from_str(&got).unwrap_or_else(|e| panic!("expected JSON ({e}); got: {got}"));
    assert_eq!(
        parsed["injected"],
        serde_json::json!(8),
        "the polyfill must read the cap the host injected, not a literal of its own"
    );
    assert_eq!(
        parsed["message"],
        format!("TypeError|fetch({url}): request body exceeds the 8-byte limit"),
        "the cap message must quote the limit it actually read"
    );
    assert_eq!(
        server.request_count(),
        0,
        "an oversized body must not cross the op boundary at all"
    );
}

/// The abort primitives exist as globals and carry the static
/// constructors the Fetch standard defines. Absent, feature detection
/// (`typeof AbortController === "function"`) takes a fallback branch
/// locally that production would never take.
#[tokio::test]
async fn the_abort_primitives_are_installed_as_globals() {
    let script = r#"
      return JSON.stringify({
        controller: typeof globalThis.AbortController,
        signal: typeof globalThis.AbortSignal,
        abort: typeof AbortSignal.abort,
        timeout: typeof AbortSignal.timeout,
        // A pending controller has not aborted yet.
        pending: new AbortController().signal.aborted,
        // A zero-millisecond deadline has already elapsed, so it CAN be
        // honoured synchronously.
        zeroTimeout: (() => {
          const s = AbortSignal.timeout(0);
          return s.aborted + ":" + s.reason.name;
        })(),
        // A positive one is carried to the Rust transport instead (this
        // host has no timers), so the signal itself stays unaborted.
        positiveTimeout: (() => {
          const s = AbortSignal.timeout(50);
          return s.aborted + ":" + s._timeoutMs;
        })(),
      });
    "#;

    let got = probe(script, DispatchMode::RequestTime).await;
    let parsed: serde_json::Value =
        serde_json::from_str(&got).unwrap_or_else(|e| panic!("expected JSON ({e}); got: {got}"));
    assert_eq!(parsed["controller"], "function");
    assert_eq!(parsed["signal"], "function");
    assert_eq!(parsed["abort"], "function");
    assert_eq!(parsed["timeout"], "function");
    assert_eq!(parsed["pending"], false);
    assert_eq!(parsed["zeroTimeout"], "true:TimeoutError");
    assert_eq!(parsed["positiveTimeout"], "false:50");
}

/// `AbortSignal.timeout(ms)` genuinely bounds a request, through the
/// Rust deadline (this host has no timers), and reports itself the way
/// the standard says rather than quoting the host's own limit.
#[tokio::test]
async fn a_signal_timeout_bounds_the_request_and_reports_as_a_timeout_error() {
    let server = LoopbackServer::spawn(|_req, stream| async move {
        let _keep_the_socket_open = stream;
        std::future::pending::<()>().await;
    })
    .await;
    let url = server.url("/never");
    let script = format!(
        r#"{DESCRIBE}
           return await expectReject(() =>
             fetch({url:?}, {{ signal: AbortSignal.timeout(250) }}),
           );"#
    );

    let got = probe(&script, DispatchMode::RequestTime).await;
    assert_eq!(
        got,
        "TimeoutError|The operation was aborted due to timeout."
    );
    assert_eq!(
        server.request_count(),
        1,
        "the deadline is enforced by the transport, so the request did reach the server"
    );
}

/// A stream body is refused even when it arrives INSIDE an
/// already-constructed `Request`.
///
/// The dangerous shape, and the reason the check reads a recorded flag
/// rather than `init.body`: by the time such a `Request` reaches
/// `fetch`, the stream has been coerced to the string
/// `"[object Object]"`, so a check that looked only at `init.body`
/// would send that to the server as a real payload — silently wrong
/// data instead of the documented `TypeError`.
#[tokio::test]
async fn a_stream_body_carried_inside_a_request_is_refused_too() {
    let server = LoopbackServer::spawn_static(ok_response("must never be reached")).await;
    let url = server.url("/upload");
    let script = format!(
        r#"{DESCRIBE}
           const stream = {{ getReader() {{ throw new Error("never called"); }} }};
           const carried = new Request({url:?}, {{ method: "POST", body: stream }});
           return await expectReject(() => fetch(carried));"#
    );

    let got = probe(&script, DispatchMode::RequestTime).await;
    assert_eq!(got, format!("TypeError|fetch({url}{STREAM_BODY_TAIL}"));
    assert_eq!(
        server.request_count(),
        0,
        "a coerced stream must never reach the wire as a payload"
    );
}

/// The BodyInit default Content-Type survives a `Request` copy.
///
/// `fetch(new Request(url, { body: "x" }))` and the equivalent
/// url-plus-init call must put the same request on the wire. They stop
/// agreeing the moment the default type is recomputed at send time,
/// because the copy's body is bytes by then and bytes carry no default.
#[tokio::test]
async fn the_body_init_default_content_type_survives_a_request_copy() {
    let server = LoopbackServer::spawn_static(ok_response("ok")).await;
    let via_init = server.url("/via-init");
    let via_request = server.url("/via-request");
    let script = format!(
        r#"await fetch({via_init:?}, {{ method: "POST", body: "x" }});
           await fetch(new Request({via_request:?}, {{ method: "POST", body: "y" }}));
           return "sent";"#
    );

    let got = probe(&script, DispatchMode::RequestTime).await;
    assert_eq!(got, "sent");

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    let types: Vec<Option<&str>> = requests.iter().map(|r| r.header("content-type")).collect();
    assert_eq!(
        types,
        vec![
            Some("text/plain;charset=UTF-8"),
            Some("text/plain;charset=UTF-8")
        ],
        "both spellings of the same request must put the same content-type on the wire"
    );
}

/// Dispatching disturbs the request's body, exactly as the standard
/// says: a body-bearing `Request` may be fetched once, and one whose
/// body was already read is refused rather than silently resent.
///
/// Without this, a `Request` could be fetched in a loop with each pass
/// a real network side effect — the request count below is what proves
/// the second attempt never reached the wire.
#[tokio::test]
async fn dispatching_disturbs_the_request_body() {
    let server = LoopbackServer::spawn_static(ok_response("ok")).await;
    let url = server.url("/submit");
    let script = format!(
        r#"{DESCRIBE}
           const once = new Request({url:?}, {{ method: "POST", body: "payload" }});
           const first = await fetch(once);
           const second = await expectReject(() => fetch(once));

           // A body already read by hand cannot then be sent.
           const read = new Request({url:?}, {{ method: "POST", body: "payload" }});
           await read.text();
           const afterRead = await expectReject(() => fetch(read));

           // A body-LESS request is not disturbed, so it may be
           // re-fetched — the rule is about bodies, not about requests.
           const bodyless = new Request({url:?});
           await fetch(bodyless);
           const again = await fetch(bodyless);

           return JSON.stringify({{
             first: first.status,
             second,
             afterRead,
             again: again.status,
             bodyUsed: once.bodyUsed,
           }});"#
    );

    let got = probe(&script, DispatchMode::RequestTime).await;
    let parsed: serde_json::Value =
        serde_json::from_str(&got).unwrap_or_else(|e| panic!("expected JSON ({e}); got: {got}"));
    assert_eq!(parsed["first"], 200);
    assert_eq!(parsed["second"], "TypeError|Body already consumed");
    assert_eq!(parsed["afterRead"], "TypeError|Body already consumed");
    assert_eq!(parsed["again"], 200);
    assert_eq!(
        parsed["bodyUsed"], true,
        "dispatching must mark the SOURCE request's body used, not just the internal copy"
    );
    assert_eq!(
        server.request_count(),
        3,
        "one body-bearing send plus the two body-less ones — the refused attempts must not \
         have reached the wire"
    );
}

/// A signal timeout LONGER than the host ceiling means the host's
/// deadline is the one that fires, and it must keep its own diagnostic.
///
/// Relabelling it would tell the developer their 60-second timeout
/// fired at 30 seconds, and would throw away the URL, the effective
/// deadline, and the note that production Workers has no such limit.
#[tokio::test]
async fn a_host_deadline_is_not_relabelled_as_the_callers() {
    let server = LoopbackServer::spawn(|_req, stream| async move {
        let _keep_the_socket_open = stream;
        std::future::pending::<()>().await;
    })
    .await;
    let url = server.url("/never");
    // The JS-visible host ceiling is lowered to 100ms for this isolate
    // so the caller's 250ms deadline EXCEEDS it — the exact condition
    // the branch turns on — while the transport still ends the call in
    // 250ms rather than the real 30s. (Lowering the genuine Rust
    // ceiling would mean setting `ZFB_SSR_FETCH_TIMEOUT_MS`, which is
    // process-wide and memoised at first use, so it would race every
    // other test in this binary.) What is under test is which side's
    // DIAGNOSTIC survives, which is decided here from this value.
    let script = format!(
        r#"{DESCRIBE}
           return await expectReject(() =>
             fetch({url:?}, {{ signal: AbortSignal.timeout(250) }}),
           );"#
    );

    let got = probe_with_limits(
        &script,
        DispatchMode::RequestTime,
        serde_json::json!({ "fetchTimeoutMs": 100 }),
    )
    .await;
    assert!(
        got.starts_with("TimeoutError|fetch(") && got.contains("timed out after"),
        "the host's own deadline diagnostic must survive intact when it is the one that \
         fired; got: {got}"
    );
    assert!(
        !got.contains("aborted due to timeout"),
        "the caller's deadline did not fire — 9000ms is longer than the host ceiling; got: {got}"
    );
}

/// The misleading wording lives on exactly ONE code path.
///
/// A source-level check rather than a behavioural one on purpose: it is
/// the only way to prove a *negative* across every branch, including
/// ones no test drives yet, and it is what stops a future wave from
/// reaching for the familiar string when it adds a new rejection.
#[test]
fn the_ssg_denial_message_appears_on_exactly_one_code_path() {
    let occurrences = extensions::WEB_POLYFILLS_SRC
        .matches("fetch() called from SSG runtime")
        .count();
    assert_eq!(
        occurrences, 1,
        "the SSG denial must be reachable from exactly one place — a genuine build-time render"
    );
    assert!(
        !extensions::WEB_POLYFILLS_SRC.contains("is not implemented in the SSG runtime"),
        "a capability that is unavailable at request time as well must not blame the SSG policy \
         for it; say `the zfb embedded runtime` instead"
    );
}

/// The transport's own wall-clock deadline reaches JS as a
/// `TimeoutError` carrying the transport's message.
///
/// This is the boundary `registerHostErrorClasses` exists for.
/// deno_core rebuilds an op error from its class NAME, and its map only
/// knows the six ECMAScript builtins — before the registration, this
/// exact case surfaced as `TypeError: invalid_argument`, with the
/// deadline, the URL and the production-divergence note all gone. A
/// regression here does not merely reword a message; it destroys the
/// whole diagnostic.
#[tokio::test]
async fn the_transport_deadline_reaches_js_as_a_timeout_error_with_its_message() {
    let server = LoopbackServer::spawn(|_req, stream| async move {
        let _keep_the_socket_open = stream;
        std::future::pending::<()>().await;
    })
    .await;
    let url = server.url("/never");
    // Driven through the op directly rather than through `fetch`: this
    // is about the RUST error crossing the boundary intact, and
    // `fetch`'s own signal-timeout path deliberately rewrites the
    // message (see the test above), which would mask the regression.
    let script = format!(
        r#"{DESCRIBE}
           return await expectReject(() =>
             Deno.core.ops.op_zfb_fetch(
               {{
                 url: {url:?},
                 method: "GET",
                 headers: [],
                 redirect: "follow",
                 hasBody: false,
                 timeoutMs: 200,
               }},
               new Uint8Array(0),
             ),
           );"#
    );

    let got = probe(&script, DispatchMode::RequestTime).await;
    assert_eq!(
        got,
        format!("TimeoutError|fetch({url}){TRANSPORT_TIMEOUT_TAIL}"),
        "the deadline's class AND message must both survive the op boundary"
    );
}

// ---------------------------------------------------------------------
// Epic-review fixes (epic #2012 review pass). Each test below was
// written RED against the pre-fix code and its observed failure is
// recorded in the PR description.
// ---------------------------------------------------------------------

/// Guardrail 4 is enforced **in Rust**, not only in the JS polyfill.
///
/// `Deno.core.ops.op_zfb_fetch` is reachable from bundle code — the
/// polyfill is one caller of it, not a gate in front of it. Before this
/// fix the op consulted no [`DispatchMode`] at all, so a build-time
/// render that skipped `fetch()` and called the op directly opened a
/// real socket. A policy enforced only in the layer the bundle controls
/// is not enforced.
#[tokio::test]
async fn a_build_time_dispatch_cannot_reach_the_network_through_the_raw_op() {
    let server = LoopbackServer::spawn_static(ok_response("must never be reached")).await;
    let url = server.url("/raw-op");
    let script = format!(
        r#"{DESCRIBE}
           return await expectReject(() =>
             Deno.core.ops.op_zfb_fetch(
               {{
                 url: {url:?},
                 method: "GET",
                 headers: [],
                 redirect: "follow",
                 hasBody: false,
               }},
               new Uint8Array(0),
             ),
           );"#
    );

    let got = probe(&script, DispatchMode::BuildTime).await;
    assert_eq!(
        got,
        format!("TypeError|{}", expected_ssg_denial(&url)),
        "the op itself must refuse a build-time caller, with the same policy wording the \
         polyfill uses"
    );
    assert_eq!(
        server.request_count(),
        0,
        "the build-time denial must happen before any socket is opened, however the op was \
         reached"
    );
}

/// The polyfill's view of the host bridge cannot be swapped out.
///
/// `globalThis.__zfb` is a writable data property and the polyfill used
/// to re-read it on every `fetch`, so replacing the object with
/// `{ mode: "request-time" }` made a build-time render take the
/// request-time branch. The polyfill now captures the host's own mode
/// reader once, through a single-use channel the shim consumes at boot.
#[tokio::test]
async fn replacing_the_zfb_bridge_cannot_select_the_request_time_branch() {
    let server = LoopbackServer::spawn_static(ok_response("must never be reached")).await;
    let url = server.url("/swapped-bridge");
    let script = format!(
        r#"{DESCRIBE}
           const realLimits = globalThis.__zfb.limits;
           globalThis.__zfb = {{ mode: "request-time", limits: realLimits }};
           return await expectReject(() => fetch({url:?}));"#
    );

    let got = probe(&script, DispatchMode::BuildTime).await;
    // `Error`, not `TypeError`: this is the polyfill's own build-time
    // rejection, whose class and wording predate the epic and are
    // deliberately untouched. (The op's identically-worded refusal —
    // the enforcement that does not depend on JS — arrives as a
    // `TypeError`; see the raw-op test above.)
    assert_eq!(
        got,
        format!("Error|{}", expected_ssg_denial(&url)),
        "a forged bridge must not move the polyfill off the build-time branch"
    );
    assert_eq!(
        server.request_count(),
        0,
        "and no socket may be opened along the way"
    );
}

/// Bundle code cannot move the JS-side caps.
///
/// `__zfb.limits` used to be an ordinary mutable object, so a bundle
/// could **raise** `maxRequestBodyBytes` and wave an oversized payload
/// past the JS pre-check into the op. The object (and the array inside
/// it) is now frozen, and — because the polyfill reads its captured
/// copy rather than the global — even replacing the whole bridge
/// changes nothing.
///
/// Proved by attempting to LOWER the cap, which is the cheap direction:
/// a 9-byte body against a cap the bundle set to 8 must still be sent,
/// because the mutation did not take.
#[tokio::test]
async fn bundle_code_cannot_move_the_js_side_request_body_cap() {
    let server = LoopbackServer::spawn_static(ok_response("delivered")).await;
    let url = server.url("/upload");
    let script = format!(
        r#"{DESCRIBE}
           let threw = "none";
           try {{
             globalThis.__zfb.limits.maxRequestBodyBytes = 8;
           }} catch (e) {{
             threw = String(e && e.name);
           }}
           return JSON.stringify({{
             threw,
             after: globalThis.__zfb.limits.maxRequestBodyBytes,
             result: await expectReject(() =>
               fetch({url:?}, {{ method: "POST", body: "123456789" }}),
             ),
           }});"#
    );

    let got = probe(&script, DispatchMode::RequestTime).await;
    let parsed: serde_json::Value =
        serde_json::from_str(&got).unwrap_or_else(|e| panic!("expected JSON ({e}); got: {got}"));
    assert_eq!(
        parsed["threw"], "TypeError",
        "writing to a frozen limits object must throw in the bundle's strict-mode module scope"
    );
    assert_eq!(
        parsed["after"],
        serde_json::json!(limits::MAX_REQUEST_BODY_BYTES),
        "the cap must still read as the Rust constant after the attempted mutation"
    );
    assert_eq!(
        parsed["result"], "RESOLVED|200",
        "the 9-byte body must still be sent — the bundle's lowered cap must have no effect"
    );
    assert_eq!(
        server.request_count(),
        1,
        "and it must have reached the wire exactly once"
    );
}

/// An abort arriving MID-BODY genuinely cancels the transport.
///
/// The #2013 contract's "Abort" row says the Rust future is dropped and
/// the socket closed. Before this fix `raceWithAbort` settled the
/// caller's promise while the transport ran on to the 30-second
/// deadline — the subrequest slot stayed spent and up to 100 MB kept
/// buffering — so an abort freed nothing. This is also the
/// `cancellation mid-body` case wave 3's own test matrix required and
/// never landed.
///
/// Deliberately NOT driven through [`probe`]: that helper drops the
/// host when it returns, and dropping the `JsRuntime` tears down every
/// pending op, which would close the socket with or without the fix.
/// The host is held alive here for exactly that reason.
#[tokio::test]
async fn an_abort_mid_body_cancels_the_transport_and_closes_the_socket() {
    use tokio::io::AsyncReadExt;
    use tokio::sync::Semaphore;

    // `/slow` promises 64 bytes, writes 8, signals `body_started`, then
    // parks holding the socket — the transport is now mid-body. `/gate`
    // answers only after that signal, which is how the bundle learns the
    // transport reached that point without a sleep.
    //
    // The handler aborts and returns its `Response` IMMEDIATELY, with
    // nothing afterwards to keep the event loop turning. That is
    // load-bearing: `CancelHandle::cancel()` only marks and wakes the
    // handle, so without `drain_cancelled_fetches` the dispatch promise
    // would settle first and the host would go idle with the socket
    // still open. Do not add a trailing fetch here to "help it along" —
    // that is exactly the crutch this case exists to remove.
    let body_started = Arc::new(Semaphore::new(0));
    let socket_closed = Arc::new(Semaphore::new(0));
    let started = body_started.clone();
    let closed = socket_closed.clone();
    let server = LoopbackServer::spawn(move |req, mut stream| {
        let started = started.clone();
        let closed = closed.clone();
        async move {
            match req.target.as_str() {
                "/slow" => {
                    let head = "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\n\
                                content-length: 64\r\n\r\n";
                    if stream.write_all(head.as_bytes()).await.is_err() {
                        return;
                    }
                    if stream.write_all(b"01234567").await.is_err() {
                        return;
                    }
                    let _ = stream.flush().await;
                    started.add_permits(1);
                    // The remaining 56 bytes are never written. A genuine
                    // cancellation closes the socket, and this read then
                    // reports EOF.
                    let mut buf = [0u8; 64];
                    loop {
                        match stream.read(&mut buf).await {
                            Ok(0) | Err(_) => {
                                closed.add_permits(1);
                                return;
                            }
                            Ok(_) => {}
                        }
                    }
                }
                "/gate" => {
                    if let Ok(permit) = started.acquire().await {
                        permit.forget();
                    }
                    let _ = stream.write_all(&ok_response("go")).await;
                    let _ = stream.shutdown().await;
                }
                other => panic!("unexpected request target {other:?}"),
            }
        }
    })
    .await;

    let slow = server.url("/slow");
    let gate = server.url("/gate");
    let bundle = format!(
        r#"
        export default {{
          async fetch(request) {{
            const controller = new AbortController();
            const pending = fetch({slow:?}, {{ signal: controller.signal }});
            pending.catch(() => {{}});
            await fetch({gate:?});
            controller.abort();
            let out;
            try {{
              await pending;
              out = "RESOLVED";
            }} catch (e) {{
              out = String(e && e.name);
            }}
            return new Response(out, {{ status: 200 }});
          }},
        }};
        "#
    );

    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute the abort bundle");
    let mut request = HttpRequestLike::get("http://zfb.local/");
    request.mode = DispatchMode::RequestTime;
    let response = tokio::time::timeout(BOUND, host.dispatch_fetch(request))
        .await
        .expect("the abort dispatch settles within 30s")
        .expect("dispatch");
    assert_eq!(
        response.body_utf8(),
        Some("AbortError"),
        "the caller's promise must reject with an AbortError"
    );

    // The host is still alive here, so nothing but a genuine
    // cancellation can have closed the connection.
    tokio::time::timeout(BOUND, socket_closed.acquire())
        .await
        .unwrap_or_else(|_| {
            panic!(
                "the mid-body connection was never closed: the abort settled the caller's \
                 promise but left the transport running, so the subrequest slot stays spent \
                 and the response keeps buffering"
            )
        })
        .expect("socket-closed semaphore")
        .forget();

    drop(host);
}

/// Evaluate `script` (which must produce a Promise) with the event loop
/// running, and return the settled value stringified.
///
/// A private twin of `fetch_boundary_tests::eval_await_string`: the mode
/// tests below need to observe host state BETWEEN dispatches, which
/// [`probe`] cannot show because it drops the host as it returns.
async fn eval_await_string(host: &mut EmbeddedV8RenderHost, script: String) -> String {
    let promise = host
        .runtime
        .execute_script("zfb:mode_tracking_test", script)
        .expect("test script evaluates");
    let resolve = host.runtime.resolve(promise);
    let resolved = host
        .runtime
        .with_event_loop_promise(Box::pin(resolve), PollEventLoopOptions::default())
        .await
        .expect("the test promise settles");
    deno_core::scope!(scope, &mut host.runtime);
    let local = v8::Local::new(scope, resolved);
    local.to_rust_string_lossy(scope)
}

fn read_js_mode(host: &mut EmbeddedV8RenderHost) -> String {
    let value = host
        .runtime
        .execute_script("zfb:read_mode", "String(globalThis.__zfb.mode)")
        .expect("the mode read evaluates");
    deno_core::scope!(scope, &mut host.runtime);
    let local = v8::Local::new(scope, value);
    local.to_rust_string_lossy(scope)
}

/// The mode belongs to the dispatch that set it, not to the last
/// `finally` that happened to run.
///
/// `dispatch`'s `finally` used to restore `undefined` the instant the
/// dispatch settled, so anything a request-time handler started without
/// awaiting — which returns a `Response` and leaves the call in flight —
/// reported itself as an SSG policy denial **at request time**: exactly
/// the misdiagnosis this epic exists to remove. The settled dispatch's
/// mode now stands until the next dispatch replaces it or the host
/// resets it for a module evaluation.
#[tokio::test]
async fn the_mode_outlives_its_dispatch_for_the_calls_that_dispatch_orphaned() {
    let bundle = r#"
        export default {
          async fetch(request) {
            return new Response("ok", { status: 200 });
          },
        };
    "#;
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    host.execute_module("bundle.mjs", bundle)
        .await
        .expect("execute the bundle");
    assert_eq!(
        read_js_mode(&mut host),
        "undefined",
        "module evaluation is not a dispatch, so it runs at the denying default"
    );

    let mut request = HttpRequestLike::get("http://zfb.local/");
    request.mode = DispatchMode::RequestTime;
    host.dispatch_fetch(request).await.expect("dispatch");
    assert_eq!(
        read_js_mode(&mut host),
        "request-time",
        "a continuation the request-time dispatch orphaned must still read as request-time"
    );

    let mut request = HttpRequestLike::get("http://zfb.local/");
    request.mode = DispatchMode::BuildTime;
    host.dispatch_fetch(request).await.expect("dispatch");
    assert_eq!(
        read_js_mode(&mut host),
        "build-time",
        "and the next dispatch replaces it unconditionally — nothing is inherited"
    );

    host.reset_dispatch_mode_for_evaluation();
    assert_eq!(
        read_js_mode(&mut host),
        "undefined",
        "the host drops back to the denying default before evaluating a module"
    );
}

/// A forged nested `dispatch` cannot republish a stale mode after the
/// host has moved on.
///
/// Bundle code can reach `globalThis.__zfb.dispatch` but not the nonce,
/// so it cannot SELECT a mode — but the old code still captured and
/// restored the ambient mode in the forged call's `finally`. For a
/// floating call that `finally` runs long after the enclosing dispatch
/// ended, republishing whatever was current when it started. An
/// unauthorised call now touches the mode neither on entry nor on exit.
#[tokio::test]
async fn a_floating_unauthorised_dispatch_cannot_republish_a_stale_mode() {
    let bundle = r#"
        let release;
        globalThis.__release = () => release && release();
        export default {
          async fetch(request) {
            const url = new URL(request.url);
            if (url.pathname === "/nested") {
              // Parks until the test releases it, so it is guaranteed
              // to settle AFTER the outer dispatch and after the host
              // reset below.
              await new Promise((r) => { release = r; });
              return new Response("nested", { status: 200 });
            }
            // A forged re-entry: the right shape, the wrong nonce.
            globalThis.__nested = globalThis.__zfb.dispatch(
              "http://zfb.local/nested", "GET", null, undefined, "request-time", "not-the-nonce",
            );
            globalThis.__nested.catch(() => {});
            return new Response("outer", { status: 200 });
          },
        };
    "#;
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    host.execute_module("bundle.mjs", bundle)
        .await
        .expect("execute the bundle");

    let mut request = HttpRequestLike::get("http://zfb.local/outer");
    request.mode = DispatchMode::RequestTime;
    let response = host.dispatch_fetch(request).await.expect("dispatch");
    assert_eq!(
        response.body_utf8(),
        Some("outer"),
        "the outer dispatch must settle while the forged nested one is still parked"
    );

    host.reset_dispatch_mode_for_evaluation();
    assert_eq!(
        read_js_mode(&mut host),
        "undefined",
        "the reset took effect"
    );

    let after = eval_await_string(
        &mut host,
        "(async () => { globalThis.__release(); await globalThis.__nested; \
         return String(globalThis.__zfb.mode); })()"
            .to_string(),
    )
    .await;
    assert_eq!(
        after, "undefined",
        "the forged nested dispatch settling must not put the mode back to what it was when \
         that call started"
    );
}

/// The mode nonce is CSPRNG-derived and per host.
///
/// It used to be `pid | wall-clock nanos | counter` — a value bundle
/// code could RECONSTRUCT rather than having to guess — justified by a
/// comment claiming no `getrandom` edge existed in this crate. Wave 5
/// wired `getrandom::fill` in `crypto.rs`, so that rationale was stale
/// as well as wrong.
#[test]
fn the_mode_nonce_is_256_csprng_bits_and_differs_per_host() {
    let a = EmbeddedV8RenderHost::new().expect("host boot");
    let b = EmbeddedV8RenderHost::new().expect("host boot");
    assert_eq!(
        a.mode_nonce.len(),
        "zfb-mode-".len() + 64,
        "32 bytes, hex-encoded, behind the fixed prefix"
    );
    assert!(
        a.mode_nonce
            .strip_prefix("zfb-mode-")
            .expect("the prefix is part of the format")
            .chars()
            .all(|c| c.is_ascii_hexdigit()),
        "the body must be hex, so it carries no structure to reconstruct: {}",
        a.mode_nonce
    );
    assert_ne!(
        a.mode_nonce, b.mode_nonce,
        "two hosts must not share a nonce"
    );
    assert!(
        !a.mode_nonce.contains(&format!("{:x}", std::process::id())),
        "the pid must not be recoverable from the nonce: {}",
        a.mode_nonce
    );
}

/// The nonce comparison is constant-time, and nothing reintroduces the
/// short-circuiting one.
///
/// A source check because it is the only way to prove the absence of a
/// `===` across every branch, including ones no test drives.
#[test]
fn the_nonce_is_compared_in_constant_time() {
    let src = extensions::HOST_GLOBALS_SHIM_SRC;
    assert!(
        src.contains("__zfb_constantTimeEquals"),
        "the shim must carry a constant-time comparison for the mode nonce"
    );
    assert!(
        !src.contains("nonce === __ZFB_MODE_NONCE") && !src.contains("__ZFB_MODE_NONCE === nonce"),
        "a short-circuiting `===` on the nonce leaks a prefix oracle — compare in constant time"
    );
}

/// The op must not COPY the request body before it has checked the cap.
///
/// `#[buffer(copy)]` performs the allocation during argument decoding,
/// i.e. before a single line of the op body runs — so the ceiling would
/// be enforced only after the very allocation it exists to prevent. A
/// source check, because the ordering is a property of the attribute
/// rather than of any value a test could read back.
#[test]
fn the_request_body_cap_is_checked_before_the_buffer_is_copied() {
    // Comments stripped first, so the doc-comment that EXPLAINS why
    // `#[buffer(copy)]` is wrong cannot itself trip the check.
    let src: String = include_str!("fetch.rs")
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !src.contains("#[buffer(copy)]"),
        "op_zfb_fetch must take a borrowed `#[buffer]` and copy only after the cap check"
    );
    let op_body = src
        .split_once("pub async fn op_zfb_fetch(")
        .expect("the op is declared in this file")
        .1;
    let check_at = op_body
        .find("FetchError::RequestBodyTooLarge")
        .expect("the op checks the request-body cap itself");
    let copy_at = op_body
        .find("body.to_vec()")
        .expect("the op copies the buffer");
    assert!(
        check_at < copy_at,
        "the cap check must precede the copy in op_zfb_fetch"
    );
}

/// A completed fetch leaves no cancellation handle behind.
///
/// The registry is keyed by an id the JS side mints per call, so an
/// entry that is never removed is a per-fetch leak for the life of the
/// host — and the ids would eventually collide with a live one.
#[tokio::test]
async fn a_settled_fetch_unregisters_its_cancellation_handle() {
    let server = LoopbackServer::spawn_static(ok_response("ok")).await;
    let url = server.url("/ok");
    let bundle = format!(
        r#"
        export default {{
          async fetch(request) {{
            const r = await fetch({url:?}, {{ signal: AbortSignal.timeout(10000) }});
            return new Response(String(r.status), {{ status: 200 }});
          }},
        }};
        "#
    );
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute the bundle");
    let mut request = HttpRequestLike::get("http://zfb.local/");
    request.mode = DispatchMode::RequestTime;
    let response = tokio::time::timeout(BOUND, host.dispatch_fetch(request))
        .await
        .expect("the dispatch settles within 30s")
        .expect("dispatch");
    assert_eq!(response.body_utf8(), Some("200"));

    let op_state = host.runtime.op_state();
    let op_state = op_state.borrow();
    let cancels = op_state
        .try_borrow::<Rc<fetch::CancelRegistry>>()
        .expect("the registry is installed");
    assert!(
        cancels.is_empty(),
        "a settled fetch must remove its own handle; {} left behind",
        cancels.len()
    );
}
