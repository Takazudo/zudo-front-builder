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

/// Sub-issue #1761: `Response`'s BodyInit content-type defaulting
/// (Fetch "extract a body" step 5). A `string` body with no explicit
/// header defaults to `text/plain;charset=UTF-8` and the body bytes are
/// untouched (no doctype prepend — that's a server-side concern gated
/// on the header this test proves gets set).
#[tokio::test]
async fn response_string_body_defaults_to_text_plain() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = workerd_shaped_bundle(r#"return new Response("plain body");"#);
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute bundle");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body_utf8(), Some("plain body"));
    assert_eq!(resp.content_type(), Some("text/plain;charset=UTF-8"));
}

/// A `URLSearchParams` body defaults to the form-urlencoded type.
#[tokio::test]
async fn response_urlsearchparams_body_defaults_to_form_urlencoded() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle =
        workerd_shaped_bundle(r#"return new Response(new URLSearchParams({ a: "1", b: "2" }));"#);
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute bundle");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body_utf8(), Some("a=1&b=2"));
    assert_eq!(
        resp.content_type(),
        Some("application/x-www-form-urlencoded;charset=UTF-8")
    );
}

/// Sub-issue #1762: `URLSearchParams` must apply the
/// `application/x-www-form-urlencoded` `+`-means-space convention on
/// both parse and serialize, and a literal `+` (`%2B`) must round-trip
/// as a real plus character. Each assertion's expected value is what
/// the native (spec) `URLSearchParams` produces for the same input —
/// this is a V8-eval parity check against the embedded polyfill, not
/// a hand-picked string.
#[tokio::test]
async fn urlsearchparams_plus_space_form_encoding_matrix() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = workerd_shaped_bundle(
        r#"
        const failures = [];
        function check(label, actual, expected) {
          if (actual !== expected) {
            failures.push(`${label}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
          }
        }

        // `+` decodes to space on parse.
        check(
          "plus-decodes-to-space",
          new URLSearchParams("q=hello+world").get("q"),
          "hello world",
        );

        // Space serializes to `+`.
        const spaceParams = new URLSearchParams();
        spaceParams.set("a", "x y");
        check("space-serializes-to-plus", spaceParams.toString(), "a=x+y");

        // Literal plus round-trips: `%2B` parses to a literal `+` and
        // re-serializes as `%2B` (not a raw `+`, which would decode
        // back to space).
        const literalPlus = new URLSearchParams("k=a%2Bb");
        check("percent-2b-parses-to-literal-plus", literalPlus.get("k"), "a+b");
        check("literal-plus-reserializes-as-percent-2b", literalPlus.toString(), "k=a%2Bb");

        // Empty keys and empty values parse and re-serialize per spec.
        const empties = new URLSearchParams("=v&k=&k2&&");
        check("empty-key-with-value", empties.get(""), "v");
        check("key-with-empty-value", empties.get("k"), "");
        check("bare-key-no-eq", empties.get("k2"), "");
        check(
          "empty-segments-and-bare-key-reserialize",
          empties.toString(),
          "=v&k=&k2=",
        );

        // Repeated keys: getAll preserves parse order; toString keeps
        // deterministic first-insertion order.
        const repeated = new URLSearchParams("r=1&r=2&r=3");
        check(
          "repeated-key-getall-order",
          JSON.stringify(repeated.getAll("r")),
          JSON.stringify(["1", "2", "3"]),
        );
        check("repeated-key-tostring-order", repeated.toString(), "r=1&r=2&r=3");

        // Iterator behavior (entries/for..of) is unchanged for the
        // plus/space + repeated-key cases above.
        const iterFixture = new URLSearchParams("a=x+y&b=1&b=2");
        const viaEntries = [...iterFixture.entries()].map(([k, v]) => `${k}=${v}`).join("&");
        check("entries-iterator-order", viaEntries, "a=x y&b=1&b=2");
        const viaForOf = [];
        for (const [k, v] of iterFixture) {
          viaForOf.push(`${k}=${v}`);
        }
        check("for-of-iterator-order", viaForOf.join("&"), "a=x y&b=1&b=2");

        if (failures.length > 0) {
          return new Response(failures.join("\n"), { status: 500 });
        }
        return new Response("OK", { status: 200 });
        "#,
    );
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute bundle");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    assert_eq!(resp.body_utf8(), Some("OK"), "status={}", resp.status);
    assert_eq!(resp.status, 200);
}

/// An `ArrayBuffer` body gets NO automatic content-type per the Fetch
/// BodyInit table — typed arrays / `ArrayBuffer` are the one shape that
/// stays opaque by default.
#[tokio::test]
async fn response_arraybuffer_body_gets_no_automatic_content_type() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = workerd_shaped_bundle(
        r#"
        const bytes = new Uint8Array([1, 2, 3, 4]);
        return new Response(bytes.buffer);
        "#,
    );
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute bundle");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, vec![1u8, 2, 3, 4]);
    assert_eq!(resp.content_type(), None);
}

/// An explicit `content-type` header always wins over the BodyInit
/// default, regardless of body shape.
#[tokio::test]
async fn response_explicit_content_type_header_wins_over_bodyinit_default() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = workerd_shaped_bundle(
        r#"
        return new Response("<h1>hi</h1>", {
          headers: { "content-type": "text/html; charset=utf-8" },
        });
        "#,
    );
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute bundle");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.content_type(), Some("text/html; charset=utf-8"));
}

/// `clone()` preserves whatever content-type the original settled on —
/// whether explicit or BodyInit-defaulted.
#[tokio::test]
async fn response_clone_preserves_bodyinit_defaulted_content_type() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = workerd_shaped_bundle(
        r#"
        const original = new Response("plain body");
        const cloned = original.clone();
        return cloned;
        "#,
    );
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute bundle");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body_utf8(), Some("plain body"));
    assert_eq!(resp.content_type(), Some("text/plain;charset=UTF-8"));
}

/// Deep-review fix: the `Response` constructor must clone a caller-
/// supplied `Headers` instance rather than alias it — otherwise the
/// BodyInit content-type default's `set()` call would mutate a
/// `Headers` object the caller still holds a reference to. Pass the
/// same `Headers` instance to two `Response`s and confirm the first
/// construction's default doesn't leak into the second.
#[tokio::test]
async fn response_construction_does_not_mutate_caller_headers_object() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = workerd_shaped_bundle(
        r#"
        const shared = new Headers();
        const first = new Response("plain body", { headers: shared });
        // If `Response` aliased `shared`, it would now carry the
        // text/plain default the first construction installed.
        const stillEmpty = !shared.has("content-type");
        return new Response(String(stillEmpty), { status: stillEmpty ? 200 : 500 });
        "#,
    );
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute bundle");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    assert_eq!(
        resp.status, 200,
        "constructing a Response must not mutate the caller's shared Headers object"
    );
}

/// `Response.json()` must still install `application/json` even though
/// its body is a stringified JSON string — the static helper installs
/// the header BEFORE construction so it isn't beaten by the
/// constructor's new string-body default.
#[tokio::test]
async fn response_json_still_sets_application_json_content_type() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = workerd_shaped_bundle(r#"return Response.json({ ok: true });"#);
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute bundle");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body_utf8(), Some("{\"ok\":true}"));
    assert_eq!(resp.content_type(), Some("application/json"));
}

/// `Response.json()` still honors an explicit caller-supplied
/// content-type header instead of overwriting it with `application/json`.
#[tokio::test]
async fn response_json_honors_explicit_content_type_override() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = workerd_shaped_bundle(
        r#"
        return Response.json(
          { ok: true },
          { headers: { "content-type": "application/ld+json" } },
        );
        "#,
    );
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute bundle");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.content_type(), Some("application/ld+json"));
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
    let has_line_col_token = msg.lines().any(|l| extract_line_col(l).is_some());
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
    use std::panic::catch_unwind;
    use std::panic::AssertUnwindSafe;

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
async fn good_bundle_dispatches_and_bad_only_host_reports_install_error() {
    // Renamed from `successful_bundle_after_failed_clears_stale_install_error`
    // to match what the public API actually lets us assert.
    //
    // The Ok-arm clear (`*self.last_install_error.borrow_mut() = None` in
    // execute_module) is deliberately defensive and UNOBSERVABLE through the
    // public API: `last_install_error` is only ever read by dispatch_fetch's
    // pre-install error path, and once a workerd-shaped bundle installs,
    // `bundle_installed` flips true and the field is never read again. So no
    // black-box test — single-host or otherwise — can witness the clear. We
    // therefore assert the two observable behaviours that bracket it:
    //   - a host whose load succeeded dispatches correctly (Ok arm taken);
    //   - a host whose only load was non-workerd reports the install error
    //     (Err arm taken).
    //
    // (A single EmbeddedV8RenderHost also cannot load two main modules —
    // deno_core allows only one `load_main_es_module` per runtime — so the
    // bad-then-good sequence isn't expressible on one host anyway, but the
    // observability limit above is the real reason this stays two hosts.)

    // Host A: only successful load is a workerd-shaped bundle (Ok arm).
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

/// Item #1 perf: request bodies are now passed via base64 + atob instead of a
/// Uint8Array.from([b0,b1,...]) numeric-array literal.  This test dispatches a
/// POST request with a binary body and asserts the handler receives it intact.
///
/// Regression guard: if the base64 decode in the JS expression is broken, the
/// handler's `req.arrayBuffer()` call returns an empty or mangled buffer and
/// the echoed JSON will not match.
#[tokio::test]
async fn dispatch_post_with_binary_body_round_trips() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    // Bundle echoes the raw request body bytes back as a JSON array.
    let bundle = r#"
        export default {
          async fetch(request) {
            const buf = await request.arrayBuffer();
            const bytes = Array.from(new Uint8Array(buf));
            return new Response(JSON.stringify(bytes), {
              status: 200,
              headers: { "content-type": "application/json" },
            });
          },
        };
    "#;
    host.execute_module("bundle.mjs", bundle)
        .await
        .expect("execute bundle");

    // Send a body with all 256 byte values to catch any byte-value aliasing.
    let body: Vec<u8> = (0u8..=255u8).collect();
    let req = HttpRequestLike {
        url: "http://zfb.local/echo".to_string(),
        method: "POST".to_string(),
        headers: std::collections::BTreeMap::new(),
        body: Some(body.clone()),
    };
    let resp = host.dispatch_fetch(req).await.expect("dispatch POST");
    assert_eq!(resp.status, 200);
    let body_str = resp.body_utf8().expect("response must be UTF-8");
    let echoed: Vec<u8> =
        serde_json::from_str::<Vec<u8>>(body_str).expect("response must be valid JSON array");
    assert_eq!(
        echoed, body,
        "dispatched body must round-trip intact through base64 encode/atob decode"
    );
}

/// Bridge-seam proof for sub-issue #1760: a bundle that appends two distinct
/// `Set-Cookie` values — one of them carrying an `Expires` attribute whose
/// value itself contains a comma — must see both survive `dispatch_fetch` in
/// order. Before #1760 the JS→Rust boundary (`globals_shim.js`'s dispatch,
/// `DispatchResult.headers`, and `HttpResponseLike.headers`) collapsed
/// same-name headers into a single-valued map, and the old `Headers.append`
/// comma-joined `Set-Cookie` values on write — which would also have
/// corrupted the Expires-comma cookie even before the map collapse.
#[tokio::test]
async fn dispatch_fetch_preserves_ordered_duplicate_set_cookie_headers() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = workerd_shaped_bundle(
        r#"
        const headers = new Headers();
        headers.append("Set-Cookie", "a=1; Path=/");
        headers.append(
          "Set-Cookie",
          "b=2; Expires=Wed, 21 Oct 2026 07:28:00 GMT; Path=/",
        );
        return new Response("ok", { status: 200, headers });
        "#,
    );
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute bundle");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    let cookies: Vec<&str> = resp
        .headers
        .iter()
        .filter(|(k, _)| k == "set-cookie")
        .map(|(_, v)| v.as_str())
        .collect();
    assert_eq!(
        cookies,
        vec![
            "a=1; Path=/",
            "b=2; Expires=Wed, 21 Oct 2026 07:28:00 GMT; Path=/",
        ],
        "both Set-Cookie values (one containing a comma inside Expires) must \
         survive dispatch_fetch in order, got {cookies:?}"
    );
}

/// `Headers.set()` must replace every prior value for the name, not append
/// alongside them (sub-issue #1760).
#[tokio::test]
async fn headers_set_replaces_all_prior_values_for_name() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = workerd_shaped_bundle(
        r#"
        const headers = new Headers();
        headers.append("X-Trace", "one");
        headers.append("X-Trace", "two");
        headers.set("X-Trace", "final");
        return new Response("ok", { status: 200, headers });
        "#,
    );
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute bundle");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    let values: Vec<&str> = resp
        .headers
        .iter()
        .filter(|(k, _)| k == "x-trace")
        .map(|(_, v)| v.as_str())
        .collect();
    assert_eq!(
        values,
        vec!["final"],
        "set() must replace all prior values for the name, got {values:?}"
    );
}

/// Constructing a `Headers` from another `Headers` (the clone-init form)
/// must preserve duplicate `Set-Cookie` values rather than collapsing them
/// via the combined single-value view (sub-issue #1760).
#[tokio::test]
async fn headers_clone_from_headers_preserves_duplicate_set_cookie() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = workerd_shaped_bundle(
        r#"
        const original = new Headers();
        original.append("Set-Cookie", "a=1; Path=/");
        original.append("Set-Cookie", "b=2; Path=/");
        const cloned = new Headers(original);
        return new Response("ok", { status: 200, headers: cloned });
        "#,
    );
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute bundle");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    let cookies: Vec<&str> = resp
        .headers
        .iter()
        .filter(|(k, _)| k == "set-cookie")
        .map(|(_, v)| v.as_str())
        .collect();
    assert_eq!(
        cookies,
        vec!["a=1; Path=/", "b=2; Path=/"],
        "cloning a Headers must preserve duplicate set-cookie entries, got {cookies:?}"
    );
}

/// Ordinary (non-`Set-Cookie`) headers still combine into a single
/// comma-joined value per the Fetch "sort and combine" algorithm, even
/// though `append` no longer joins eagerly at write time.
#[tokio::test]
async fn ordinary_headers_still_combine_with_comma_separator() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = workerd_shaped_bundle(
        r#"
        const headers = new Headers();
        headers.append("Vary", "Accept-Encoding");
        headers.append("Vary", "Accept-Language");
        return new Response("ok", { status: 200, headers });
        "#,
    );
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute bundle");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    let vary: Vec<&str> = resp
        .headers
        .iter()
        .filter(|(k, _)| k == "vary")
        .map(|(_, v)| v.as_str())
        .collect();
    assert_eq!(
        vary,
        vec!["Accept-Encoding, Accept-Language"],
        "ordinary repeated headers must combine into one comma-joined entry, got {vary:?}"
    );
}

/// Live iteration (WHATWG "iterate a map" semantics): deleting a
/// not-yet-visited header during `forEach` must remove it from the
/// remaining traversal — the deleted key is NOT yielded. A snapshot-based
/// iterator would still visit the stale `b`.
#[tokio::test]
async fn headers_foreach_deleting_later_key_skips_it() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = workerd_shaped_bundle(
        r#"
        const headers = new Headers();
        headers.append("a", "1");
        headers.append("b", "2");
        const visited = [];
        headers.forEach((v, k) => {
          visited.push(k);
          if (k === "a") headers.delete("b");
        });
        return new Response(visited.join(","), { status: 200 });
        "#,
    );
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute bundle");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    assert_eq!(
        resp.body_utf8(),
        Some("a"),
        "deleting a not-yet-visited header mid-forEach must skip it (live iteration)"
    );
}

/// Live iteration: a header appended during `forEach` must become visible
/// to a later step and IS yielded. A snapshot-based iterator would miss it.
#[tokio::test]
async fn headers_foreach_appending_key_visits_it() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = workerd_shaped_bundle(
        r#"
        const headers = new Headers();
        headers.append("a", "1");
        const visited = [];
        headers.forEach((v, k) => {
          visited.push(k);
          if (k === "a") headers.append("c", "3");
        });
        return new Response(visited.join(","), { status: 200 });
        "#,
    );
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute bundle");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    assert_eq!(
        resp.body_utf8(),
        Some("a,c"),
        "appending a header mid-forEach must make it visible to a later step (live iteration)"
    );
}

/// Regression guard: non-mutating iteration keeps the exact sorted-combined
/// order — header names in sorted order, `set-cookie` values yielded
/// uncombined, every other name comma-joined into one entry. The live
/// iterator must not disturb this order.
#[tokio::test]
async fn headers_entries_nonmutating_order_is_sorted_and_combined() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = workerd_shaped_bundle(
        r#"
        const headers = new Headers();
        headers.append("b-two", "2");
        headers.append("a-one", "1");
        headers.append("Set-Cookie", "x=1");
        headers.append("Set-Cookie", "y=2");
        headers.append("vary", "Accept-Encoding");
        headers.append("vary", "Accept-Language");
        const parts = [];
        for (const [k, v] of headers.entries()) parts.push(k + "=" + v);
        return new Response(parts.join("|"), { status: 200 });
        "#,
    );
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute bundle");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    assert_eq!(
        resp.body_utf8(),
        Some("a-one=1|b-two=2|set-cookie=x=1|set-cookie=y=2|vary=Accept-Encoding, Accept-Language"),
        "non-mutating iteration must keep sorted+combined order (set-cookie uncombined)"
    );
}
