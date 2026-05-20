//! `node:*` stub tests for `EmbeddedV8RenderHost` (sub-issue #162).
//!
//! User bundles produced by arbitrary developer code may
//! include top-level `import "node:fs"` (or `node:path`, etc.) for
//! branches that only fire under Workers / production SSR. The host
//! must:
//!
//! 1. *Resolve* every `node:*` specifier in the v1 list at module-
//!    load time so the bundle evaluates at all.
//! 2. *Throw* with a recognisable message when any member is
//!    actually called.
//!
//! These tests assert both halves for the v1 list:
//! `node:fs`, `node:fs/promises`, `node:path`, `node:url`, `node:buffer`.

#![cfg(feature = "embed_v8")]

use zfb_render::{EmbeddedV8RenderHost, HttpRequestLike, RenderHost};

const STUB_ERROR_TOKEN: &str = "node:* is not available under the SSG runtime";

/// Per-specifier test driver. Loads a bundle that imports the
/// specifier as both a default and a named import, then dispatches
/// a request that does NOT touch the import — should succeed.
async fn assert_import_resolves(specifier: &str, named_import: &str) {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = format!(
        r#"
        import nodeStub, {{ {named} }} from "{specifier}";
        // Also probe namespace-style import indirectly via dynamic.
        // Storing `nodeStub` keeps it from being tree-shaken.
        globalThis.__zfbProbeStub = nodeStub;
        globalThis.__zfbProbeNamed = {named};
        export default {{
          fetch(_request) {{
            return new Response("loaded", {{ status: 200 }});
          }},
        }};
        "#,
        specifier = specifier,
        named = named_import,
    );
    host.execute_module("bundle.mjs", &bundle)
        .await
        .unwrap_or_else(|e| {
            panic!("bundle importing {specifier} failed to load: {e}")
        });
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .unwrap_or_else(|e| {
            panic!("dispatch after importing {specifier} failed: {e}")
        });
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body_utf8(), Some("loaded"));
}

/// Per-specifier test driver. Loads a bundle whose `fetch` actually
/// invokes a member of the imported namespace and asserts the call
/// throws with the canonical error message.
async fn assert_call_throws(specifier: &str, member_call: &str) {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = format!(
        r#"
        import nodeStub from "{specifier}";
        export default {{
          fetch(_request) {{
            // member_call is a JS expression that touches the stub
            // — e.g. `nodeStub.readFileSync("/tmp")`.
            const result = {call};
            return new Response("unexpected: " + String(result));
          }},
        }};
        "#,
        specifier = specifier,
        call = member_call,
    );
    host.execute_module("bundle.mjs", &bundle)
        .await
        .unwrap_or_else(|e| {
            panic!("bundle importing {specifier} failed to load: {e}")
        });
    let err = match host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
    {
        Ok(_) => panic!(
            "expected dispatch to throw for {specifier}, but it returned a response"
        ),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(
        msg.contains(STUB_ERROR_TOKEN),
        "expected `{STUB_ERROR_TOKEN}` in error for {specifier}, got: {msg}"
    );
}

#[tokio::test]
async fn node_fs_resolves_and_throws_on_call() {
    assert_import_resolves("node:fs", "readFileSync").await;
    assert_call_throws("node:fs", "nodeStub.readFileSync('/tmp/x')").await;
}

#[tokio::test]
async fn node_fs_promises_resolves_and_throws_on_call() {
    assert_import_resolves("node:fs/promises", "readFile").await;
    assert_call_throws("node:fs/promises", "nodeStub.readFile('/tmp/x')").await;
}

#[tokio::test]
async fn node_path_resolves_and_throws_on_call() {
    assert_import_resolves("node:path", "join").await;
    assert_call_throws("node:path", "nodeStub.join('/a', '/b')").await;
}

#[tokio::test]
async fn node_url_resolves_and_throws_on_call() {
    assert_import_resolves("node:url", "fileURLToPath").await;
    // `node:url` is the deprecated Node namespace; the global
    // `URL` from the polyfill is unaffected (covered separately
    // in `embedded_v8_smoke.rs::handles_hono_shape_router`).
    assert_call_throws(
        "node:url",
        "nodeStub.fileURLToPath('file:///a')",
    )
    .await;
}

#[tokio::test]
async fn node_buffer_resolves_and_throws_on_call() {
    assert_import_resolves("node:buffer", "Buffer").await;
    assert_call_throws("node:buffer", "nodeStub.Buffer.from('hello')").await;
}

#[tokio::test]
async fn unknown_node_specifier_is_rejected_at_load_time() {
    // `node:net` is NOT in the v1 list. The loader must refuse it
    // (rather than silently absorbing) so a misspelling / unsupported
    // module surfaces with a clear error.
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = r#"
        import _net from "node:net";
        export default { fetch() { return new Response("never") } };
    "#;
    let err = host
        .execute_module("bundle.mjs", bundle)
        .await
        .expect_err("expected unknown node:* import to fail");
    let msg = err.to_string();
    // The exact wording comes from BundleModuleLoader's load
    // fallback path. We don't pin the wording here — just confirm
    // it surfaces the unsupported specifier somewhere.
    assert!(
        msg.contains("node:net"),
        "expected error to mention node:net, got: {msg}"
    );
}
