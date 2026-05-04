//! `RenderHost` trait — the abstraction boundary between the renderer and the
//! underlying JS runtime.
//!
//! ## ADR history
//!
//! - ADR-001 originally embedded a JS runtime in-process via `deno_core`.
//! - ADR-005 retired that path in favour of a short-lived miniflare
//!   (workerd) subprocess driven by `@takazudo/zfb-runtime`.
//! - ADR-007 (sub-issue #161) re-introduces an *in-process* V8 host —
//!   [`crate::embedded_v8::EmbeddedV8RenderHost`] — to remove the Node.js
//!   subprocess from the SSG critical path. The embedded host loads the
//!   same workerd-shape bundle (`export default { fetch }`) that the
//!   miniflare client used to drive, so the bundler does not change.
//!
//! The trait shape is unchanged. Production callers wire whichever
//! concrete host fits the deployment: subprocess client for builds that
//! still want miniflare-level isolation, embedded host for builds that
//! want a lower-overhead single-process pipeline.
//!
//! In-crate tests use lightweight in-process fakes that satisfy
//! `RenderHost` directly; integration tests against the real
//! [`crate::embedded_v8`] module live in `tests/embedded_v8_*.rs`.
//!
//! ## Async design
//!
//! All three operations on `RenderHost` are `async`. The miniflare
//! subprocess client drives I/O over stdin/stdout; the embedded V8 host
//! drives `deno_core::JsRuntime::run_event_loop`. Either way the async
//! signature lets the orchestrator schedule other work while a request
//! is in flight. In-process test fakes implement the trait methods
//! as trivially-ready futures (just `async { ... }`).
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
/// Two production hosts satisfy this trait: a miniflare subprocess client
/// (ADR-005) and the in-process embedded V8 host
/// ([`crate::embedded_v8::EmbeddedV8RenderHost`], ADR-007). Tests in this
/// crate also use in-process fakes. Add a new impl by satisfying these
/// three operations.
///
/// The trait is intentionally **not** `Send` / `Sync`: subprocess clients
/// own a stdio handle that must be driven from a single thread, and the
/// embedded V8 isolate carries the same invariant (V8 isolates are
/// pinned to the thread that creates them). Hosts that span threads do
/// so by parking the host on a dedicated thread and exchanging messages
/// over a channel.
///
/// All methods are `async` so implementations that need I/O (subprocess
/// stdin/stdout, V8 microtask draining, top-level await on module
/// evaluate) can do so without blocking the caller's thread.
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
