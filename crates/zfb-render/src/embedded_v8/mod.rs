//! In-process V8 host for the SSG renderer.
//!
//! ADR-007 (sub-issue #161) re-introduces an in-process JS runtime to
//! remove the Node.js + miniflare subprocess from the build-time render
//! path. The host loads a workerd-shape bundle (`export default { fetch }`)
//! produced by `zfb-build`'s bundler, drives its `fetch` export per
//! request, and surfaces V8 stack traces in a shape that
//! [`crate::sourcemap::decode_position`] can re-project to the original
//! `.tsx` lines.
//!
//! ## Architecture
//!
//! - [`EmbeddedV8RenderHost`] owns the [`deno_core::JsRuntime`] and a
//!   single tokio current-thread runtime (V8 isolates are pinned to
//!   the thread that creates them, so the host is `!Send + !Sync` —
//!   matches the `RenderHost` trait invariant).
//!
//! - The bundle is loaded as the runtime's *main* ESM module via a
//!   custom [`module_loader::BundleModuleLoader`] that:
//!
//!   - Serves the bundle source from an in-memory string.
//!   - Resolves `node:*` to the throwing-proxy stubs in
//!     [`extensions`].
//!   - Resolves `ext:zfb_node_stubs/*` helper imports.
//!   - Refuses any other unknown specifier with a clear error
//!     (the workerd-shape bundle is supposed to be self-contained;
//!     a stray bare-import is a bundler bug, not something to absorb).
//!
//! - Web Platform globals (`Request`, `Response`, `Headers`, `URL`,
//!   `URLSearchParams`, `TextEncoder`, `TextDecoder`, `atob`, `btoa`,
//!   `structuredClone`, a minimal `crypto`) are installed via a
//!   pure-JS polyfill at host boot time —
//!   [`extensions::WEB_POLYFILLS_SRC`]. We deliberately do NOT use
//!   `deno_fetch` / `deno_web`; see `Cargo.toml`'s comment block
//!   "Why a polyfill instead of deno_fetch/deno_web" for the
//!   trade-off (heavy compile, lazy-load bootstrap surface, and the
//!   SSG path never makes outgoing network requests so the
//!   `deno_fetch` hyper/rustls/h2/tower stack is dead weight).
//!
//! - Per-dispatch flow: the host stashes the bundle's `default`
//!   export at boot, then for each call it invokes the JS-side
//!   `__zfb.dispatch(url, method, headers, body)` (see
//!   `extensions::HOST_GLOBALS_SHIM_SRC`) which builds a JS
//!   `Request`, awaits `default.fetch(req)`, materialises the
//!   response body via `arrayBuffer()`, and returns
//!   `{ status, headers, body }` as a JS object. The Rust side
//!   pulls those fields back out via `serde_v8` deserialisation.
//!
//! ## node:* stubs
//!
//! Five specifiers in the v1 list (`node:fs`, `node:fs/promises`,
//! `node:path`, `node:url`, `node:buffer`) resolve at module-load time
//! to throwing-proxy stubs. Each member access throws
//! `Error("node:* is not available under the SSG runtime")` so user
//! code that imports a Node namespace for a code path that only fires
//! under Workers / production SSR continues to *load*; only actual
//! invocation fails. ADR-007 documents the rationale (allows
//! Workers-targeted user code to opt into SSG mode without bundler-
//! time conditional compilation).
//!
//! ## Panic safety
//!
//! V8 isolates allocate native resources (heap arena, microtask
//! queue, op state). We release those resources by dropping the
//! `JsRuntime` value when the host is dropped. A panic inside any
//! `RenderHost` method unwinds out through the host's owned
//! `JsRuntime`; `JsRuntime`'s own `Drop` impl tears down V8 cleanly.
//! The `tests/embedded_v8_smoke.rs::isolate_drops_cleanly_on_panic`
//! test wraps the whole lifecycle in `catch_unwind` and asserts the
//! drop runs without leaking.
//!
//! ## Concurrency
//!
//! Single-threaded by design. V8 isolates are not `Send`, deno_core's
//! ops use `Rc<RefCell<...>>` internally, and we don't want the
//! complexity of a host-per-thread pool on the SSG critical path
//! (the renderer drives one URL at a time today). If parallel SSG
//! ever lands, expect to spawn one host per worker thread and route
//! by route-key, not to make this host `Send`.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use async_trait::async_trait;
use deno_core::{
    serde_v8, v8, JsRuntime, ModuleId, ModuleSpecifier, PollEventLoopOptions, RuntimeOptions,
};
use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::error::{RenderError, Result};
use crate::render_host::{ModuleHandle, RenderHost};

