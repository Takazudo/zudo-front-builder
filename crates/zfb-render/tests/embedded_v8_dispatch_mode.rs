//! Per-dispatch mode plumbing for the embedded V8 host (issue #2014,
//! epic #2012 — contract:
//! `research/2013-request-time-capability-contract.md`, "Mode
//! distinction").
//!
//! These tests exist because a mode flag that is *plumbed but never
//! read* would pass a naive test. Each one is written so that a flag
//! stuck on a single value fails:
//!
//! - [`build_time_dispatch_rejects_fetch_with_the_byte_identical_ssg_message`]
//!   and [`request_time_dispatch_reaches_a_distinct_branch`] run the
//!   SAME probe bundle two ways and assert two DIFFERENT outcomes, so
//!   neither a stuck `BuildTime` nor a stuck `RequestTime` can satisfy
//!   both.
//! - [`throwing_request_time_dispatch_restores_the_mode`]
//!   is the leak test: it drives a request-time dispatch that THROWS
//!   and then asserts the next build-time dispatch is still denied.
//!   Without the `finally` in `globals_shim.js`'s `dispatch`, the
//!   request-time value survives on the shared host and that assertion
//!   fails.
//! - [`mode_is_absent_outside_any_dispatch_and_reads_as_build_time`]
//!   pins the fail-safe default: at module-evaluation time no dispatch
//!   is active, `__zfb.mode` is `undefined`, and `fetch` must still
//!   take the SSG branch.
//!
//! Level 1/3 by the zudo-test-wisdom ladder: a real V8 isolate running
//! the real shim + polyfill sources, no external process. Blind spot
//! stated plainly: nothing here proves the *Rust* call-site wiring in
//! `crates/zfb/src/ssr_adapter.rs` — that lives in `zfb`'s own tests,
//! and the trait seam's build-time default is pinned in
//! `crates/zfb-build/src/renderer.rs`.

#![cfg(feature = "embed_v8")]

use zfb_render::{DispatchMode, EmbeddedV8RenderHost, HttpRequestLike, RenderHost};

/// The exact text `web_polyfills.js` rejects a build-time `fetch()`
/// with, minus the URL the caller passed. Guardrail 4 of epic #2012:
/// the SSG denial is deliberate policy and must not become collateral
/// damage of the mode split, so this literal is asserted byte-for-byte
/// rather than by substring.
const SSG_DENIAL_TAIL: &str = "). The embedded V8 host does not support outgoing network requests during build-time render. Move the data fetch to a build step or a runtime-only branch.";

fn expected_ssg_denial(url: &str) -> String {
    format!("fetch() called from SSG runtime (url={url}{SSG_DENIAL_TAIL}")
}

/// The URL the probe bundle's in-handler `fetch()` targets. `.invalid`
/// is reserved by RFC 2606 and is never resolvable — but nothing here
/// opens a socket anyway: both branches reject before any transport.
const PROBE_URL: &str = "http://upstream.invalid/data";

/// A bundle that reports, per route, what `fetch()` did and what
/// `__zfb.mode` said while the handler ran.
///
/// Also records — at MODULE EVALUATION time, i.e. outside any dispatch
/// — the value of `__zfb.mode` and the rejection an unguarded
/// module-scope `fetch()` produced, so the "absent mode" default can be
/// observed from a later dispatch instead of racing it.
fn probe_bundle() -> String {
    format!(
        r#"
        const moduleLoadMode = String(globalThis.__zfb && globalThis.__zfb.mode);
        let moduleLoadFetchError = "(still pending)";
        const moduleLoadFetchProbe = fetch({probe_url:?}).then(
          () => {{ moduleLoadFetchError = "(resolved - fetch must never resolve here)"; }},
          (e) => {{ moduleLoadFetchError = String(e.message); }},
        );

        export default {{
          async fetch(request) {{
            const url = new URL(request.url);
            if (url.pathname === "/module-load-probe") {{
              await moduleLoadFetchProbe;
              return new Response(
                JSON.stringify({{ mode: moduleLoadMode, caught: moduleLoadFetchError }}),
                {{ status: 200, headers: {{ "content-type": "application/json" }} }},
              );
            }}
            if (url.pathname === "/leak-readback") {{
              // Reads what the side module recorded at ITS module-eval
              // time — see `leak_probe_side_module`.
              const lp = globalThis.__zfbLeakProbe;
              await lp.promise;
              return new Response(
                JSON.stringify({{ mode: lp.mode, caught: lp.caught }}),
                {{ status: 200, headers: {{ "content-type": "application/json" }} }},
              );
            }}
            if (url.pathname === "/throw") {{
              // Throws BEFORE returning a Response, so the dispatch
              // promise rejects and only the `finally` can restore the
              // mode.
              throw new Error("deliberate throw from a request-time dispatch");
            }}
            let caught = "(fetch did not reject)";
            try {{
              await fetch({probe_url:?});
            }} catch (e) {{
              caught = String(e.message);
            }}
            return new Response(
              JSON.stringify({{ mode: String(globalThis.__zfb.mode), caught }}),
              {{ status: 200, headers: {{ "content-type": "application/json" }} }},
            );
          }},
        }};
        "#,
        probe_url = PROBE_URL,
    )
}

