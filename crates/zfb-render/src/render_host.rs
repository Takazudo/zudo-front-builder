//! `RenderHost` trait — the abstraction boundary between the renderer and the
//! underlying JS runtime.
//!
//! The plan recommendation is `deno_core` (V8); ADR-001 (in-flight) formally
//! locks the runtime choice. We expose a trait so a different host can be
//! swapped in once the ADR lands without touching call sites.
//!
//! The deno_core-backed implementation lives behind the `deno_core_host`
//! feature flag and is currently a skeleton — Sub 1's measured spike + ADR-001
//! is the source of truth and may pick a different runtime.

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

/// Abstraction over the JS runtime. Implementations:
///
/// * `DenoCoreHost` (feature `deno_core_host`) — V8 via deno_core, plan default.
///
/// Add a new impl by satisfying these three operations.
///
/// The trait is intentionally **not** `Send` / `Sync`: V8 isolates are pinned
/// to a single thread, and `quickjs-rs` keeps a similar invariant. Hosts that
/// span threads do so by running the isolate on a dedicated thread and
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

// ---------------------------------------------------------------------------
// deno_core-backed implementation (gated).
// ---------------------------------------------------------------------------

/// V8/deno_core-backed `RenderHost`.
///
/// **Status:** skeleton awaiting ADR-001 + Sub 4's adapter wiring. The
/// constructor initialises a `JsRuntime`, but `execute_module` /
/// `call_default` / `get_export` are not yet wired into the deno_core module
/// loader (ES module loader, top-level await driver, source-mapped errors).
///
/// TODO(ADR-001): finalise the runtime choice and either flesh this out or
/// replace it with the chosen alternative.
#[cfg(feature = "deno_core_host")]
pub struct DenoCoreHost {
    // We keep the runtime in an `Option` so `execute_module` can take ownership
    // around an `&mut self` boundary if needed. The `Option` is always `Some`
    // outside of in-flight calls.
    runtime: Option<deno_core::JsRuntime>,
    next_id: u32,
}

#[cfg(feature = "deno_core_host")]
impl DenoCoreHost {
    /// Build a new host with default deno_core options.
    ///
    /// V8's platform must be initialised exactly once per process. deno_core's
    /// `JsRuntime::new` handles that internally.
    pub fn new() -> Self {
        let runtime = deno_core::JsRuntime::new(deno_core::RuntimeOptions::default());
        Self {
            runtime: Some(runtime),
            next_id: 0,
        }
    }

    fn mint_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }
}

#[cfg(feature = "deno_core_host")]
impl Default for DenoCoreHost {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "deno_core_host")]
impl RenderHost for DenoCoreHost {
    fn execute_module(&mut self, name: &str, _source: &str) -> Result<ModuleHandle> {
        // TODO(ADR-001 / Sub 4): wire deno_core's ES module loader, then
        // `load_main_es_module_from_code` + `mod_evaluate` + driver pump.
        let _ = self.runtime.as_mut();
        let id = self.mint_id();
        Ok(ModuleHandle::new(id, name))
    }

    fn call_default(&mut self, handle: &ModuleHandle, _props: JsonValue) -> Result<String> {
        // TODO(ADR-001 / Sub 4): grab module namespace, look up `default`,
        // call it, coerce the return value to a string. Returns a placeholder
        // so smoke tests run without V8 wired in.
        Err(crate::RenderError::Runtime(format!(
            "DenoCoreHost::call_default not yet implemented for `{}` (ADR-001 pending)",
            handle.name
        )))
    }

    fn get_export(&mut self, handle: &ModuleHandle, name: &str) -> Result<JsonValue> {
        // TODO(ADR-001 / Sub 6): pull `name` off the module namespace,
        // serialise via serde_v8 → serde_json::Value.
        Err(crate::RenderError::Runtime(format!(
            "DenoCoreHost::get_export(`{}`) not yet implemented for `{}` (ADR-001 pending)",
            name, handle.name
        )))
    }
}
