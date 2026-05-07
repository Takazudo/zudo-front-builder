//! Regression tests for the `Event` / `CustomEvent` / `EventTarget`
//! globals stubbed by [`extensions::BROWSER_EVENT_SRC`].
//!
//! Surfaced by zudolab/zudo-doc#1521 (W5A): the just-shipped
//! client-router surface in `@takazudo/zfb-runtime` declares three
//! classes at module top level that extend the browser `Event`
//! global. Without a stub, the embedded V8 host fails to load the
//! bundle with:
//!
//!   ReferenceError: Event is not defined
//!
//! The host now provides minimal stubs sufficient for module-load
//! evaluation. These tests pin the contract:
//!
//! - `class X extends Event {}` evaluates at module top level.
//! - `super(type, init)` from a subclass produces an Event-shaped
//!   instance (defaultPrevented, type, etc.).
//! - `new CustomEvent(type, { detail })` works.
//! - `new EventTarget()` works and methods are callable.
//!
//! We deliberately do NOT exercise event dispatch — the SSG path
//! never dispatches events, and pinning behaviour we don't intend
//! to support would make future expansion harder.

#![cfg(feature = "embed_v8")]

use zfb_render::{EmbeddedV8RenderHost, HttpRequestLike, RenderHost};

#[tokio::test]
async fn module_top_level_extends_event_loads() {
    // Mirrors the failing case in `@takazudo/zfb-runtime`'s
    // `client-router/events.ts`: a class hierarchy extending Event
    // declared at module top level. If `Event` is missing from
    // globalThis, this module aborts at load time before `fetch` is
    // ever called.
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = r#"
        class BeforeEvent extends Event {
          constructor(type, init, payload) {
            super(type, init);
            this.payload = payload;
          }
        }
        class TransitionBeforePreparationEvent extends BeforeEvent {
          constructor(payload) {
            super("zfb:before-preparation", { cancelable: true }, payload);
          }
        }
        export default {
          fetch(_request) {
            const evt = new TransitionBeforePreparationEvent({ ok: true });
            const body = JSON.stringify({
              type: evt.type,
              cancelable: evt.cancelable,
              defaultPrevented: evt.defaultPrevented,
              payload: evt.payload,
            });
            return new Response(body, {
              status: 200,
              headers: { "content-type": "application/json" },
            });
          },
        };
    "#;
    host.execute_module("bundle.mjs", bundle)
        .await
        .expect("bundle with `class X extends Event` should load");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    assert_eq!(resp.status, 200);
    let body = resp.body_utf8().expect("utf-8 body");
    assert!(body.contains(r#""type":"zfb:before-preparation""#), "body={body}");
    assert!(body.contains(r#""cancelable":true"#), "body={body}");
    assert!(body.contains(r#""defaultPrevented":false"#), "body={body}");
    assert!(body.contains(r#""payload":{"ok":true}"#), "body={body}");
}

#[tokio::test]
async fn custom_event_carries_detail() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = r#"
        export default {
          fetch(_request) {
            const evt = new CustomEvent("zfb:custom", { detail: { id: 42 } });
            return new Response(
              JSON.stringify({ type: evt.type, detail: evt.detail }),
              { status: 200, headers: { "content-type": "application/json" } }
            );
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
    let body = resp.body_utf8().expect("utf-8 body");
    assert!(body.contains(r#""detail":{"id":42}"#), "body={body}");
}

#[tokio::test]
async fn event_target_subclass_constructs() {
    // Some upstream user code subclasses EventTarget at module top
    // level. The stubs only need to make `extends EventTarget` and
    // `new Subclass()` succeed; we don't need real listener
    // semantics on the SSG path.
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = r#"
        class Bus extends EventTarget {}
        const bus = new Bus();
        export default {
          fetch(_request) {
            bus.addEventListener("ping", () => {});
            const ok = bus.dispatchEvent(new Event("ping"));
            return new Response(String(ok), { status: 200 });
          },
        };
    "#;
    host.execute_module("bundle.mjs", bundle)
        .await
        .expect("bundle with `class X extends EventTarget` should load");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body_utf8(), Some("true"));
}

#[tokio::test]
async fn prevent_default_records_on_cancelable_event() {
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = r#"
        export default {
          fetch(_request) {
            const cancelable = new Event("a", { cancelable: true });
            cancelable.preventDefault();
            const passive = new Event("b");
            passive.preventDefault();
            return new Response(
              JSON.stringify({
                cancelable: cancelable.defaultPrevented,
                passive: passive.defaultPrevented,
              }),
              { status: 200, headers: { "content-type": "application/json" } }
            );
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
    let body = resp.body_utf8().expect("utf-8 body");
    // preventDefault on a cancelable event should record; on a
    // non-cancelable event it should be a no-op.
    assert!(body.contains(r#""cancelable":true"#), "body={body}");
    assert!(body.contains(r#""passive":false"#), "body={body}");
}
