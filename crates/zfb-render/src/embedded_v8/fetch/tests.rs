//! The #2015 acceptance matrix for the request-time fetch transport.
//!
//! **Every case drives a deterministic loopback server** — guardrail 3
//! of epic #2012 forbids reaching the public internet, which would make
//! these non-deterministic and flaky in CI. Ports come from the OS
//! (`127.0.0.1:0`), never hard-coded.
//!
//! Waits are event- or condition-keyed, never fixed sleeps gating an
//! assertion: the timeout and cancellation cases drive servers that
//! block deterministically and signal over a `oneshot`, so a slow CI
//! machine changes how long a test takes but never whether it passes.
//!
//! Level 1/3 on the zfb ladder — pure Rust logic plus a real socket. The
//! V8-boundary half (the op is genuinely async and its rejections reach
//! JS as rejected promises) lives in `embedded_v8/mod.rs`'s own tests,
//! which have a real isolate to drive.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;

use super::*;
use crate::embedded_v8::loopback_test_server::{
    closed_port_url, ok_response, redirect_response, LoopbackServer,
};

// ---------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------

/// A plain `GET` spec with no headers and no body.
fn get(url: &str) -> FetchRequestSpec {
    FetchRequestSpec {
        url: url.to_string(),
        method: "GET".to_string(),
        headers: Vec::new(),
        redirect: RedirectMode::Follow,
        has_body: false,
        timeout_ms: None,
    }
}

/// A generous config. Cases that exercise a limit lower the one limit
/// they are about, so nothing else can be the reason a test fails.
fn config() -> FetchConfig {
    FetchConfig {
        timeout_ms: 30_000,
        max_redirects: 20,
        max_request_body_bytes: 1024 * 1024,
        max_response_body_bytes: 1024 * 1024,
        max_subrequests: 100,
    }
}

fn client() -> reqwest::Client {
    build_fetch_client().expect("build the shared reqwest client")
}

async fn run(
    config: &FetchConfig,
    spec: &FetchRequestSpec,
    body: Vec<u8>,
) -> Result<FetchOutcome, FetchError> {
    let counter = SubrequestCounter::new();
    perform_fetch(&client(), &counter, config, spec, body).await
}

/// A `oneshot::Sender` usable from the `Fn` (not `FnOnce`) handler
/// closure the loopback server takes.
type Signal = Arc<Mutex<Option<oneshot::Sender<()>>>>;

fn signal() -> (Signal, oneshot::Receiver<()>) {
    let (tx, rx) = oneshot::channel();
    (Arc::new(Mutex::new(Some(tx))), rx)
}

fn fire(signal: &Signal) {
    let taken = signal.lock().expect("signal mutex").take();
    if let Some(tx) = taken {
        let _ = tx.send(());
    }
}

// ---------------------------------------------------------------------
// 1. success
// ---------------------------------------------------------------------

#[tokio::test]
async fn success_surfaces_status_status_text_headers_body_and_final_url() {
    let server = LoopbackServer::spawn_static(ok_response("hello loopback")).await;
    let url = server.url("/greeting");

    let outcome = run(&config(), &get(&url), Vec::new())
        .await
        .expect("a 200 from the loopback server");

    assert_eq!(outcome.status, 200);
    assert_eq!(outcome.status_text, "OK");
    assert_eq!(outcome.body, b"hello loopback");
    assert_eq!(outcome.header("content-type"), Some("text/plain"));
    assert_eq!(outcome.url, url, "response.url is the final URL");
    assert!(!outcome.redirected, "no hop was followed");
    assert_eq!(server.requests()[0].target, "/greeting");
}

// ---------------------------------------------------------------------
// 2. timeout
// ---------------------------------------------------------------------

#[tokio::test]
async fn timeout_rejects_with_a_timeout_error_naming_the_deadline() {
    // The server reads the request and then never answers — the block
    // is deterministic, not a race against a real clock.
    let server = LoopbackServer::spawn(|_req, stream| async move {
        let _keep_the_socket_open = stream;
        std::future::pending::<()>().await;
    })
    .await;
    let url = server.url("/never");

    let config = FetchConfig {
        timeout_ms: 250,
        ..config()
    };
    let err = run(&config, &get(&url), Vec::new())
        .await
        .expect_err("a server that never answers must trip the deadline");

    assert_eq!(
        err,
        FetchError::Timeout {
            url: url.clone(),
            timeout_ms: 250
        }
    );
    assert_eq!(
        err.js_error_class(),
        "TimeoutError",
        "the contract gives the deadline its own error name"
    );
}

/// `AbortSignal.timeout(ms)` reaches the transport as
/// `spec.timeout_ms` (issue #2016) and must NARROW the deadline only.
///
/// A `min` rather than an override is the whole point: divergence D1
/// exists because one hung `fetch` wedges the single SSR V8 thread, so
/// a bundle asking for a ten-minute deadline must not get one. Only the
/// operator may raise the ceiling, via `ZFB_SSR_FETCH_TIMEOUT_MS`.
#[test]
fn a_caller_requested_deadline_can_only_narrow_the_hosts_own() {
    // Narrower wins.
    assert_eq!(effective_timeout_ms(30_000, Some(250)), 250);
    // Wider is ignored — the host's ceiling stands.
    assert_eq!(effective_timeout_ms(30_000, Some(600_000)), 30_000);
    // Absent leaves the host's deadline untouched.
    assert_eq!(effective_timeout_ms(30_000, None), 30_000);
    // `0` means "as soon as possible", never "no deadline at all" —
    // a zero-duration `tokio::time::timeout` would still be honoured,
    // but 1ms keeps the value non-degenerate.
    assert_eq!(effective_timeout_ms(30_000, Some(0)), 1);
}

