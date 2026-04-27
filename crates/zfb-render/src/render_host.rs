//! `RenderHost` trait — the abstraction boundary between the renderer and the
//! underlying JS runtime.
//!
//! ADR-005 supersedes ADR-001: zfb no longer embeds a JS runtime in the Rust
//! binary. Build-time TSX→HTML rendering is delegated to a short-lived
//! miniflare (workerd) subprocess driven by `@takazudo/zfb-runtime`. The Rust
//! side talks to that subprocess over an IPC boundary; the `RenderHost` trait
//! is preserved as the abstraction seam so caller code in this crate can be
//! aimed at whatever concrete host the orchestrator wires up.
//!
//! No in-process JS host implementation lives here at the moment. The
//! production host (a miniflare subprocess client) lands in T6 (build-time
//! render orchestration). Tests in this crate use lightweight in-process fakes
//! that satisfy `RenderHost`.

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
/// Production target (per ADR-005) is a thin client over a miniflare
/// subprocess; tests in this crate use in-process fakes. Add a new impl by
/// satisfying these three operations.
///
/// The trait is intentionally **not** `Send` / `Sync`: subprocess clients
/// typically own a stdio handle that must be driven from a single thread, and
/// any future in-process JS isolate would carry the same invariant. Hosts
/// that span threads do so by parking the host on a dedicated thread and
/// exchanging messages over a channel.
pub trait RenderHost {
    /// Load `source` as an ES module under the display `name`. Subsequent
    /// `call_default` / `get_export` calls reference the returned handle.
    ///
    /// Modules are evaluated immediately (top-level await is awaited).
    fn execute_module(&mut self, name: &str, source: &str) -> Result<ModuleHandle>;

    /// Call the module's `default` export with `props`, expecting a string
    /// result (the rendered HTML for the current page).
    fn call_default(&mut self, handle: &ModuleHandle, props: JsonValue) -> Result<String>;

    /// Read a named export from `handle` and return it as JSON. Used for the
    /// `meta` and `paths` exports.
    fn get_export(&mut self, handle: &ModuleHandle, name: &str) -> Result<JsonValue>;
}