mod dispatch;
pub mod extensions;
mod module_loader;

pub use dispatch::{HttpRequestLike, HttpResponseLike};
pub use module_loader::BundleModuleLoader;

/// Synthetic main-module specifier used when the caller calls
/// [`EmbeddedV8RenderHost::execute_module`] without supplying a URL.
/// Bundles are self-contained, so the URL only affects diagnostic
/// stack-trace text.
pub const DEFAULT_MAIN_SPECIFIER: &str = "file:///zfb-bundle/main.mjs";

/// In-process V8 host. See module docstring.
///
/// `EmbeddedV8RenderHost` is deliberately `!Send + !Sync` — V8 isolates
/// are pinned to the thread that creates them, and `deno_core`'s
/// `JsRuntime` is `!Send` for that reason. Callers that span threads
/// must park the host on a dedicated thread and route via channels.
pub struct EmbeddedV8RenderHost {
    runtime: JsRuntime,
    loader: Rc<BundleModuleLoader>,
    /// `(specifier → (ModuleHandle, ModuleId))` so subsequent
    /// `call_default` / `get_export` calls can find the underlying
    /// `deno_core` `ModuleId`.
    handles: RefCell<BTreeMap<String, (ModuleHandle, ModuleId)>>,
    /// Counter for [`ModuleHandle::id`]. Sequential integers; opaque to
    /// callers.
    next_handle_id: RefCell<u32>,
    /// Set when [`Self::install_bundle_default`] has wired the
    /// bundle's `default` export into `globalThis.__zfb` so
    /// [`Self::dispatch_fetch`] can find it.
    bundle_installed: RefCell<bool>,
    /// Specifier of the most-recently installed bundle (the one
    /// `dispatch_fetch` will drive). `None` until `install_bundle_default`
    /// runs.
    active_bundle_specifier: RefCell<Option<String>>,
}

// SAFETY-of-shape note: explicitly `!Send + !Sync` via `*const ()`-style
// markers would be redundant here because `JsRuntime` is `!Send + !Sync`
// already. We document the invariant via a doc-comment rather than a
// runtime check; `assert_not_impl_*` would require the
// `static_assertions` crate which is not in the workspace dep graph.
//
// If a future change introduces a field that *adds* `Send`/`Sync` to
// the struct, the trait impl `RenderHost for EmbeddedV8RenderHost`
// would still compile (the trait is `?Send`), so we'd lose the
// invariant silently. Reviewers: when adding fields to this struct,
// confirm the field type is `!Send` if you want to preserve the
// invariant. The default — anything containing a `Rc<...>`,
// `RefCell<...>`, or the existing `JsRuntime` — preserves it.

impl EmbeddedV8RenderHost {
    /// Create a new host with the default extension set
    /// (`deno_fetch` + `deno_web` + the node:* stubs + the host
    /// globals shim).
    ///
    /// First-call cost is dominated by V8's snapshot warmup. The host
    /// is intended to be created **once per build** and reused across
    /// all routes. See ADR-007 for the lifecycle contract.
    pub fn new() -> Result<Self> {
        Self::with_loader(BundleModuleLoader::new())
    }

    /// Create a host with a caller-supplied loader. Tests use this
    /// to inject extra in-memory modules (e.g. a stub `hono`).
    pub fn with_loader(loader: BundleModuleLoader) -> Result<Self> {
        let loader = Rc::new(loader);
        // The host runs on a bare `deno_core::JsRuntime` with no
        // extra extensions. Web Platform globals (Request / Response /
        // Headers / URL / fetch / TextEncoder / TextDecoder / atob /
        // btoa / structuredClone / crypto) are installed in pure JS
        // by `bootstrap_host_shim` from
        // `extensions::WEB_POLYFILLS_SRC`. See `Cargo.toml` "Why a
        // polyfill instead of deno_fetch/deno_web" for the trade-off.
        let extensions = build_extensions();
        let runtime = JsRuntime::new(RuntimeOptions {
            module_loader: Some(loader.clone()),
            extensions,
            ..Default::default()
        });
        let mut host = Self {
            runtime,
            loader,
            handles: RefCell::new(BTreeMap::new()),
            next_handle_id: RefCell::new(1),
            bundle_installed: RefCell::new(false),
            active_bundle_specifier: RefCell::new(None),
        };
        host.bootstrap_host_shim()?;
        Ok(host)
    }