/// The narrowed deadline is the one the transport actually applies, and
/// the one the error message quotes — proved against a real socket, not
/// just the pure helper above.
#[tokio::test]
async fn a_caller_requested_deadline_is_the_one_the_transport_enforces() {
    let server = LoopbackServer::spawn(|_req, stream| async move {
        let _keep_the_socket_open = stream;
        std::future::pending::<()>().await;
    })
    .await;
    let url = server.url("/never");

    // The host's own deadline is deliberately far too long to be what
    // ends this call, so only the caller-requested one can.
    let config = FetchConfig {
        timeout_ms: 60_000,
        ..config()
    };
    let mut spec = get(&url);
    spec.timeout_ms = Some(250);

    let err = run(&config, &spec, Vec::new())
        .await
        .expect_err("the caller-requested deadline must trip");
    assert_eq!(
        err,
        FetchError::Timeout {
            url,
            timeout_ms: 250
        }
    );
}

// ---------------------------------------------------------------------
// 3. cancellation mid-body
// ---------------------------------------------------------------------

#[tokio::test]
async fn dropping_the_future_mid_body_cancels_the_request_and_closes_the_socket() {
    let (written, written_rx) = signal();
    let (closed, closed_rx) = signal();
    let server = LoopbackServer::spawn(move |_req, mut stream| {
        let written = written.clone();
        let closed = closed.clone();
        async move {
            // Promise 1024 bytes, deliver 16, then stop. The client is
            // now parked mid-body with no timer involved.
            let head = "HTTP/1.1 200 OK\r\ncontent-length: 1024\r\nconnection: close\r\n\r\n";
            let _ = stream.write_all(head.as_bytes()).await;
            let _ = stream.write_all(&[b'x'; 16]).await;
            let _ = stream.flush().await;
            fire(&written);

            // Report the moment the client's half of the connection
            // goes away.
            let mut sink = [0u8; 64];
            loop {
                match stream.read(&mut sink).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => continue,
                }
            }
            fire(&closed);
        }
    })
    .await;

    let client = client();
    let counter = SubrequestCounter::new();
    let config = config();
    let spec = get(&server.url("/slow"));
    let mut fetch = Box::pin(perform_fetch(&client, &counter, &config, &spec, Vec::new()));

    tokio::select! {
        _ = &mut fetch => panic!("the fetch resolved even though the body never finished"),
        signalled = written_rx => signalled.expect("server reported the partial write"),
    }

    // This is the cancellation: no abort API, just dropping the future.
    drop(fetch);

    tokio::time::timeout(std::time::Duration::from_secs(10), closed_rx)
        .await
        .expect("dropping the fetch future did not close the socket within 10s")
        .expect("server task ended without reporting the close");
}

// ---------------------------------------------------------------------
// 4. redirect-limit exceeded
// ---------------------------------------------------------------------

/// A server that redirects every request straight back to itself.
async fn endless_redirect_server(status: u16, reason: &'static str) -> LoopbackServer {
    LoopbackServer::spawn(move |_req, mut stream| async move {
        let _ = stream
            .write_all(&redirect_response(status, reason, "/again"))
            .await;
        let _ = stream.shutdown().await;
    })
    .await
}

#[tokio::test]
async fn redirect_limit_exceeded_rejects_and_names_the_limit() {
    let server = endless_redirect_server(302, "Found").await;
    let url = server.url("/start");
    let config = FetchConfig {
        max_redirects: 3,
        ..config()
    };

    let err = run(&config, &get(&url), Vec::new())
        .await
        .expect_err("an endless redirect chain must be refused");

    assert_eq!(
        err,
        FetchError::TooManyRedirects {
            url: url.clone(),
            limit: 3
        }
    );
    assert_eq!(
        server.request_count(),
        4,
        "the initial request plus exactly `max_redirects` follows"
    );
}

// ---------------------------------------------------------------------
// 5. redirect: "error"
// ---------------------------------------------------------------------

#[tokio::test]
async fn redirect_error_mode_turns_a_redirect_status_into_a_network_error() {
    let server = endless_redirect_server(301, "Moved Permanently").await;
    let url = server.url("/start");
    let mut spec = get(&url);
    spec.redirect = RedirectMode::Error;

    let err = run(&config(), &spec, Vec::new())
        .await
        .expect_err("redirect mode \"error\" must reject a redirect status");

    assert_eq!(err, FetchError::RedirectNotAllowed { url: url.clone() });
    assert_eq!(
        server.request_count(),
        1,
        "the hop is refused, never followed"
    );
}