/// `{ mode, caught }` as the probe bundle reports it.
#[derive(Debug, serde::Deserialize)]
struct Probe {
    mode: String,
    caught: String,
}

async fn boot_probe_host() -> EmbeddedV8RenderHost {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    host.execute_module("bundle.mjs", &probe_bundle())
        .await
        .expect("execute probe bundle");
    host
}

async fn probe(host: &mut EmbeddedV8RenderHost, path: &str, mode: DispatchMode) -> Probe {
    let mut req = HttpRequestLike::get(format!("http://zfb.local{path}"));
    req.mode = mode;
    let resp = host.dispatch_fetch(req).await.expect("dispatch");
    assert_eq!(resp.status, 200, "probe route must answer 200");
    serde_json::from_str(resp.body_utf8().expect("probe body is UTF-8"))
        .expect("probe body is JSON")
}

#[tokio::test]
async fn build_time_dispatch_rejects_fetch_with_the_byte_identical_ssg_message() {
    let mut host = boot_probe_host().await;
    let got = probe(&mut host, "/", DispatchMode::BuildTime).await;
    assert_eq!(
        got.mode, "build-time",
        "a build-time dispatch must publish `__zfb.mode = \"build-time\"`"
    );
    assert_eq!(
        got.caught,
        expected_ssg_denial(PROBE_URL),
        "the build-time SSG denial must be byte-identical to the pre-#2014 message \
         (guardrail 4: it is deliberate policy, not collateral damage)"
    );
}

#[tokio::test]
async fn request_time_dispatch_reaches_a_distinct_branch() {
    let mut host = boot_probe_host().await;
    let got = probe(&mut host, "/", DispatchMode::RequestTime).await;
    assert_eq!(
        got.mode, "request-time",
        "a request-time dispatch must publish `__zfb.mode = \"request-time\"`"
    );
    assert!(
        got.caught.contains("request-time SSR runtime"),
        "request-time fetch must reject AS a request-time branch, got: {}",
        got.caught
    );
    assert!(
        !got.caught.contains("SSG runtime"),
        "the request-time branch must not reuse the SSG denial, got: {}",
        got.caught
    );
    assert_ne!(
        got.caught,
        expected_ssg_denial(PROBE_URL),
        "the two branches must be distinguishable by the message alone"
    );
}

/// A SIDE module that captures `__zfb.mode` and what `fetch()` did AT
/// ITS OWN MODULE-EVALUATION time — i.e. outside any dispatch — into a
/// global the main bundle's `/leak-readback` route hands back.
///
/// This is the observation point that makes the `finally` restore
/// falsifiable. Reading the leak from the *next dispatch's* handler
/// cannot work: every dispatch assigns `__zfb.mode` on entry, so it
/// overwrites a leaked value before any handler code runs. Only JS that
/// executes BETWEEN dispatches can see the residue.
///
/// It is loaded via `validate_worker_module_shape`, a real production
/// seam (the dev content-trace boot validates the inner worker on the
/// same host that later serves both build-time prerenders and
/// request-time SSR), so it needs a well-shaped `default.fetch` even
/// though nothing dispatches to it.
fn leak_probe_side_module() -> String {
    format!(
        r#"
        globalThis.__zfbLeakProbe = {{
          mode: String(globalThis.__zfb && globalThis.__zfb.mode),
          caught: "(still pending)",
        }};
        globalThis.__zfbLeakProbe.promise = fetch({probe_url:?}).then(
          () => {{ globalThis.__zfbLeakProbe.caught = "(resolved - fetch must never resolve here)"; }},
          (e) => {{ globalThis.__zfbLeakProbe.caught = String(e.message); }},
        );
        export default {{
          fetch() {{ return new Response("side module is never dispatched to"); }},
        }};
        "#,
        probe_url = PROBE_URL,
    )
}