    /// Install the web platform polyfills and the host-bridge globals
    /// shim. Order matters: the polyfills (Request / Response / URL /
    /// fetch / encoders / etc.) ship before the host shim so the
    /// host shim's `dispatch(...)` helper can use them when it
    /// constructs a Request from `dispatch_fetch`'s arguments.
    fn bootstrap_host_shim(&mut self) -> Result<()> {
        self.runtime
            .execute_script("zfb:web_polyfills", extensions::WEB_POLYFILLS_SRC)
            .map_err(|e| {
                RenderError::Runtime(format!("web polyfills init failed: {e}"))
            })?;
        self.runtime
            .execute_script("zfb:host_shim", extensions::HOST_GLOBALS_SHIM_SRC)
            .map_err(|e| RenderError::Runtime(format!("host shim init failed: {e}")))?;
        Ok(())
    }

    /// Install the bundle's `default` export into the host shim so
    /// [`Self::dispatch_fetch`] can find it. Called automatically by
    /// [`Self::execute_module`].
    fn install_bundle_default(&mut self, module_id: ModuleId, specifier: &str) -> Result<()> {
        // Pull the bundle's namespace and read `default` off it.
        let namespace = self
            .runtime
            .get_module_namespace(module_id)
            .map_err(|e| RenderError::Runtime(format!("get_module_namespace failed: {e}")))?;
        // Enter a context scope to access v8 locals, then build a
        // TryCatch around the call so a throw from `setBundle`
        // (only possible if the host shim is corrupted) surfaces
        // as a Rust error rather than aborting the host.
        deno_core::scope!(scope, &mut self.runtime);
        let local_ns: v8::Local<v8::Object> = v8::Local::new(scope, namespace);
        // Read `.default`.
        let key = v8::String::new(scope, "default")
            .ok_or_else(|| RenderError::Runtime("v8 string alloc failed".into()))?;
        let default_val = local_ns.get(scope, key.into()).ok_or_else(|| {
            RenderError::Runtime(format!(
                "bundle `{specifier}` has no `default` export — \
                 expected workerd shape `export default {{ fetch }}`"
            ))
        })?;
        if !default_val.is_object() {
            return Err(RenderError::Runtime(format!(
                "bundle `{specifier}` `default` export is not an object \
                 (workerd shape requires `export default {{ fetch }}`)"
            )));
        }
        // Look up `globalThis.__zfb.setBundle`.
        let global = scope.get_current_context().global(scope);
        let zfb_key = v8::String::new(scope, "__zfb")
            .ok_or_else(|| RenderError::Runtime("v8 string alloc failed".into()))?;
        let zfb_obj = global
            .get(scope, zfb_key.into())
            .ok_or_else(|| RenderError::Runtime("__zfb not on globalThis".into()))?;
        let zfb_obj: v8::Local<v8::Object> = zfb_obj.try_into().map_err(|_| {
            RenderError::Runtime("__zfb is not an object (host shim missing)".into())
        })?;
        let set_bundle_key = v8::String::new(scope, "setBundle")
            .ok_or_else(|| RenderError::Runtime("v8 string alloc failed".into()))?;
        let set_bundle_fn = zfb_obj
            .get(scope, set_bundle_key.into())
            .ok_or_else(|| RenderError::Runtime("__zfb.setBundle missing".into()))?;
        let set_bundle_fn: v8::Local<v8::Function> = set_bundle_fn.try_into().map_err(|_| {
            RenderError::Runtime("__zfb.setBundle is not a function".into())
        })?;
        let recv: v8::Local<v8::Value> = zfb_obj.into();
        // Build a TryCatch via the macro so the scope plumbing is
        // correct — see v8 crate's `tc_scope!` for the pinned-ref
        // shape. The exception is only ever non-empty if the host
        // shim itself is corrupted, but we surface it cleanly all
        // the same.
        v8::tc_scope!(tc, scope);
        let _ = set_bundle_fn.call(tc, recv, &[default_val]);
        if tc.has_caught() {
            let exc = tc.exception();
            let msg = exc
                .map(|e| e.to_rust_string_lossy(tc))
                .unwrap_or_else(|| "<no exception>".to_string());
            return Err(RenderError::Runtime(format!(
                "__zfb.setBundle threw: {msg}"
            )));
        }
        *self.bundle_installed.borrow_mut() = true;
        *self.active_bundle_specifier.borrow_mut() = Some(specifier.to_string());
        Ok(())
    }