#[tokio::test]
async fn redirect_manual_mode_returns_the_redirect_response_unchanged() {
    let server = endless_redirect_server(302, "Found").await;
    let mut spec = get(&server.url("/start"));
    spec.redirect = RedirectMode::Manual;

    let outcome = run(&config(), &spec, Vec::new())
        .await
        .expect("manual mode hands the redirect back");

    assert_eq!(outcome.status, 302);
    assert_eq!(outcome.header("location"), Some("/again"));
    assert!(
        !outcome.redirected,
        "manual mode follows nothing, so `redirected` stays false"
    );
    assert_eq!(server.request_count(), 1);
}

// ---------------------------------------------------------------------
// 6/7/8. method rewriting across redirect statuses
// ---------------------------------------------------------------------

/// `/` redirects to `/dest`; `/dest` answers 200.
async fn two_hop_server(status: u16, reason: &'static str) -> LoopbackServer {
    LoopbackServer::spawn(move |req, mut stream| async move {
        let bytes = if req.target == "/dest" {
            ok_response("arrived")
        } else {
            redirect_response(status, reason, "/dest")
        };
        let _ = stream.write_all(&bytes).await;
        let _ = stream.shutdown().await;
    })
    .await
}

fn body_spec(url: &str, method: &str) -> FetchRequestSpec {
    FetchRequestSpec {
        url: url.to_string(),
        method: method.to_string(),
        headers: vec![
            ("content-type".to_string(), "text/plain".to_string()),
            ("x-keep".to_string(), "yes".to_string()),
        ],
        redirect: RedirectMode::Follow,
        has_body: true,
        timeout_ms: None,
    }
}

#[tokio::test]
async fn redirect_301_rewrites_post_to_get_and_drops_the_body() {
    let server = two_hop_server(301, "Moved Permanently").await;
    let outcome = run(
        &config(),
        &body_spec(&server.url("/"), "POST"),
        b"payload".to_vec(),
    )
    .await
    .expect("the chain resolves");

    assert_eq!(outcome.status, 200);
    assert!(outcome.redirected);
    let requests = server.requests();
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].body, b"payload");
    assert_eq!(requests[1].method, "GET", "301 on POST becomes GET");
    assert!(requests[1].body.is_empty(), "the body is dropped");
    assert_eq!(
        requests[1].header("content-type"),
        None,
        "body-describing headers go with the body"
    );
    assert_eq!(
        requests[1].header("x-keep"),
        Some("yes"),
        "unrelated headers survive the rewrite"
    );
}

#[tokio::test]
async fn redirect_301_on_a_non_post_method_preserves_method_and_body() {
    // The other half of the 301/302 rule: only POST is rewritten.
    let server = two_hop_server(301, "Moved Permanently").await;
    run(
        &config(),
        &body_spec(&server.url("/"), "PUT"),
        b"payload".to_vec(),
    )
    .await
    .expect("the chain resolves");

    let requests = server.requests();
    assert_eq!(requests[1].method, "PUT");
    assert_eq!(requests[1].body, b"payload");
    assert_eq!(requests[1].header("content-type"), Some("text/plain"));
}

#[tokio::test]
async fn redirect_303_rewrites_put_to_get_and_drops_the_body() {
    let server = two_hop_server(303, "See Other").await;
    run(
        &config(),
        &body_spec(&server.url("/"), "PUT"),
        b"payload".to_vec(),
    )
    .await
    .expect("the chain resolves");

    let requests = server.requests();
    assert_eq!(requests[0].method, "PUT");
    assert_eq!(
        requests[1].method, "GET",
        "303 rewrites every method except GET/HEAD"
    );
    assert!(requests[1].body.is_empty());
    assert_eq!(requests[1].header("content-type"), None);
}

#[tokio::test]
async fn redirect_307_preserves_method_and_body() {
    let server = two_hop_server(307, "Temporary Redirect").await;
    run(
        &config(),
        &body_spec(&server.url("/"), "POST"),
        b"payload".to_vec(),
    )
    .await
    .expect("the chain resolves");

    let requests = server.requests();
    assert_eq!(requests[1].method, "POST", "307 preserves every method");
    assert_eq!(requests[1].body, b"payload", "and the body");
    assert_eq!(requests[1].header("content-type"), Some("text/plain"));
}

// ---------------------------------------------------------------------
// 9. 300 and 304 are NOT redirects
// ---------------------------------------------------------------------

#[tokio::test]
async fn status_300_and_304_are_ordinary_responses_in_every_redirect_mode() {
    // 300 deliberately carries a `location` too, so "has a Location
    // header" cannot be mistaken for "is a redirect".
    let multiple_choices = b"HTTP/1.1 300 Multiple Choices\r\nlocation: /other\r\ncontent-length: 6\r\nconnection: close\r\n\r\nchoose".to_vec();
    let not_modified =
        b"HTTP/1.1 304 Not Modified\r\netag: \"v1\"\r\nconnection: close\r\n\r\n".to_vec();

    for (raw, status, body) in [
        (multiple_choices, 300u16, b"choose".to_vec()),
        (not_modified, 304u16, Vec::new()),
    ] {
        for mode in [
            RedirectMode::Follow,
            RedirectMode::Manual,
            RedirectMode::Error,
        ] {
            let server = LoopbackServer::spawn_static(raw.clone()).await;
            let mut spec = get(&server.url("/"));
            spec.redirect = mode;

            let outcome = run(&config(), &spec, Vec::new()).await.unwrap_or_else(|e| {
                panic!("{status} must be an ordinary response under {mode:?}, got {e}")
            });

            assert_eq!(outcome.status, status);
            assert_eq!(outcome.body, body);
            assert!(
                !outcome.redirected,
                "{status} is not a redirect, so nothing was followed"
            );
            assert_eq!(
                server.request_count(),
                1,
                "{status} under {mode:?} must not be chased"
            );
        }
    }
}

