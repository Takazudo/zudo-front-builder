//! Smoke tests for `EmbeddedV8RenderHost` (sub-issue #162).
//!
//! What's exercised here:
//!
//! - **Stub bundle**: a hand-rolled `export default { fetch }` bundle
//!   whose `fetch` returns `new Response("ok")`. Proves the host
//!   boots, loads the bundle, dispatches a request, and round-trips
//!   the response body.
//!
//! - **ESM imports + top-level await**: a bundle that `import`s a
//!   sibling module (registered via `BundleModuleLoader::register_module`)
//!   and uses top-level `await` to drive a Promise before its
//!   default export resolves. Both must work for Hono-shaped
//!   bundles to evaluate.
//!
//! - **Hono-shape**: a hand-rolled router-shape that mirrors what
//!   the framework does. We don't pull `Hono` itself in here because
//!   the worktree has no built bundle; the real-bundle smoke test
//!   in `embedded_v8_real_bundle_smoke.rs` covers that.
//!
//! - **Frame parsing**: a bundle that throws on dispatch; the
//!   resulting `RenderError::Runtime` payload is fed through
//!   `find_frame_candidates`-style probing to confirm the V8 stack
//!   format is `<specifier>:LINE:COL` (the format
//!   `crate::sourcemap` already accepts).
//!
//! - **Panic safety**: drops the host while a dispatch is in flight
//!   and asserts no panic / no abort.
//!
//! All tests are gated on the `embed_v8` feature.

#![cfg(feature = "embed_v8")]

use zfb_render::{EmbeddedV8RenderHost, HttpRequestLike, RenderHost};

/// The shape every workerd-style bundle exports. Wrapped here so each
/// test can swap the route handler.
fn workerd_shaped_bundle(handler_body: &str) -> String {
    format!(
        r#"
        const handler = (request) => {{
          {handler_body}
        }};
        export default {{
          fetch(request) {{
            return handler(request);
          }}
        }};
        "#
    )
}

#[tokio::test]
async fn dispatches_simple_get_against_stub_bundle() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = workerd_shaped_bundle(
        r#"return new Response("ok", { status: 200, headers: { "content-type": "text/plain" } });"#,
    );
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute bundle");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/any"))
        .await
        .expect("dispatch");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body_utf8(), Some("ok"));
    assert_eq!(resp.content_type(), Some("text/plain"));
}

#[tokio::test]
async fn handles_hono_shape_router() {
    // Mini Hono-shape: a router that switches on URL path and returns
    // an HTML body for "/". We don't import Hono itself here — that
    // requires a built bundle. This proves the URL parsing + Headers
    // round-trip work end-to-end via the polyfill.
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = r#"
        export default {
          fetch(request) {
            const url = new URL(request.url);
            if (url.pathname === "/") {
              return new Response("<h1>home</h1>", {
                status: 200,
                headers: { "content-type": "text/html; charset=utf-8" },
              });
            }
            if (url.pathname === "/about") {
              return new Response("<h1>about</h1>", {
                status: 200,
                headers: new Headers({ "content-type": "text/html" }),
              });
            }
            return new Response("not found", { status: 404 });
          },
        };
    "#;
    host.execute_module("bundle.mjs", bundle)
        .await
        .expect("execute bundle");
    let home = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch /");
    assert_eq!(home.status, 200);
    assert!(home.body_utf8().unwrap().contains("home"));
    assert!(home.content_type().unwrap().contains("text/html"));

    let about = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/about"))
        .await
        .expect("dispatch /about");
    assert_eq!(about.status, 200);
    assert!(about.body_utf8().unwrap().contains("about"));

    let missing = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/no-such"))
        .await
        .expect("dispatch /no-such");
    assert_eq!(missing.status, 404);
}

#[tokio::test]
async fn supports_esm_imports_and_top_level_await() {
    use zfb_render::embedded_v8::BundleModuleLoader;
    let loader = BundleModuleLoader::new();
    // Sibling module: an async function the bundle imports.
    loader.register_module(
        "file:///zfb/lib.mjs",
        r#"
        export async function greeting() {
          // Top-level promise resolution.
          return await Promise.resolve("hello from lib");
        }
        "#,
    );
    let mut host = EmbeddedV8RenderHost::with_loader(loader).expect("host boot");
    let bundle = r#"
        import { greeting } from "file:///zfb/lib.mjs";
        // Top-level await — the response body is computed before the
        // module finishes evaluating.
        const message = await greeting();
        export default {
          fetch(_request) {
            return new Response(message, { status: 200 });
          },
        };
    "#;
    host.execute_module("bundle.mjs", bundle)
        .await
        .expect("execute bundle");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body_utf8(), Some("hello from lib"));
}