    /// Dispatch a single request through the bundle's
    /// `default.fetch` and return the materialised
    /// [`HttpResponseLike`]. This is the load-bearing entrypoint for
    /// the build-time renderer (sub-issue #164 will wire it).
    ///
    /// `execute_module` MUST have been called for at least one bundle
    /// before this is called; otherwise the host returns a `Runtime`
    /// error.
    pub async fn dispatch_fetch(
        &mut self,
        request: HttpRequestLike,
    ) -> Result<HttpResponseLike> {
        if !*self.bundle_installed.borrow() {
            return Err(RenderError::Runtime(
                "embedded V8 host: dispatch_fetch called before any bundle was loaded \
                 (call execute_module() first)"
                    .into(),
            ));
        }
        // Drive the JS-side `__zfb.dispatch(url, method, headers, body)`
        // helper. It returns a Promise; we wait for it via
        // `with_event_loop_promise` which polls the V8 event loop
        // CONCURRENTLY with the promise resolution future. Calling
        // bare `runtime.resolve(...).await` would deadlock because
        // the future depends on microtasks that only fire while the
        // event loop is being polled.
        //
        // Result shape:
        //   { status: number, headers: Record<string, string>, body: Uint8Array }
        let promise = self.invoke_dispatch_js(&request)?;
        let resolve_future = self.runtime.resolve(promise);
        let resolved = self
            .runtime
            .with_event_loop_promise(
                Box::pin(resolve_future),
                PollEventLoopOptions::default(),
            )
            .await
            .map_err(|e| RenderError::Runtime(format_js_error(&e)))?;
        // Pull the resolved JS object back out as a Rust struct.
        deno_core::scope!(scope, &mut self.runtime);
        let local = v8::Local::new(scope, resolved);
        let parsed: DispatchResult = serde_v8::from_v8(scope, local).map_err(|e| {
            RenderError::Runtime(format!("failed to deserialise dispatch result: {e}"))
        })?;
        let mut headers = BTreeMap::new();
        for (k, v) in parsed.headers {
            headers.insert(k.to_lowercase(), v);
        }
        Ok(HttpResponseLike {
            status: parsed.status,
            headers,
            // Copy the body out of v8's backing store so the
            // response is detached from the JsRuntime's lifetime
            // (the next dispatch_fetch call may invalidate the
            // backing store via GC).
            body: parsed.body.as_ref().to_vec(),
        })
    }

    /// Invoke `__zfb.dispatch(...)` for `request` and return the
    /// resulting v8 Promise as a `Global<Value>`. The caller is
    /// responsible for awaiting / resolving it.
    fn invoke_dispatch_js(
        &mut self,
        request: &HttpRequestLike,
    ) -> Result<v8::Global<v8::Value>> {
        // We construct the call as a small JS expression rather than
        // wrestling with v8::Function::call from Rust — `serde_v8`'s
        // round trip on the input arguments is brittle when the body
        // is a `Uint8Array`, and the expression form is what
        // `deno_core` itself uses internally for similar plumbing.
        let url = request.url.clone();
        let method = request.method.clone();
        // Serialise headers + body as JSON literals embedded in the
        // expression. Bodies are rare on the SSG path (GETs) so the
        // base64 round-trip cost is negligible; we go through a
        // simple `Uint8Array.from(numberArray)` to avoid pulling in
        // a base64 polyfill for the host shim.
        let headers_literal = serde_json::to_string(&request.headers).map_err(|e| {
            RenderError::Runtime(format!("encoding request headers as JSON failed: {e}"))
        })?;
        let body_literal = match &request.body {
            None => "undefined".to_string(),
            Some(bytes) if bytes.is_empty() => "undefined".to_string(),
            Some(bytes) => {
                let nums: Vec<String> = bytes.iter().map(|b| b.to_string()).collect();
                format!("Uint8Array.from([{}])", nums.join(","))
            }
        };
        let url_literal = serde_json::to_string(&url).map_err(|e| {
            RenderError::Runtime(format!("encoding request URL as JSON failed: {e}"))
        })?;
        let method_literal = serde_json::to_string(&method).map_err(|e| {
            RenderError::Runtime(format!("encoding request method as JSON failed: {e}"))
        })?;
        let script = format!(
            "globalThis.__zfb.dispatch({url}, {method}, {headers}, {body})",
            url = url_literal,
            method = method_literal,
            headers = headers_literal,
            body = body_literal,
        );
        let result = self
            .runtime
            .execute_script("zfb:dispatch", script)
            .map_err(|e| RenderError::Runtime(format_js_error(&e)))?;
        Ok(result)
    }