#[test]
fn redirect_statuses_are_exactly_the_five() {
    for status in 300u16..=399 {
        let expected = matches!(status, 301 | 302 | 303 | 307 | 308);
        assert_eq!(
            is_redirect_status(status),
            expected,
            "status {status} classified wrong — \"any 3xx is a redirect\" is the classic bug here"
        );
    }
    assert!(!is_redirect_status(200));
    assert!(!is_redirect_status(404));
}

// ---------------------------------------------------------------------
// 10. cross-origin redirect strips auth
// ---------------------------------------------------------------------

fn credentialed_spec(url: &str) -> FetchRequestSpec {
    FetchRequestSpec {
        url: url.to_string(),
        method: "GET".to_string(),
        headers: vec![
            ("authorization".to_string(), "Bearer secret".to_string()),
            ("cookie".to_string(), "sid=abc".to_string()),
            ("x-keep".to_string(), "yes".to_string()),
        ],
        redirect: RedirectMode::Follow,
        has_body: false,
        timeout_ms: None,
    }
}

#[tokio::test]
async fn cross_origin_redirect_strips_authorization_and_cookie() {
    let upstream = LoopbackServer::spawn_static(ok_response("arrived")).await;
    let upstream_url = upstream.url("/dest");
    let redirector = LoopbackServer::spawn(move |_req, mut stream| {
        let target = upstream_url.clone();
        async move {
            let _ = stream
                .write_all(&redirect_response(302, "Found", &target))
                .await;
            let _ = stream.shutdown().await;
        }
    })
    .await;

    let outcome = run(
        &config(),
        &credentialed_spec(&redirector.url("/")),
        Vec::new(),
    )
    .await
    .expect("the cross-origin chain resolves");

    assert_eq!(outcome.status, 200);
    assert!(outcome.redirected);
    let landed = &upstream.requests()[0];
    assert_eq!(
        landed.header("authorization"),
        None,
        "credentials must not cross an origin boundary"
    );
    assert_eq!(landed.header("cookie"), None);
    assert_eq!(
        landed.header("x-keep"),
        Some("yes"),
        "only the credential headers are stripped"
    );
}

#[tokio::test]
async fn same_origin_redirect_keeps_authorization_and_cookie() {
    // The contrast case: without this, "strip everything, always" would
    // pass the test above while being wrong.
    let server = two_hop_server(302, "Found").await;
    run(&config(), &credentialed_spec(&server.url("/")), Vec::new())
        .await
        .expect("the same-origin chain resolves");

    let landed = &server.requests()[1];
    assert_eq!(landed.header("authorization"), Some("Bearer secret"));
    assert_eq!(landed.header("cookie"), Some("sid=abc"));
}

// ---------------------------------------------------------------------
// 11. oversized request body
// ---------------------------------------------------------------------

#[tokio::test]
async fn oversized_request_body_is_rejected_before_any_socket_is_opened() {
    let server = LoopbackServer::spawn_static(ok_response("never reached")).await;
    let url = server.url("/upload");
    let config = FetchConfig {
        max_request_body_bytes: 8,
        ..config()
    };
    let mut spec = get(&url);
    spec.method = "POST".to_string();
    spec.has_body = true;

    let err = run(&config, &spec, vec![b'x'; 9])
        .await
        .expect_err("a body over the cap must be refused");

    assert_eq!(
        err,
        FetchError::RequestBodyTooLarge {
            url: url.clone(),
            limit: 8
        }
    );
    assert_eq!(
        server.request_count(),
        0,
        "the oversized payload never reached the network"
    );
}

#[tokio::test]
async fn a_request_body_exactly_at_the_cap_is_allowed() {
    let server = LoopbackServer::spawn_static(ok_response("ok")).await;
    let config = FetchConfig {
        max_request_body_bytes: 8,
        ..config()
    };
    let mut spec = get(&server.url("/upload"));
    spec.method = "POST".to_string();
    spec.has_body = true;

    run(&config, &spec, vec![b'x'; 8])
        .await
        .expect("the cap is inclusive");
    assert_eq!(server.requests()[0].body.len(), 8);
}

// ---------------------------------------------------------------------
// 12. oversized response body — streamed, aborted early
// ---------------------------------------------------------------------