/// The leak test — the one that catches a missing `finally`.
///
/// A request-time dispatch that THROWS must still restore `__zfb.mode`.
/// `crates/zfb/src/ssr_adapter.rs` and the dev tick's prerender pass
/// share one `EmbeddedV8RenderHost` instance, so a mode left behind is
/// request-time capability granted to code that never asked for it.
///
/// The residue is read from a module evaluated AFTER the throwing
/// dispatch, because that is the only place it is observable — see
/// [`leak_probe_side_module`]. What this proves: the restore runs, and
/// any JS evaluated outside a dispatch on this host sees build-time.
/// What it does not prove: anything about the Rust call sites.
#[tokio::test]
async fn throwing_request_time_dispatch_restores_the_mode() {
    let mut host = boot_probe_host().await;

    // Sanity: the host starts out denying build-time fetch.
    let before = probe(&mut host, "/", DispatchMode::BuildTime).await;
    assert_eq!(before.caught, expected_ssg_denial(PROBE_URL));

    // A request-time dispatch whose handler throws. The dispatch itself
    // must fail — if it ever starts succeeding this test is no longer
    // exercising the throwing path and must be rewritten, not relaxed.
    let mut throwing = HttpRequestLike::get("http://zfb.local/throw");
    throwing.mode = DispatchMode::RequestTime;
    let err = host
        .dispatch_fetch(throwing)
        .await
        .expect_err("the /throw route must make the dispatch fail");
    assert!(
        err.to_string()
            .contains("deliberate throw from a request-time dispatch"),
        "expected the handler's own throw to surface, got: {err}"
    );

    // Evaluate a module on the same host, between dispatches. Without
    // the `finally` in `globals_shim.js`'s `dispatch`, this module runs
    // with `__zfb.mode` still set to "request-time".
    host.validate_worker_module_shape("leak-probe.mjs", &leak_probe_side_module())
        .await
        .expect("evaluate leak-probe side module");
    let got = probe(&mut host, "/leak-readback", DispatchMode::BuildTime).await;
    assert_eq!(
        got.mode, "undefined",
        "a throwing request-time dispatch left `__zfb.mode` set for code that runs between \
         dispatches — the `finally` restore in globals_shim.js is missing or broken"
    );
    assert_eq!(
        got.caught,
        expected_ssg_denial(PROBE_URL),
        "module-scope fetch after a throwing request-time dispatch must hit the SSG denial"
    );
}

/// The non-throwing half of the same restore: a request-time dispatch
/// that returns normally must also not leave the mode behind. Same
/// module-evaluation observation point, for the same reason.
#[tokio::test]
async fn returning_request_time_dispatch_restores_the_mode() {
    let mut host = boot_probe_host().await;
    let during = probe(&mut host, "/", DispatchMode::RequestTime).await;
    assert_eq!(during.mode, "request-time");

    host.validate_worker_module_shape("leak-probe.mjs", &leak_probe_side_module())
        .await
        .expect("evaluate leak-probe side module");
    let got = probe(&mut host, "/leak-readback", DispatchMode::BuildTime).await;
    assert_eq!(
        got.mode, "undefined",
        "a returning request-time dispatch left `__zfb.mode` set for code that runs between \
         dispatches"
    );
    assert_eq!(got.caught, expected_ssg_denial(PROBE_URL));
}

/// Belt-and-braces: after a throwing request-time dispatch the NEXT
/// build-time dispatch is still denied. This one cannot fail on its own
/// if the `finally` is dropped (the next dispatch reassigns the mode on
/// entry), so it is a regression guard for the assignment path, not the
/// restore — [`throwing_request_time_dispatch_restores_the_mode`] is
/// what covers the restore.
#[tokio::test]
async fn build_time_dispatch_after_a_throwing_request_time_dispatch_is_still_denied() {
    let mut host = boot_probe_host().await;
    let mut throwing = HttpRequestLike::get("http://zfb.local/throw");
    throwing.mode = DispatchMode::RequestTime;
    host.dispatch_fetch(throwing)
        .await
        .expect_err("the /throw route must make the dispatch fail");

    let after = probe(&mut host, "/", DispatchMode::BuildTime).await;
    assert_eq!(after.mode, "build-time");
    assert_eq!(after.caught, expected_ssg_denial(PROBE_URL));
}

/// Fail-safe default: outside any dispatch — here, during module
/// evaluation — `__zfb.mode` is absent and `fetch` still takes the SSG
/// branch. This is the JS-side half of "any call site not explicitly
/// updated keeps the existing SSG denial".
#[tokio::test]
async fn mode_is_absent_outside_any_dispatch_and_reads_as_build_time() {
    let mut host = boot_probe_host().await;
    // The route is requested with RequestTime deliberately: the values
    // being read were captured at module-evaluation time, so a
    // request-time REQUEST must not be able to retroactively change
    // them.
    let got = probe(&mut host, "/module-load-probe", DispatchMode::RequestTime).await;
    assert_eq!(
        got.mode, "undefined",
        "`__zfb.mode` must be absent outside any dispatch"
    );
    assert_eq!(
        got.caught,
        expected_ssg_denial(PROBE_URL),
        "with `__zfb.mode` absent the polyfill must fall back to the denying build-time branch"
    );
}

/// `HttpRequestLike::get` — the SSG hot-path constructor — is
/// build-time, so an un-updated Rust call site keeps the existing
/// denial without the JS default ever being consulted.
#[tokio::test]
async fn http_request_like_get_defaults_to_build_time_end_to_end() {
    let mut host = boot_probe_host().await;
    // NOTE: no `req.mode = ...` here — that is the point.
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    let got: Probe =
        serde_json::from_str(resp.body_utf8().expect("probe body is UTF-8")).expect("probe JSON");
    assert_eq!(got.mode, "build-time");
    assert_eq!(got.caught, expected_ssg_denial(PROBE_URL));
}