    fn allocate_handle(&self, name: &str) -> ModuleHandle {
        let mut next = self.next_handle_id.borrow_mut();
        let id = *next;
        *next = next.checked_add(1).unwrap_or(1);
        ModuleHandle::new(id, name)
    }

    /// Lookup `(handle, module_id)` for an already-registered module.
    fn module_id_for(&self, handle: &ModuleHandle) -> Option<ModuleId> {
        self.handles
            .borrow()
            .get(&handle.name)
            .map(|(_, id)| *id)
    }
}

#[async_trait(?Send)]
impl RenderHost for EmbeddedV8RenderHost {
    async fn execute_module(&mut self, name: &str, source: &str) -> Result<ModuleHandle> {
        // Pick a module URL — the caller's `name` is a display path
        // (e.g. `pages/index.tsx`) which isn't a valid `file://` URL
        // by itself. We synthesise a `file:///zfb/<name>` so v8 has
        // something well-formed to print in stack frames; the URL
        // never hits the filesystem because the loader serves source
        // out of memory.
        //
        // For the well-known case where `name` already looks like a
        // bundle main (e.g. `bundle.mjs`), we use a stable URL so
        // sourcemap re-projection in `crate::sourcemap` consistently
        // sees the same prefix across runs.
        let specifier = synthesise_specifier(name);
        self.loader.register_module(specifier.as_str(), source);
        let module_specifier = ModuleSpecifier::parse(specifier.as_str()).map_err(|e| {
            RenderError::Runtime(format!(
                "embedded V8 host: bad synthetic specifier `{specifier}`: {e}"
            ))
        })?;
        // Load the module + transitive deps via `load_main_es_module`.
        // The loader has the source pre-registered so this is a
        // single dispatch through `BundleModuleLoader::load`.
        let module_id = self
            .runtime
            .load_main_es_module(&module_specifier)
            .await
            .map_err(|e| RenderError::Runtime(format!("load_main_es_module failed: {e}")))?;
        // Evaluate. `mod_evaluate` returns a future that resolves
        // when top-level await settles; we drive the event loop in
        // parallel so both sides progress.
        let evaluate = self.runtime.mod_evaluate(module_id);
        self.runtime
            .run_event_loop(PollEventLoopOptions::default())
            .await
            .map_err(|e| RenderError::Runtime(format_event_loop_error(&e)))?;
        evaluate
            .await
            .map_err(|e| RenderError::Runtime(format!("module evaluation failed: {e}")))?;
        // Wire the bundle's default export into the host shim so
        // dispatch_fetch can find it. This is the workerd-shape
        // contract; if the module isn't shaped that way the call
        // here surfaces a clear error.
        if let Err(e) = self.install_bundle_default(module_id, name) {
            // Some callers (the existing render-orchestrator path)
            // load utility modules that don't carry a `default`
            // export. We tolerate that *non-fatally* for
            // execute_module compatibility — dispatch_fetch will
            // still error if called before a shaped bundle is
            // registered.
            tracing_warn(&format!(
                "embedded V8 host: bundle `{name}` not workerd-shaped ({e}); dispatch_fetch disabled until a shaped module loads"
            ));
        }
        let handle = self.allocate_handle(name);
        self.handles
            .borrow_mut()
            .insert(handle.name.clone(), (handle.clone(), module_id));
        Ok(handle)
    }