#[tokio::test]
async fn oversized_response_body_aborts_the_connection_mid_stream() {
    const CHUNK: usize = 64 * 1024;
    const CHUNKS: usize = 200; // 12.8 MB if the server is ever allowed to finish

    let written = Arc::new(AtomicUsize::new(0));
    let (done, done_rx) = signal();
    let server_written = written.clone();
    let server = LoopbackServer::spawn(move |_req, mut stream| {
        let written = server_written.clone();
        let done = done.clone();
        async move {
            // Chunked framing, so there is no `content-length` for the
            // client to reject up front — the cap has to be enforced on
            // the bytes themselves.
            let head = "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n";
            let _ = stream.write_all(head.as_bytes()).await;
            let payload = vec![b'x'; CHUNK];
            for _ in 0..CHUNKS {
                let frame = format!("{CHUNK:x}\r\n");
                if stream.write_all(frame.as_bytes()).await.is_err()
                    || stream.write_all(&payload).await.is_err()
                    || stream.write_all(b"\r\n").await.is_err()
                {
                    break;
                }
                written.fetch_add(CHUNK, Ordering::SeqCst);
            }
            let _ = stream.write_all(b"0\r\n\r\n").await;
            fire(&done);
        }
    })
    .await;

    let url = server.url("/firehose");
    let config = FetchConfig {
        max_response_body_bytes: 1024,
        ..config()
    };

    let err = run(&config, &get(&url), Vec::new())
        .await
        .expect_err("a response over the cap must be refused");
    assert_eq!(
        err,
        FetchError::ResponseBodyTooLarge {
            url: url.clone(),
            limit: 1024
        }
    );

    tokio::time::timeout(std::time::Duration::from_secs(60), done_rx)
        .await
        .expect("the server never finished writing")
        .expect("server task ended without signalling");

    // THE falsifiable assertion. Buffering the whole body and checking
    // afterwards would still produce the error above — but the server
    // would have been allowed to write all 12.8 MB. Aborting the
    // connection the moment the cap is crossed is what makes this a
    // resource-exhaustion guard rather than a cosmetic one.
    let total = written.load(Ordering::SeqCst);
    assert!(
        total < CHUNK * CHUNKS,
        "the connection was not aborted early: the server wrote all {} bytes",
        CHUNK * CHUNKS
    );
}

#[tokio::test]
async fn a_response_body_exactly_at_the_cap_is_allowed() {
    let server = LoopbackServer::spawn_static(ok_response("12345678")).await;
    let config = FetchConfig {
        max_response_body_bytes: 8,
        ..config()
    };
    let outcome = run(&config, &get(&server.url("/")), Vec::new())
        .await
        .expect("the cap is inclusive");
    assert_eq!(outcome.body, b"12345678");
}

// ---------------------------------------------------------------------
// 13. declared content-length above the cap
// ---------------------------------------------------------------------

#[tokio::test]
async fn content_length_above_the_cap_is_rejected_before_the_body_is_read() {
    // The server declares 100 000 bytes and then sends none of them,
    // holding the connection open forever. An implementation that read
    // the body before checking would sit here until the deadline, so
    // `Timeout` vs `ResponseBodyTooLarge` is a clean, sleep-free
    // discriminator for "checked before reading".
    let server = LoopbackServer::spawn(|_req, mut stream| async move {
        let head = "HTTP/1.1 200 OK\r\ncontent-length: 100000\r\nconnection: close\r\n\r\n";
        let _ = stream.write_all(head.as_bytes()).await;
        let _ = stream.flush().await;
        std::future::pending::<()>().await;
    })
    .await;
    let url = server.url("/liar");
    let config = FetchConfig {
        max_response_body_bytes: 1024,
        timeout_ms: 20_000,
        ..config()
    };

    let err = run(&config, &get(&url), Vec::new())
        .await
        .expect_err("a declared length over the cap must be refused");

    assert_eq!(
        err,
        FetchError::ResponseBodyTooLarge {
            url: url.clone(),
            limit: 1024
        },
        "a Timeout here would mean the body was being read before the check"
    );
}

// ---------------------------------------------------------------------
// 14. connection failure
// ---------------------------------------------------------------------

#[tokio::test]
async fn connection_failure_surfaces_the_transport_cause() {
    let url = closed_port_url().await;

    let err = run(&config(), &get(&url), Vec::new())
        .await
        .expect_err("nothing is listening on that port");

    match &err {
        FetchError::Transport { url: named, cause } => {
            assert_eq!(named, &url);
            assert!(
                !cause.is_empty(),
                "the transport's own message must survive to the caller"
            );
        }
        other => panic!("expected a transport failure, got {other:?}"),
    }
    assert_eq!(err.js_error_class(), "TypeError");
    assert!(err.to_string().starts_with(&format!("fetch({url}): ")));
}

// ---------------------------------------------------------------------
// 15. disallowed scheme
// ---------------------------------------------------------------------

#[tokio::test]
async fn disallowed_schemes_are_rejected_before_any_socket_is_opened() {
    for url in [
        "ftp://example.invalid/data",
        "file:///etc/passwd",
        "data:text/plain,hi",
        "ws://example.invalid/socket",
    ] {
        let err = run(&config(), &get(url), Vec::new())
            .await
            .expect_err("only http and https are allowed");
        assert_eq!(
            err,
            FetchError::DisallowedScheme {
                url: url.to_string()
            },
            "{url} must be refused"
        );
        assert_eq!(err.to_string(), format!("Fetch API cannot load: {url}"));
    }
}