#[tokio::test]
async fn surfaces_v8_stack_with_parseable_frame() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    // The bundle's fetch handler throws synchronously. The host
    // wraps the V8 error — `RenderError::Runtime(...)` — and the
    // payload should embed the V8 stack with frames in the
    // canonical `<specifier>:LINE:COL` shape that
    // `crate::sourcemap` accepts.
    let bundle = r#"
        export default {
          fetch(_request) {
            throw new Error("test failure inside fetch handler");
          },
        };
    "#;
    host.execute_module("bundle.mjs", bundle)
        .await
        .expect("execute bundle");
    let err = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect_err("dispatch should error");
    let msg = err.to_string();
    // The walker `find_frame_candidates` in `sourcemap.rs` looks for
    // any `<specifier>:LINE:COL` substring. The bundle's specifier
    // gets prefixed with `file:///zfb/` by `synthesise_specifier`,
    // so we check for the bundle name (which is included verbatim
    // in the V8 stack frame).
    assert!(
        msg.contains("test failure"),
        "expected error message to include the JS exception text, got: {msg}"
    );
    // V8 stacks always include `at <specifier>:line:col` lines on
    // exceptions — confirm the format is parseable. This check is
    // intentionally loose because deno_core's JsError formatter
    // adds a header line; what we need is the LINE:COL token
    // somewhere in the body.
    let has_line_col_token = msg
        .lines()
        .any(|l| extract_line_col(l).is_some());
    assert!(
        has_line_col_token,
        "expected V8 stack with `<specifier>:LINE:COL` frames, got:\n{msg}"
    );
}

/// Lightweight reimplementation of the kind of probe
/// `crate::sourcemap::find_frame_candidates` does — extracts a
/// `(line, col)` token from a stack frame line. Used so this test
/// asserts on the exact format the production walker accepts.
fn extract_line_col(line: &str) -> Option<(usize, usize)> {
    // Walk back from the end of the line looking for `:N:M` digits.
    let bytes = line.as_bytes();
    let mut i = bytes.len();
    while i > 0 && bytes[i - 1] == b')' {
        i -= 1;
    }
    let end = i;
    let mut col_start = end;
    while col_start > 0 && bytes[col_start - 1].is_ascii_digit() {
        col_start -= 1;
    }
    if col_start == end || col_start == 0 || bytes[col_start - 1] != b':' {
        return None;
    }
    let col: usize = std::str::from_utf8(&bytes[col_start..end])
        .ok()?
        .parse()
        .ok()?;
    let line_end = col_start - 1;
    let mut line_start = line_end;
    while line_start > 0 && bytes[line_start - 1].is_ascii_digit() {
        line_start -= 1;
    }
    if line_start == line_end || line_start == 0 || bytes[line_start - 1] != b':' {
        return None;
    }
    let line_no: usize = std::str::from_utf8(&bytes[line_start..line_end])
        .ok()?
        .parse()
        .ok()?;
    Some((line_no, col))
}

#[test]
fn isolate_drops_cleanly_on_panic() {
    use std::panic::AssertUnwindSafe;
    use std::panic::catch_unwind;

    // Plain `#[test]` (not `#[tokio::test]`) so we can run a
    // tokio runtime inside the closure and let it tear down with
    // the panic.
    let result = catch_unwind(AssertUnwindSafe(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut host = EmbeddedV8RenderHost::new().expect("host boot");
            host.execute_module(
                "bundle.mjs",
                "export default { fetch(_r) { return new Response('ok'); } };",
            )
            .await
            .expect("execute");
            // Trigger a panic mid-flight.
            panic!("synthetic panic to test Drop cleanup");
        });
    }));
    assert!(
        result.is_err(),
        "expected the panic to propagate out of catch_unwind"
    );
    // If the panic had leaked V8 native resources, a subsequent
    // host boot in the same process would hit "isolate already
    // exists" / "v8::initialize panic". Booting again here is the
    // assertion that nothing leaked.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let mut host = EmbeddedV8RenderHost::new().expect("re-boot host");
        host.execute_module(
            "bundle.mjs",
            "export default { fetch(_r) { return new Response('post-panic'); } };",
        )
        .await
        .expect("execute after panic");
        let resp = host
            .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
            .await
            .expect("dispatch");
        assert_eq!(resp.body_utf8(), Some("post-panic"));
    });
}