    async fn call_default(&mut self, handle: &ModuleHandle, props: JsonValue) -> Result<String> {
        // The `RenderHost` trait's `call_default` predates the
        // workerd-shape contract — it expects the module's `default`
        // export to be a function returning HTML directly. The
        // embedded host honours that for non-workerd modules
        // (utility shims used by tests) by calling `default(props)`
        // and stringifying the return value. For the workerd-shape
        // path the production caller goes through
        // [`Self::dispatch_fetch`] instead.
        let module_id = self.module_id_for(handle).ok_or_else(|| {
            RenderError::Runtime(format!(
                "embedded V8 host: no such module handle `{name}`",
                name = handle.name
            ))
        })?;
        let namespace = self
            .runtime
            .get_module_namespace(module_id)
            .map_err(|e| RenderError::Runtime(format!("get_module_namespace failed: {e}")))?;
        // Build the call from inside a scope so we can reach v8.
        let promise = {
            deno_core::scope!(scope, &mut self.runtime);
            let local_ns: v8::Local<v8::Object> = v8::Local::new(scope, namespace);
            let key = v8::String::new(scope, "default")
                .ok_or_else(|| RenderError::Runtime("v8 string alloc failed".into()))?;
            let default_val = local_ns.get(scope, key.into()).ok_or_else(|| {
                RenderError::MissingDefaultExport(handle.name.clone())
            })?;
            if !default_val.is_function() {
                return Err(RenderError::MissingDefaultExport(handle.name.clone()));
            }
            let func: v8::Local<v8::Function> = default_val.try_into().map_err(|_| {
                RenderError::MissingDefaultExport(handle.name.clone())
            })?;
            // Marshal `props` from JSON to a v8 Value via serde_v8.
            let props_v8 = serde_v8::to_v8(scope, props).map_err(|e| {
                RenderError::Runtime(format!("encoding props for default() failed: {e}"))
            })?;
            let recv: v8::Local<v8::Value> = local_ns.into();
            v8::tc_scope!(tc, scope);
            let result = func.call(tc, recv, &[props_v8]);
            if tc.has_caught() {
                let exc = tc.exception();
                let msg = exc
                    .map(|e| e.to_rust_string_lossy(tc))
                    .unwrap_or_else(|| "<no exception>".to_string());
                return Err(RenderError::Runtime(msg));
            }
            let result = result.ok_or_else(|| {
                RenderError::Runtime("default() returned no value".into())
            })?;
            v8::Global::new(tc, result)
        };
        // If the result was a Promise, await it; else short-circuit.
        // Use `with_event_loop_promise` so microtasks queued by the
        // promise's chain are actually drained.
        let resolve_future = self.runtime.resolve(promise);
        let resolved = self
            .runtime
            .with_event_loop_promise(
                Box::pin(resolve_future),
                PollEventLoopOptions::default(),
            )
            .await
            .map_err(|e| RenderError::Runtime(format_js_error(&e)))?;
        deno_core::scope!(scope, &mut self.runtime);
        let local = v8::Local::new(scope, resolved);
        if let Some(s) = local_to_string_if_string(scope, local) {
            return Ok(s);
        }
        // Allow returning a string-like via .toString()
        let s = local
            .to_string(scope)
            .ok_or_else(|| RenderError::Runtime("default() return value not stringifiable".into()))?
            .to_rust_string_lossy(scope);
        Ok(s)
    }

    async fn get_export(&mut self, handle: &ModuleHandle, name: &str) -> Result<JsonValue> {
        let module_id = self.module_id_for(handle).ok_or_else(|| {
            RenderError::Runtime(format!(
                "embedded V8 host: no such module handle `{name}`",
                name = handle.name
            ))
        })?;
        let namespace = self
            .runtime
            .get_module_namespace(module_id)
            .map_err(|e| RenderError::Runtime(format!("get_module_namespace failed: {e}")))?;
        deno_core::scope!(scope, &mut self.runtime);
        let local_ns: v8::Local<v8::Object> = v8::Local::new(scope, namespace);
        let key = v8::String::new(scope, name)
            .ok_or_else(|| RenderError::Runtime("v8 string alloc failed".into()))?;
        let value = local_ns
            .get(scope, key.into())
            .ok_or_else(|| RenderError::Runtime(format!("export `{name}` not found")))?;
        let json: JsonValue = serde_v8::from_v8(scope, value).map_err(|e| {
            RenderError::Runtime(format!("export `{name}` is not JSON-serialisable: {e}"))
        })?;
        Ok(json)
    }
}