#[tokio::test]
async fn a_redirect_into_a_disallowed_scheme_is_refused() {
    let server = LoopbackServer::spawn(|_req, mut stream| async move {
        let _ = stream
            .write_all(&redirect_response(302, "Found", "ftp://example.invalid/"))
            .await;
        let _ = stream.shutdown().await;
    })
    .await;
    let url = server.url("/start");

    let err = run(&config(), &get(&url), Vec::new())
        .await
        .expect_err("the scheme allowlist applies to every hop, not just the first");

    assert_eq!(
        err,
        FetchError::DisallowedScheme { url: url.clone() },
        "the message names the fetch the caller wrote, not the hop"
    );
}

// ---------------------------------------------------------------------
// 16. subrequest-count overflow (resource exhaustion)
// ---------------------------------------------------------------------

#[tokio::test]
async fn the_subrequest_budget_is_per_dispatch_and_counts_every_request() {
    let server = LoopbackServer::spawn_static(ok_response("ok")).await;
    let url = server.url("/");
    let config = FetchConfig {
        max_subrequests: 3,
        ..config()
    };
    let client = client();
    let counter = SubrequestCounter::new();

    for attempt in 1..=3 {
        perform_fetch(&client, &counter, &config, &get(&url), Vec::new())
            .await
            .unwrap_or_else(|e| panic!("subrequest {attempt} of 3 should be within budget: {e}"));
    }
    assert_eq!(counter.used(), 3);

    let err = perform_fetch(&client, &counter, &config, &get(&url), Vec::new())
        .await
        .expect_err("the fourth call is over budget");
    assert_eq!(
        err,
        FetchError::SubrequestLimit {
            url: url.clone(),
            limit: 3
        }
    );

    // A dispatch boundary allocates a NEW counter — which is what
    // `EmbeddedV8RenderHost::begin_dispatch_subrequest_budget` does, and
    // why an op orphaned by the previous dispatch cannot spend this
    // budget. The exhausted counter above stays exhausted.
    let next_dispatch = SubrequestCounter::new();
    perform_fetch(&client, &next_dispatch, &config, &get(&url), Vec::new())
        .await
        .expect("a new dispatch starts with a full budget");
    assert_eq!(next_dispatch.used(), 1);
    assert_eq!(
        counter.used(),
        3,
        "the previous dispatch's spend is not rewritten"
    );
}

#[tokio::test]
async fn every_redirect_hop_claims_a_subrequest_slot() {
    let server = endless_redirect_server(302, "Found").await;
    let url = server.url("/start");
    let config = FetchConfig {
        max_subrequests: 2,
        max_redirects: 50, // deliberately not the limit under test
        ..config()
    };
    let counter = SubrequestCounter::new();

    let err = perform_fetch(&client(), &counter, &config, &get(&url), Vec::new())
        .await
        .expect_err("the chain exhausts the subrequest budget");

    assert_eq!(
        err,
        FetchError::SubrequestLimit {
            url: url.clone(),
            limit: 2
        }
    );
    assert_eq!(counter.used(), 2);
    assert_eq!(
        server.request_count(),
        2,
        "one hop, one slot — a redirect chain cannot outrun the budget"
    );
}

// ---------------------------------------------------------------------
// 17. host-op failure
// ---------------------------------------------------------------------

#[test]
fn a_runtime_without_the_fetch_extension_reports_host_unavailable() {
    // The runtime is shutting down, or was built without the extension:
    // either way the op has no client to use. It must REJECT. Resolving
    // to a synthetic empty `Response` would be indistinguishable from a
    // real 200 with no content — exactly the dev/prod divergence this
    // epic exists to remove, and why the op's return type is a `Result`
    // with no fallback arm.
    let bare = Rc::new(std::cell::RefCell::new(OpState::new(None)));
    let err = state_handles(&bare, "http://127.0.0.1:1/")
        .expect_err("a runtime without the extension has no transport");

    match &err {
        FetchError::HostUnavailable { url, detail } => {
            assert_eq!(url, "http://127.0.0.1:1/");
            assert!(detail.contains("client"), "detail was {detail:?}");
        }
        other => panic!("expected a host-op failure, got {other:?}"),
    }
    assert_eq!(
        err.to_string(),
        "fetch(http://127.0.0.1:1/): embedded host transport unavailable: \
         outbound HTTP client is not installed in this runtime"
    );
    assert_eq!(err.js_error_class(), "TypeError");
}

#[test]
fn a_runtime_missing_only_the_counter_also_reports_host_unavailable() {
    let state = Rc::new(std::cell::RefCell::new(OpState::new(None)));
    state.borrow_mut().put(FetchClient(client()));
    let err = state_handles(&state, "http://127.0.0.1:1/")
        .expect_err("a half-installed extension is still a host-op failure");
    assert!(
        err.to_string()
            .contains("subrequest counter is not installed"),
        "got {err}"
    );
}

