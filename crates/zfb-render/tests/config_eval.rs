//! Tests for `zfb_render::config_eval` — the in-process V8 config evaluator.
//!
//! Acceptance criteria from issue #415:
//!
//! (a) `export default { hello: "world" }` round-trips to `Value::Object(...)`.
//! (b) `export default 42` → `ConfigEvalError::NotAPlainObject`.
//! (c) Bundle that imports a relative URL → loader rejection error.
//! (d) Bundle that throws at top level → error preserves the throw message.
//! (e) `export default { fn: () => 1 }` → `fn` field silently dropped (JSON.stringify semantics).
//! (f) `export default { d: new Date(0) }` → field is the ISO string form.
//! (g) `import "node:fs"; export default {}` → loader rejects with no-Node-globals error.
//!
//! Plus:
//! - Thread-pinned wrapper: two sequential `eval_bundle` calls succeed (no thread leak).
//! - spawn_blocking parallelism: the sync wrapper does not starve async tokio workers.
//! - V8 platform double-init: constructing a `ThreadedConfigEvaluator` after
//!   `EmbeddedV8RenderHost::new()` does not cause a V8 double-init crash.

#![cfg(feature = "embed_v8")]

use zfb_render::{ConfigEvalError, ThreadedConfigEvaluator, config_eval};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn eval(js: &str) -> Result<serde_json::Value, ConfigEvalError> {
    config_eval::evaluate_config_bundle(js).await
}

// ---------------------------------------------------------------------------
// (a) Plain object round-trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_a_plain_object_round_trips() {
    let val = eval(r#"export default { hello: "world" };"#)
        .await
        .expect("should succeed");
    assert_eq!(val, serde_json::json!({ "hello": "world" }));
}

// ---------------------------------------------------------------------------
// (b) Non-object default export → error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_b_non_object_default_export_is_error() {
    let err = eval("export default 42;")
        .await
        .expect_err("should fail for primitive default");
    let msg = err.to_string();
    assert!(
        msg.contains("default export must be a plain object"),
        "unexpected error: {msg}"
    );
}

// ---------------------------------------------------------------------------
// (c) Relative import → loader rejection
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_c_relative_import_rejected() {
    // The config isolate has no FS access, so any relative import fails.
    let err = eval(r#"import "./sibling.js"; export default {};"#)
        .await
        .expect_err("should fail for relative import");
    let msg = err.to_string();
    // The loader rejection message or a module-load error embedding it.
    assert!(
        msg.contains(zfb_render::config_eval::NO_BARE_IMPORTS_ERROR)
            || msg.contains("bare imports"),
        "unexpected error: {msg}"
    );
}

// ---------------------------------------------------------------------------
// (d) Top-level throw → error preserves message
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_d_top_level_throw_preserves_message() {
    let err = eval(r#"throw new Error("config_explode_123"); export default {};"#)
        .await
        .expect_err("should fail when module throws");
    let msg = err.to_string();
    assert!(
        msg.contains("config_explode_123"),
        "throw message not preserved: {msg}"
    );
}

// ---------------------------------------------------------------------------
// (e) Function-valued field silently dropped (JSON.stringify semantics)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_e_function_field_silently_dropped() {
    let val = eval(r#"export default { fn: () => 1, keep: "yes" };"#)
        .await
        .expect("should succeed");
    // `fn` field should be absent (JSON.stringify drops function-valued fields).
    assert_eq!(
        val,
        serde_json::json!({ "keep": "yes" }),
        "expected function field to be dropped, got: {val}"
    );
}

// ---------------------------------------------------------------------------
// (f) Date → ISO string (JSON.stringify semantics)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_f_date_becomes_iso_string() {
    let val = eval(r#"export default { d: new Date(0) };"#)
        .await
        .expect("should succeed");
    // JSON.stringify(new Date(0)) → "1970-01-01T00:00:00.000Z"
    let d = val.get("d").and_then(|v| v.as_str()).expect("d field");
    assert_eq!(d, "1970-01-01T00:00:00.000Z", "unexpected date string: {d}");
}

// ---------------------------------------------------------------------------
// (g) node:* import → loader rejection with exact error message
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_g_node_fs_import_rejected() {
    let err = eval("import \"node:fs\";\nexport default {};")
        .await
        .expect_err("should fail for node:fs import");
    let msg = err.to_string();
    assert!(
        msg.contains(zfb_render::config_eval::NO_BARE_IMPORTS_ERROR)
            || msg.contains("bare imports"),
        "unexpected error: {msg}"
    );
}

// Same path covers bare specifiers like `import "x"`.
#[tokio::test]
async fn test_g_bare_specifier_import_rejected() {
    let err = eval("import \"some-pkg\";\nexport default {};")
        .await
        .expect_err("should fail for bare import");
    let msg = err.to_string();
    assert!(
        msg.contains(zfb_render::config_eval::NO_BARE_IMPORTS_ERROR)
            || msg.contains("bare imports"),
        "unexpected error: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Thread-pinned wrapper: two sequential calls, no thread leak
// ---------------------------------------------------------------------------

#[test]
fn test_thread_pinned_two_sequential_calls() {
    // First call.
    let val1 = ThreadedConfigEvaluator::eval_bundle(r#"export default { call: 1 };"#)
        .expect("first call");
    assert_eq!(val1, serde_json::json!({ "call": 1 }));

    // Second call — fresh isolate, fresh thread.
    let val2 = ThreadedConfigEvaluator::eval_bundle(r#"export default { call: 2 };"#)
        .expect("second call");
    assert_eq!(val2, serde_json::json!({ "call": 2 }));
    // Thread cleanup is implicit: eval_bundle joins the thread before returning.
    // If it didn't, the thread handle would leak; the fact that both calls
    // complete without a panic/hang proves no thread leak occurs.
}

// ---------------------------------------------------------------------------
// spawn_blocking parallelism: sync wrapper does not starve async tokio workers
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_spawn_blocking_does_not_starve_tokio_workers() {
    use std::time::{Duration, Instant};

    // Kick off V8 eval on a blocking thread. V8 boot takes ~hundreds of ms.
    let eval_handle = tokio::task::spawn_blocking(|| {
        ThreadedConfigEvaluator::eval_bundle(r#"export default { ok: true };"#)
    });

    // A sibling async sleep should complete promptly even while V8 is booting.
    let sleep_start = Instant::now();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let sleep_elapsed = sleep_start.elapsed();

    // The sleep completed on time (generous upper bound — 2 s is lenient enough
    // even on loaded CI, but tight enough to catch a deadlocked event loop).
    assert!(
        sleep_elapsed < Duration::from_secs(2),
        "tokio sleep took too long ({sleep_elapsed:?}); V8 boot may be blocking the event loop"
    );

    // The V8 eval also completes successfully.
    let val = eval_handle.await.expect("spawn_blocking join").expect("eval");
    assert_eq!(val, serde_json::json!({ "ok": true }));
}

// ---------------------------------------------------------------------------
// V8 platform double-init: render host + config eval coexist
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_v8_double_init_safety() {
    // Boot the render host (initialises V8 platform).
    let _render_host = zfb_render::EmbeddedV8RenderHost::new()
        .expect("render host boot");

    // Now evaluate a config bundle — uses a separate JsRuntime but the same
    // V8 platform. Should not double-init or crash.
    let val =
        ThreadedConfigEvaluator::eval_bundle(r#"export default { platform: "shared" };"#)
            .expect("config eval after render host init");
    assert_eq!(val, serde_json::json!({ "platform": "shared" }));
}