// Helper for the lossy string check. The v8 crate's
// `to_rust_string_lossy(&Isolate)` works on a `Local<v8::String>`;
// we accept `&Isolate` here so the caller can pass either the
// isolate or anything that derefs to it.
fn local_to_string_if_string(
    isolate: &v8::Isolate,
    local: v8::Local<v8::Value>,
) -> Option<String> {
    if local.is_string() {
        let s: v8::Local<v8::String> = local.try_into().ok()?;
        Some(s.to_rust_string_lossy(isolate))
    } else {
        None
    }
}

/// Turn a display name (e.g. `pages/index.tsx`) into a stable
/// `file:///zfb/<name>` URL the V8 stack-trace formatter is happy to
/// print. The loader registers in-memory source against this URL.
///
/// We percent-encode any byte that would otherwise break URL parsing
/// (space, control chars, non-ASCII high bytes, plus the URL-reserved
/// `?`, `#`, `%` set). Slash separators are preserved so the
/// resulting URL retains its path segment shape — this is what the
/// V8 stack-trace formatter prints back, and operators want
/// `pages/blog/[slug].tsx` not the same string with `/` encoded.
fn synthesise_specifier(name: &str) -> String {
    if name.starts_with("file://") {
        return name.to_string();
    }
    let trimmed = name.trim_start_matches('/');
    let mut out = String::from("file:///zfb/");
    for byte in trimmed.bytes() {
        match byte {
            // Preserve path-shape characters and most ASCII safe
            // characters. The unreserved set per RFC 3986 plus a
            // couple of pragmatic additions (`/` for path
            // segments, `[`/`]` so `pages/blog/[slug].tsx` lands
            // verbatim — square brackets are technically reserved
            // but the URL parser tolerates them in path).
            b'a'..=b'z'
            | b'A'..=b'Z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b'/'
            | b'['
            | b']' => out.push(byte as char),
            other => {
                out.push('%');
                out.push_str(&format!("{:02X}", other));
            }
        }
    }
    out
}

/// Build the deno_core extension list. We currently ship NO extra
/// extensions — Request / Response / Headers / URL / fetch live in
/// the JS polyfill (`extensions::WEB_POLYFILLS_SRC`) instead of the
/// `deno_fetch` / `deno_web` extensions. See `Cargo.toml`'s comment
/// "Why a polyfill instead of deno_fetch/deno_web" for the trade-off
/// (heavy compile + tricky lazy-load bootstrap vs. ~250 lines of
/// hermetic JS).
///
/// Kept as a function so a future swap to `deno_web` / `deno_fetch`
/// is a one-place change.
fn build_extensions() -> Vec<deno_core::Extension> {
    vec![]
}

/// Format a `deno_core::error::CoreError` for inclusion in
/// `RenderError::Runtime`. The string MUST embed any V8 stack frames
/// verbatim — `crate::sourcemap::find_frame_candidates` re-projects
/// them downstream, so dropping a frame here breaks the diagnostics
/// pipeline.
fn format_js_error<E: std::fmt::Display>(e: &E) -> String {
    // The `deno_core::error::JsError` Display impl already includes
    // `name`, `message`, and the multi-line stack with
    // `<specifier>:LINE:COL` frames. We just pass it through.
    e.to_string()
}

fn format_event_loop_error<E: std::fmt::Display>(e: &E) -> String {
    format!("event loop error: {e}")
}

/// Lightweight no-op tracing replacement. The crate doesn't currently
/// pull `tracing` as a hard dep so we route diagnostic logs through
/// `eprintln!` when the env-flag opts in. Used only for the
/// non-workerd-shape tolerance branch in `execute_module`.
fn tracing_warn(msg: &str) {
    // Gate behind an env flag so the smoke tests stay quiet. Set
    // `ZFB_RENDER_DEBUG=1` in CI when chasing host-side oddities.
    if std::env::var_os("ZFB_RENDER_DEBUG").is_some() {
        eprintln!("[zfb-render] {msg}");
    }
}

/// Internal shape returned by `__zfb.dispatch(...)`.
///
/// `body` is materialised as a `serde_v8::JsBuffer` so the
/// `Uint8Array` from JS round-trips without a Vec<u8> coercion. We
/// then copy out into `Vec<u8>` for the caller-facing
/// [`HttpResponseLike`] (the JsBuffer holds onto a v8 backing store
/// reference; copying decouples the response from the v8 lifetime).
#[derive(Deserialize)]
struct DispatchResult {
    status: u16,
    headers: BTreeMap<String, String>,
    body: deno_core::JsBuffer,
}