#[test]
fn a_fully_installed_state_resolves_both_handles() {
    let state = Rc::new(std::cell::RefCell::new(OpState::new(None)));
    state.borrow_mut().put(FetchClient(client()));
    state.borrow_mut().put(Rc::new(SubrequestCounter::new()));
    let (_client, counter) =
        state_handles(&state, "http://127.0.0.1:1/").expect("both handles are present");
    assert_eq!(counter.used(), 0);
}

// ---------------------------------------------------------------------
// 18. duplicate set-cookie survives
// ---------------------------------------------------------------------

#[tokio::test]
async fn duplicate_set_cookie_headers_survive_the_boundary() {
    let raw = b"HTTP/1.1 200 OK\r\nset-cookie: a=1; Path=/\r\nset-cookie: b=2; Path=/\r\ncontent-length: 2\r\nconnection: close\r\n\r\nhi".to_vec();
    let server = LoopbackServer::spawn_static(raw).await;

    let outcome = run(&config(), &get(&server.url("/")), Vec::new())
        .await
        .expect("a 200 with two cookies");

    assert_eq!(
        outcome.header_all("set-cookie"),
        vec!["a=1; Path=/", "b=2; Path=/"],
        "headers cross as an ordered pair list, never a map — a map would \
         collapse these two into one"
    );
    // And the shape itself, so a future refactor to a map is caught even
    // if it happened to keep the last value.
    assert_eq!(
        outcome
            .headers
            .iter()
            .filter(|(k, _)| k == "set-cookie")
            .count(),
        2
    );
}

// ---------------------------------------------------------------------
// transport hygiene: hop-by-hop headers, accept-encoding
// ---------------------------------------------------------------------

#[tokio::test]
async fn hop_by_hop_headers_are_dropped_and_no_accept_encoding_is_sent() {
    let server = LoopbackServer::spawn_static(ok_response("ok")).await;
    let mut spec = get(&server.url("/"));
    spec.headers = vec![
        ("Host".to_string(), "evil.example".to_string()),
        ("connection".to_string(), "upgrade".to_string()),
        ("transfer-encoding".to_string(), "chunked".to_string()),
        ("content-length".to_string(), "999".to_string()),
        ("upgrade".to_string(), "websocket".to_string()),
        ("keep-alive".to_string(), "timeout=5".to_string()),
        ("proxy-authenticate".to_string(), "Basic".to_string()),
        ("proxy-authorization".to_string(), "Basic zzz".to_string()),
        ("te".to_string(), "trailers".to_string()),
        ("trailer".to_string(), "x-checksum".to_string()),
        ("x-keep".to_string(), "yes".to_string()),
    ];

    run(&config(), &spec, Vec::new())
        .await
        .expect("the request goes through");

    let request = &server.requests()[0];
    assert_eq!(
        request.header("host"),
        Some(server.addr().to_string().as_str()),
        "`host` is recomputed by the transport, never taken from the caller"
    );
    for dropped in [
        "transfer-encoding",
        "content-length",
        "upgrade",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
    ] {
        assert_eq!(
            request.header(dropped),
            None,
            "hop-by-hop header {dropped:?} must not be forwarded; saw {:?}",
            request.header_names()
        );
    }
    assert_ne!(
        request.header("connection"),
        Some("upgrade"),
        "the caller must not be able to dictate connection handling"
    );
    assert_eq!(
        request.header("accept-encoding"),
        None,
        "no accept-encoding unless the caller sets one (divergence D6) — \
         otherwise response bytes stop being verbatim"
    );
    assert_eq!(request.header("x-keep"), Some("yes"));
}

#[tokio::test]
async fn a_caller_supplied_accept_encoding_is_forwarded() {
    let server = LoopbackServer::spawn_static(ok_response("ok")).await;
    let mut spec = get(&server.url("/"));
    spec.headers = vec![("accept-encoding".to_string(), "identity".to_string())];

    run(&config(), &spec, Vec::new())
        .await
        .expect("goes through");

    assert_eq!(
        server.requests()[0].header("accept-encoding"),
        Some("identity")
    );
}

// ---------------------------------------------------------------------
// the async property, the contract's numbers, and its exact messages
// ---------------------------------------------------------------------

#[test]
fn the_op_is_registered_as_an_async_op() {
    // Read off deno_core's own `OpDecl`, so this cannot be satisfied by
    // a comment or a naming convention. A synchronous op here would put
    // a blocking network call on the V8 isolate thread and stall every
    // concurrent render — guardrail 1 of epic #2012.
    assert!(
        op_is_async(),
        "op_zfb_fetch must be an async op; a sync op would block the isolate thread"
    );
    assert_eq!(op_zfb_fetch().name, "op_zfb_fetch");
}

#[test]
fn the_default_config_carries_the_contract_limits() {
    let config = FetchConfig::default();
    assert_eq!(config.timeout_ms, limits::fetch_timeout_ms());
    assert_eq!(config.max_redirects, limits::MAX_REDIRECTS);
    assert_eq!(
        config.max_request_body_bytes,
        limits::MAX_REQUEST_BODY_BYTES
    );
    assert_eq!(
        config.max_response_body_bytes,
        limits::MAX_RESPONSE_BODY_BYTES
    );
    assert_eq!(config.max_subrequests, limits::MAX_SUBREQUESTS_PER_DISPATCH);
}

