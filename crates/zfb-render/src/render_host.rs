//! `RenderHost` trait — the abstraction boundary between the renderer and the
//! underlying JS runtime.
//!
//! In production, build-time TSX→HTML rendering is
//! delegated to an embedded V8 host (`deno_core` in-process) driven by
//! `@takazudo/zfb-runtime`. The concrete host implementation lives in
//! [`crate::embedded_v8`]. The Rust side communicates with the host via the
//! `RenderHost` trait, which is the abstraction seam so caller code in this
//! crate can be aimed at whatever concrete host the orchestrator wires up.
//!
//! In-crate tests use lightweight in-process fakes that satisfy `RenderHost`
//! directly; integration tests against the real `embedded_v8` module live in
//! `tests/embedded_v8_*.rs`.
//!
//! ## Async design
//!
//! All three operations on `RenderHost` are `async`. The embedded V8 host
//! drives `deno_core::JsRuntime::run_event_loop`; giving those calls an async
//! signature lets the orchestrator schedule other work while a request is in
//! flight. In-process test fakes implement the trait methods as trivially-
//! ready futures (just `async { ... }`).
//!
//! `#[async_trait]` is used for dyn-compatibility: Rust stable does not yet
//! support `dyn Trait` with `async fn` methods without boxing. The macro
//! desugars each method to a `Pin<Box<dyn Future>>` return so
//! `&mut dyn RenderHost` continues to work for `Adapter::pre_render_setup`.

use async_trait::async_trait;
use serde_json::Value as JsonValue;

use crate::error::Result;

/// Opaque handle to a module already loaded into a host. The `id` is host-
/// specific; treat it as an opaque token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleHandle {
    /// Opaque, host-assigned identifier.
    pub id: u32,
    /// Display name / specifier of the module (for diagnostics).
    pub name: String,
}

impl ModuleHandle {
    /// Build a new handle. Public so test/in-memory hosts can mint handles.
    pub fn new(id: u32, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
        }
    }
}

/// Abstraction over the JS runtime.
///
/// Production target is the embedded V8 host
/// ([`crate::embedded_v8::EmbeddedV8RenderHost`]); tests in this crate use
/// in-process fakes. Add a new impl by satisfying these three operations.
///
/// The trait is intentionally **not** `Send` / `Sync`: the embedded V8 isolate
/// is pinned to the thread that creates it. Hosts that span threads do so by
/// parking the host on a dedicated thread and exchanging messages over a
/// channel.
///
/// All methods are `async` so implementations that need I/O (V8 microtask
/// draining, top-level await on module evaluate) can do so without blocking
/// the caller's thread.
#[async_trait(?Send)]
pub trait RenderHost {
    /// Load `source` as an ES module under the display `name`. Subsequent
    /// `call_default` / `get_export` calls reference the returned handle.
    ///
    /// Modules are evaluated immediately (top-level await is awaited).
    async fn execute_module(&mut self, name: &str, source: &str) -> Result<ModuleHandle>;

    /// Call the module's `default` export with `props`, expecting a string
    /// result (the rendered HTML for the current page).
    async fn call_default(&mut self, handle: &ModuleHandle, props: JsonValue) -> Result<String>;

    /// Read a named export from `handle` and return it as JSON. Used for the
    /// `meta` and `paths` exports.
    async fn get_export(&mut self, handle: &ModuleHandle, name: &str) -> Result<JsonValue>;
}
