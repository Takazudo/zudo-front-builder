//! `RenderHost` trait — the abstraction boundary between the renderer and the
//! underlying JS runtime.
//!
//! ADR-007 describes the production path: build-time TSX→HTML rendering is
//! delegated to an embedded V8 host driven by `@takazudo/zfb-runtime`. The
//! Rust side communicates with the host via the `RenderHost` trait, which is
//! the abstraction seam so caller code in this crate can be aimed at whatever
//! concrete host the orchestrator wires up.
//!
//! No in-process JS host implementation lives here at the moment. The
//! production host lands in T6 (build-time render orchestration). Tests in
//! this crate use lightweight in-process fakes that satisfy `RenderHost`.
//!
//! ## Async design
//!
//! All three operations on `RenderHost` are `async`. The embedded V8 host
//! client drives I/O; giving those calls an async signature lets the future
//! orchestrator schedule other work while a request is in flight. In-process
//! test fakes implement the trait methods as trivially-ready futures (just
//! `async { ... }`).
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
/// Production target (per ADR-007) is the embedded V8 host; tests in this
/// crate use in-process fakes. Add a new impl by satisfying these three
/// operations.
///
/// The trait is intentionally **not** `Send` / `Sync`: embedded JS host
/// implementations typically own resources that must be driven from a single
/// thread. Hosts that span threads do so by parking the host on a dedicated
/// thread and exchanging messages over a channel.
///
/// All methods are `async` so implementations that need I/O can do so
/// without blocking the caller's thread.
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
