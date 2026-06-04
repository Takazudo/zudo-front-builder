//! Console-capture tests for `EmbeddedV8RenderHost` (issue #700).
//!
//! What's exercised here:
//!
//! - **Logs then throws**: a workerd-shaped bundle whose `fetch`
//!   console.logs at several levels and then throws. The dispatch
//!   fails, and `drain_console_logs` must return the buffered lines
//!   with their `[level]` prefixes — this is the exact render-failure
//!   path `zfb-build::renderer::render_all` surfaces to the operator.
//!
//! - **Module-evaluation logs**: console output produced while the
//!   bundle's top-level code runs (including when evaluation itself
//!   throws) must be drainable too — the capture shim is installed at
//!   host boot, before bundle execution.
//!
//! - **Drain semantics**: draining clears the buffer (second drain is
//!   empty) and resets the truncation state.
//!
//! - **Buffer cap**: a worker that logs thousands of lines must not
//!   grow the buffer unboundedly; the drained output carries a single
//!   truncation marker instead.
//!
//! All tests are gated on the `embed_v8` feature.

#![cfg(feature = "embed_v8")]

use zfb_render::{EmbeddedV8RenderHost, HttpRequestLike, RenderHost};

#[tokio::test]
async fn drains_levelled_console_output_after_failed_dispatch() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    // Nothing logged yet — a fresh host drains to empty.
    assert_eq!(host.drain_console_logs(), "");

    let bundle = r#"
        export default {
          fetch(request) {
            console.log("plain line", 42);
            console.info("info line");
            console.warn("warn line");
            console.error("error line");
            console.debug({ detail: "structured" });
            throw new Error("render exploded");
          },
        };
    "#;
    host.execute_module("bundle.mjs", bundle)
        .await
        .expect("execute bundle");
    let err = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/boom"))
        .await
        .expect_err("dispatch must fail when fetch throws");
    assert!(
        err.to_string().contains("render exploded"),
        "dispatch error must carry the thrown message: {err}"
    );

    let logs = host.drain_console_logs();
    assert!(
        logs.contains("[log] plain line 42"),
        "missing log line: {logs:?}"
    );
    assert!(
        logs.contains("[info] info line"),
        "missing info line: {logs:?}"
    );
    assert!(
        logs.contains("[warn] warn line"),
        "missing warn line: {logs:?}"
    );
    assert!(
        logs.contains("[error] error line"),
        "missing error line: {logs:?}"
    );
    assert!(
        logs.contains(r#"[debug] {"detail":"structured"}"#),
        "objects must JSON-stringify in the capture: {logs:?}"
    );

    // Draining clears the buffer.
    assert_eq!(host.drain_console_logs(), "");
}

#[tokio::test]
async fn captures_module_evaluation_logs_even_when_evaluation_fails() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = r#"
        console.log("during module evaluation");
        throw new Error("boot boom");
    "#;
    host.execute_module("bundle.mjs", bundle)
        .await
        .expect_err("module evaluation must fail");
    let logs = host.drain_console_logs();
    assert!(
        logs.contains("[log] during module evaluation"),
        "logs printed before a module-evaluation failure must be drainable: {logs:?}"
    );
}

#[tokio::test]
async fn caps_buffer_and_marks_truncation() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    // 1500 lines blows past the 1000-entry cap (kept close to the
    // cap so the live stdout passthrough does not spam test output). The noisy loop lives
    // in `fetch` (not module top-level) so the same bundle can log
    // again after the drain — a host loads only one main module.
    let bundle = r#"
        export default {
          fetch(request) {
            const url = new URL(request.url);
            if (url.pathname === "/noisy") {
              for (let i = 0; i < 1500; i++) {
                console.log("line " + i);
              }
            } else {
              console.log("fresh after drain");
            }
            return new Response("ok", { status: 200 });
          },
        };
    "#;
    host.execute_module("bundle.mjs", bundle)
        .await
        .expect("execute bundle");
    host.dispatch_fetch(HttpRequestLike::get("http://zfb.local/noisy"))
        .await
        .expect("dispatch /noisy");
    let logs = host.drain_console_logs();
    let line_count = logs.lines().count();
    assert!(
        line_count <= 1001,
        "buffer must be capped (got {line_count} lines)"
    );
    assert!(
        logs.contains("[log] line 0"),
        "early lines kept: {line_count} lines"
    );
    assert!(
        !logs.contains("line 1499"),
        "lines past the cap must be dropped"
    );
    assert!(
        logs.contains("console capture truncated"),
        "a truncation marker must record that lines were dropped"
    );
    // The marker appears exactly once, not per dropped line.
    assert_eq!(logs.matches("console capture truncated").count(), 1);

    // Draining resets the cap state: new logs are captured again.
    host.dispatch_fetch(HttpRequestLike::get("http://zfb.local/quiet"))
        .await
        .expect("dispatch /quiet");
    let fresh = host.drain_console_logs();
    assert!(
        fresh.contains("[log] fresh after drain"),
        "capture must resume after a drain resets the cap: {fresh:?}"
    );
    assert!(!fresh.contains("truncated"));
}