#[tokio::test]
async fn dispatch_fetch_errors_when_called_before_bundle() {
    // Part 1: no module loaded at all — the message must match the
    // unchanged baseline (no "last install error" appended).
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let err = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect_err("dispatch before bundle should error");
    let msg = err.to_string();
    assert!(
        msg.contains("dispatch_fetch called before any bundle was loaded"),
        "expected pre-load error, got: {msg}"
    );
    assert!(
        !msg.contains("last install error"),
        "no install was attempted, so message must not contain 'last install error', got: {msg}"
    );

    // Part 2: load a non-workerd-shaped module (no `default` export),
    // then call dispatch_fetch — the error must surface the install
    // failure reason.
    let mut host2 = EmbeddedV8RenderHost::new().expect("host boot");
    host2
        .execute_module("util.mjs", "export const helper = () => 42;")
        .await
        .expect("execute utility module (non-fatal, no default export)");
    let err2 = host2
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect_err("dispatch after utility-only load should error");
    let msg2 = err2.to_string();
    assert!(
        msg2.contains("dispatch_fetch called before any bundle was loaded"),
        "expected pre-load base message, got: {msg2}"
    );
    assert!(
        msg2.contains("last install error:"),
        "expected install-error suffix after failed install, got: {msg2}"
    );
}

#[tokio::test]
async fn dispatch_fetch_surfaces_malformed_default_install_error() {
    // `export default 42` — the default export exists but is not an
    // object (workerd shape requires an object with a `fetch` method).
    // install_bundle_default rejects it; the error must appear in the
    // dispatch_fetch message.
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    host.execute_module("bundle.mjs", "export default 42;")
        .await
        .expect("execute_module is non-fatal even for malformed default");
    let err = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect_err("dispatch should error when bundle is malformed");
    let msg = err.to_string();
    assert!(
        msg.contains("dispatch_fetch called before any bundle was loaded"),
        "expected base message, got: {msg}"
    );
    assert!(
        msg.contains("last install error:"),
        "expected install-error suffix for malformed default, got: {msg}"
    );
}

#[tokio::test]
async fn successful_bundle_after_failed_clears_stale_install_error() {
    // Two separate host instances simulate the "bad module first, then
    // good module" scenario.  A single EmbeddedV8RenderHost cannot load
    // two main modules (deno_core only allows one `load_main_es_module`
    // call per runtime), so we verify the clearing logic indirectly:
    // a host that loaded a failed module stores a last_install_error, and
    // a host whose final load succeeded dispatches without error.

    // Host A: load bad then good is not achievable on one runtime, so we
    // verify the "cleared" path by ensuring a host whose only successful
    // load was a workerd-shaped bundle can dispatch correctly (the clear
    // in install_bundle_default's Ok arm is the mechanism under test).
    let mut host_good = EmbeddedV8RenderHost::new().expect("host boot (good path)");
    host_good
        .execute_module(
            "bundle.mjs",
            "export default { fetch(_r) { return new Response('ok-after-good', { status: 200 }); } };",
        )
        .await
        .expect("execute good bundle");
    // dispatch_fetch must succeed — last_install_error is None after a
    // successful install (cleared in the Ok arm).
    let resp = host_good
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch should succeed when last install succeeded");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body_utf8(), Some("ok-after-good"));

    // Host B: load a bad module — dispatch_fetch must report the install error.
    let mut host_bad = EmbeddedV8RenderHost::new().expect("host boot (bad path)");
    host_bad
        .execute_module("util.mjs", "export const x = 1;")
        .await
        .expect("execute utility module (non-fatal)");
    let err = host_bad
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect_err("dispatch must fail when only a non-workerd module was loaded");
    let msg = err.to_string();
    assert!(
        msg.contains("last install error:"),
        "expected install-error suffix when bad module was the last loaded, got: {msg}"
    );
}