#[test]
fn error_messages_match_the_2013_contract_verbatim() {
    // #2016's JS layer and #2018's diagnostics both quote these strings,
    // and the contract writes them out in full with the DEFAULT limits
    // substituted — so render each against the default limits, not the
    // small ones the cap tests use.
    let url = "https://api.example.com/v1".to_string();
    let cases: Vec<(FetchError, &str)> = vec![
        (
            FetchError::DisallowedScheme { url: url.clone() },
            "Fetch API cannot load: https://api.example.com/v1",
        ),
        (
            FetchError::TooManyRedirects {
                url: url.clone(),
                limit: limits::MAX_REDIRECTS,
            },
            "fetch(https://api.example.com/v1): too many redirects (limit 20)",
        ),
        (
            FetchError::RedirectNotAllowed { url: url.clone() },
            "fetch(https://api.example.com/v1): redirect not allowed (redirect mode is \"error\")",
        ),
        (
            FetchError::RequestBodyTooLarge {
                url: url.clone(),
                limit: limits::MAX_REQUEST_BODY_BYTES,
            },
            "fetch(https://api.example.com/v1): request body exceeds the 104857600-byte limit",
        ),
        (
            FetchError::ResponseBodyTooLarge {
                url: url.clone(),
                limit: limits::MAX_RESPONSE_BODY_BYTES,
            },
            "fetch(https://api.example.com/v1): response body exceeds the 104857600-byte limit",
        ),
        (
            FetchError::Timeout {
                url: url.clone(),
                timeout_ms: limits::DEFAULT_FETCH_TIMEOUT_MS,
            },
            "fetch(https://api.example.com/v1): timed out after 30000ms (zfb embedded-runtime \
             request-time limit; production Cloudflare Workers has no per-subrequest timeout)",
        ),
        (
            FetchError::SubrequestLimit {
                url: url.clone(),
                limit: limits::MAX_SUBREQUESTS_PER_DISPATCH,
            },
            "fetch(https://api.example.com/v1): exceeded the 50-subrequest limit for a single \
             request",
        ),
        (
            FetchError::Transport {
                url: url.clone(),
                cause: "connection refused".to_string(),
            },
            "fetch(https://api.example.com/v1): connection refused",
        ),
        (
            FetchError::HostUnavailable {
                url: url.clone(),
                detail: "channel closed".to_string(),
            },
            "fetch(https://api.example.com/v1): embedded host transport unavailable: channel \
             closed",
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }
}

#[test]
fn only_the_deadline_carries_a_non_type_error_name() {
    let url = "https://example.com/".to_string();
    assert_eq!(
        FetchError::Timeout {
            url: url.clone(),
            timeout_ms: 1
        }
        .js_error_class(),
        "TimeoutError"
    );
    for error in [
        FetchError::DisallowedScheme { url: url.clone() },
        FetchError::InvalidUrl { url: url.clone() },
        FetchError::TooManyRedirects {
            url: url.clone(),
            limit: 20,
        },
        FetchError::RedirectNotAllowed { url: url.clone() },
        FetchError::RequestBodyTooLarge {
            url: url.clone(),
            limit: 1,
        },
        FetchError::ResponseBodyTooLarge {
            url: url.clone(),
            limit: 1,
        },
        FetchError::SubrequestLimit {
            url: url.clone(),
            limit: 1,
        },
        FetchError::Transport {
            url: url.clone(),
            cause: "x".into(),
        },
        FetchError::HostUnavailable {
            url: url.clone(),
            detail: "x".into(),
        },
    ] {
        assert_eq!(
            error.js_error_class(),
            "TypeError",
            "every non-deadline failure is a Fetch network error: {error}"
        );
    }
}

#[test]
fn redirect_mode_parses_from_the_spec_spellings() {
    for (json, expected) in [
        ("\"follow\"", RedirectMode::Follow),
        ("\"manual\"", RedirectMode::Manual),
        ("\"error\"", RedirectMode::Error),
    ] {
        assert_eq!(
            serde_json::from_str::<RedirectMode>(json).expect("valid mode"),
            expected
        );
    }
    assert_eq!(RedirectMode::default(), RedirectMode::Follow);
}

#[test]
fn method_rewriting_follows_the_contract_table() {
    use reqwest::Method;
    let cases = [
        (303, Method::PUT, Method::GET, true),
        (303, Method::POST, Method::GET, true),
        (303, Method::DELETE, Method::GET, true),
        (303, Method::GET, Method::GET, false),
        (303, Method::HEAD, Method::HEAD, false),
        (301, Method::POST, Method::GET, true),
        (302, Method::POST, Method::GET, true),
        (301, Method::PUT, Method::PUT, false),
        (302, Method::DELETE, Method::DELETE, false),
        (307, Method::POST, Method::POST, false),
        (308, Method::POST, Method::POST, false),
        (307, Method::PUT, Method::PUT, false),
    ];
    for (status, from, expected_method, expected_drop) in cases {
        let (method, drop_body) = rewrite_method_for_redirect(status, &from);
        assert_eq!(
            (method, drop_body),
            (expected_method, expected_drop),
            "status {status} with method {from}"
        );
    }
}
