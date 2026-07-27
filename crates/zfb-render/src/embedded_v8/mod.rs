//! In-process V8 host for the SSG renderer.
//!
//! This host is an in-process JS runtime that replaces the external
//! Node.js subprocess from the build-time render
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
//! - Browser-Event globals (`Event`, `CustomEvent`, `EventTarget`)
//!   are installed via [`extensions::BROWSER_EVENT_SRC`]. Needed so
//!   bundles whose top-level code declares `class X extends Event`
//!   (e.g. `@takazudo/zfb-runtime`'s client-router events module)
//!   evaluate at all on the SSG path. See `js/browser_event.js` for
//!   the scope and intentional gaps.
//!
//! - Per-dispatch flow: the host stashes the bundle's `default`
//!   export at boot, then for each call it invokes the JS-side
//!   `__zfb.dispatch(url, method, headers, body, mode)` (see
//!   `extensions::HOST_GLOBALS_SHIM_SRC`) which builds a JS
//!   `Request`, awaits `default.fetch(req)`, materialises the
//!   response body via `arrayBuffer()`, and returns
//!   `{ status, headers, body }` as a JS object. The Rust side
//!   pulls those fields back out via `serde_v8` deserialisation.
//!
//! - Worker console output is captured by a shim installed at host
//!   boot (part of `extensions::HOST_GLOBALS_SHIM_SRC`): the levelled
//!   `console` methods are patched to buffer each line (capped) while
//!   still forwarding to the runtime's original stdout printer.
//!   [`EmbeddedV8RenderHost::drain_console_logs`] retrieves and clears
//!   the buffer so render failures can surface what the worker
//!   printed (issue #700).
//!
//! ## node:* stubs
//!
//! Five specifiers in the v1 list (`node:fs`, `node:fs/promises`,
//! `node:path`, `node:url`, `node:buffer`) resolve at module-load time
//! to throwing-proxy stubs. Each member access throws
//! `Error("node:* is not available under the SSG runtime")` so user
//! code that imports a Node namespace for a code path that only fires
//! under Workers / production SSR continues to *load*; only actual
//! invocation fails. This allows
//! Workers-targeted user code to opt into SSG mode without bundler-
//! time conditional compilation.
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

pub mod crypto;
mod dispatch;
pub mod extensions;
pub mod fetch;
#[cfg(test)]
mod js_crypto_tests;
#[cfg(test)]
mod js_fetch_tests;
pub mod limits;
#[cfg(test)]
pub(crate) mod loopback_test_server;
mod module_loader;

pub use dispatch::{DispatchMode, HttpRequestLike, HttpResponseLike};
pub use module_loader::{AliasHook, BundleModuleLoader, PluginRegistryHooks, VirtualModuleHook};

/// Encode `bytes` as a standard base64 string (RFC 4648, alphabet A-Za-z0-9+/).
///
/// Used to pass request bodies to the JS dispatch shim without building a
/// numeric-array literal (`Uint8Array.from([b0,b1,…])`), which grows O(N)
/// as source text that V8 must re-parse on every call.  The encoded form is
/// always a valid JSON string; the caller wraps it in `serde_json::to_string`
/// before embedding it in the dispatch script.
///
/// No external crate needed — the alphabet and padding rules are trivial and
/// adding a dep purely for this one site would bloat the compile graph.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((triple >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(triple & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Generate a per-host nonce for the dispatch-mode authorisation check
/// (issue #2014).
///
/// 256 bits from the OS CSPRNG, hex-encoded. The earlier construction
/// — process id, wall-clock nanoseconds, and a counter — was a value
/// bundle code could *reconstruct* rather than having to guess, which
/// is not the property a nonce needs. `getrandom` is already a
/// dependency of this crate under the same `embed_v8` feature (it backs
/// `crypto::OsEntropy`, issue #2017), so there is no cost to using it.
///
/// A CSPRNG failure is fatal rather than silently falling back to a
/// predictable value: a host that cannot produce a nonce is a host
/// whose mode authorisation would be forgeable.
fn generate_mode_nonce() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| {
        RenderError::Runtime(format!(
            "embedded V8 host: could not read OS entropy for the dispatch-mode nonce: {e}"
        ))
    })?;
    let mut out = String::with_capacity(2 * bytes.len() + 9);
    out.push_str("zfb-mode-");
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    Ok(out)
}

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
    /// Last error returned by [`Self::install_bundle_default`].  Set on
    /// failure, cleared on success.  Surfaced in [`Self::dispatch_fetch`]'s
    /// "no bundle loaded" error so operators can see why the install failed.
    last_install_error: RefCell<Option<String>>,
    /// Per-host nonce that authorises CHOOSING a [`DispatchMode`]
    /// (issue #2014).
    ///
    /// `globalThis.__zfb.dispatch` is necessarily reachable from the
    /// evaluated bundle, so without this a build-time handler could
    /// re-enter it with `"request-time"` and hand its nested handler the
    /// request-time branch. The shim honours the `mode` argument only
    /// when the caller presents this value; anything else INHERITS the
    /// enclosing dispatch's mode, so a forged call can never widen
    /// capability. The nonce is substituted into the shim source at boot
    /// and lives in that script's closure — it is never reachable as a
    /// property of any global.
    ///
    /// This guards against *selecting* the mode in JS, not against a
    /// hostile module in the same realm; the bundle is first-party code
    /// zfb itself compiled, and no JS-visible bridge can be made proof
    /// against that. The enforcement that does not depend on JS at all
    /// is [`fetch::DispatchModeState`], read inside the op.
    mode_nonce: String,
    /// Request-time limits published to JS as `__zfb.limits`.
    ///
    /// Normally exactly [`limits::limits_js_literal`]. Tests boot a
    /// host with individual caps overridden (see
    /// [`Self::with_limits_override`]) because the JS-visible object is
    /// frozen — bundle code can no longer lower a cap for the duration
    /// of one probe, which is the point.
    limits_json: String,
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
    /// Create a new host with the default extension set (a JS polyfill
    /// providing Web Platform globals + the node:* stubs + the host
    /// globals shim; no `deno_fetch` / `deno_web` Rust extensions — see
    /// `build_extensions()` and the `Cargo.toml` note "Why a polyfill
    /// instead of deno_fetch/deno_web").
    ///
    /// First-call cost is dominated by V8's snapshot warmup. The host
    /// is intended to be created **once per build** and reused across
    /// all routes.
    pub fn new() -> Result<Self> {
        Self::with_loader(BundleModuleLoader::new())
    }

    /// Create a host with a caller-supplied loader. Tests use this
    /// to inject extra in-memory modules (e.g. a stub `hono`).
    pub fn with_loader(loader: BundleModuleLoader) -> Result<Self> {
        Self::with_loader_and_limits(loader, limits::limits_js_literal())
    }

    /// Boot a host whose JS-visible `__zfb.limits` carries `overrides`
    /// merged over the real constants.
    ///
    /// Exists because the limits object is **frozen** (epic #2012
    /// review fix 5): bundle code used to be able to raise
    /// `maxRequestBodyBytes` and wave an oversized payload past the JS
    /// pre-check, and the same mutability was what let a test lower a
    /// cap for one probe. The cap is now moved where a real deployment
    /// would move it — at host boot, from Rust — so the tests that
    /// exercise the JS-side checks keep their exact assertions without
    /// the object having to stay writable.
    #[cfg(test)]
    pub(crate) fn with_limits_override(overrides: serde_json::Value) -> Result<Self> {
        let mut merged: serde_json::Value = serde_json::from_str(&limits::limits_js_literal())
            .expect("the rendered limits literal is valid JSON");
        let (serde_json::Value::Object(base), serde_json::Value::Object(patch)) =
            (&mut merged, overrides)
        else {
            panic!("limits overrides must be a JSON object");
        };
        for (key, value) in patch {
            assert!(
                base.contains_key(&key),
                "`{key}` is not a published limit — a typo here would silently test nothing"
            );
            base.insert(key, value);
        }
        Self::with_loader_and_limits(BundleModuleLoader::new(), merged.to_string())
    }

    fn with_loader_and_limits(loader: BundleModuleLoader, limits_json: String) -> Result<Self> {
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
            last_install_error: RefCell::new(None),
            mode_nonce: generate_mode_nonce()?,
            limits_json,
        };
        host.bootstrap_host_shim()?;
        Ok(host)
    }

    /// Install the web platform polyfills, the browser-event globals,
    /// and the host-bridge globals shim. Order matters: the polyfills
    /// (Request / Response / URL / fetch / encoders / etc.) and the
    /// browser-event globals (`Event` / `CustomEvent` / `EventTarget`)
    /// ship before the host shim so module-top-level
    /// `class X extends Event` declarations in the bundle evaluate
    /// against the stubbed globals, and so the host shim's
    /// `dispatch(...)` helper can use Request / Response when it
    /// constructs them from `dispatch_fetch`'s arguments.
    fn bootstrap_host_shim(&mut self) -> Result<()> {
        self.runtime
            .execute_script("zfb:web_polyfills", extensions::WEB_POLYFILLS_SRC)
            .map_err(|e| RenderError::Runtime(format!("web polyfills init failed: {e}")))?;
        self.runtime
            .execute_script("zfb:browser_event", extensions::BROWSER_EVENT_SRC)
            .map_err(|e| RenderError::Runtime(format!("browser-event globals init failed: {e}")))?;
        // Bake this host's mode nonce into the shim source before it
        // runs. The shim body is an IIFE, so the substituted value ends
        // up closure-private rather than in the global lexical
        // environment where bundle code could read it by name.
        //
        // The request-time limit constants (issue #2016) are baked in
        // by the same pass: `web_polyfills.js` reads its request-body
        // cap out of `__zfb.limits` rather than carrying a second copy
        // of the numbers in `limits.rs`.
        let shim_src = extensions::HOST_GLOBALS_SHIM_SRC
            .replace(extensions::MODE_NONCE_PLACEHOLDER, &self.mode_nonce)
            .replace(extensions::LIMITS_PLACEHOLDER, &self.limits_json);
        debug_assert!(
            !shim_src.contains(extensions::MODE_NONCE_PLACEHOLDER),
            "host shim still carries the mode-nonce placeholder after substitution"
        );
        debug_assert!(
            !shim_src.contains(extensions::LIMITS_PLACEHOLDER),
            "host shim still carries the limits placeholder after substitution"
        );
        self.runtime
            .execute_script("zfb:host_shim", shim_src)
            .map_err(|e| RenderError::Runtime(format!("host shim init failed: {e}")))?;
        Ok(())
    }

    /// Install the bundle's `default` export into the host shim so
    /// [`Self::dispatch_fetch`] can find it. Called automatically by
    /// [`Self::execute_module`].
    fn install_bundle_default(&mut self, module_id: ModuleId, specifier: &str) -> Result<()> {
        self.check_bundle_default_shape(module_id, specifier, true)
    }

    /// Strictly validate `module_id`'s `default` export against the
    /// workerd-shape contract WITHOUT wiring it into
    /// `globalThis.__zfb.setBundle` — i.e. without making it the
    /// `dispatch_fetch` target. Used to validate a module that is
    /// imported as a dependency (not run as the main module) — e.g.
    /// the dev content-trace wrapper's inner worker, whose own
    /// `default.fetch` is only touched lazily at dispatch time and so
    /// isn't caught by validating the wrapper's (always well-shaped)
    /// `default` export alone. See [`Self::validate_worker_module_shape`].
    fn validate_bundle_default_shape_only(
        &mut self,
        module_id: ModuleId,
        specifier: &str,
    ) -> Result<()> {
        self.check_bundle_default_shape(module_id, specifier, false)
    }

    /// Shared implementation for [`Self::install_bundle_default`] and
    /// [`Self::validate_bundle_default_shape_only`]. Always runs the full
    /// workerd-shape validation (missing/undefined default, non-object
    /// default, object without `fetch`, non-callable `fetch`); when
    /// `install` is `true`, additionally wires the validated `default`
    /// export into `globalThis.__zfb.setBundle` and flips
    /// `bundle_installed` so `dispatch_fetch` can find it.
    fn check_bundle_default_shape(
        &mut self,
        module_id: ModuleId,
        specifier: &str,
        install: bool,
    ) -> Result<()> {
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
        // `Object::get` only returns `None` on a thrown exception, not on a
        // genuinely absent property — a missing `default` export reads back
        // as the JS value `undefined`, same as an export explicitly set to
        // `undefined`. So the `None` arm below is an exception path (should
        // be unreachable for a plain namespace-object property read) and the
        // "missing default" diagnostic is produced by the `is_undefined()`
        // check that follows, not by this `ok_or_else`.
        let default_val = local_ns.get(scope, key.into()).ok_or_else(|| {
            RenderError::Runtime(format!(
                "bundle `{specifier}` `default` export lookup failed unexpectedly"
            ))
        })?;
        if default_val.is_undefined() {
            return Err(RenderError::Runtime(format!(
                "bundle `{specifier}` has no `default` export — \
                 expected workerd shape `export default {{ fetch }}`"
            )));
        }
        if !default_val.is_object() {
            // Covers `null` and any other non-object primitive default
            // (string, number, boolean, …) — distinct from the
            // "no default export at all" case above.
            return Err(RenderError::Runtime(format!(
                "bundle `{specifier}` `default` export is not an object \
                 (workerd shape requires `export default {{ fetch }}`)"
            )));
        }
        let default_obj: v8::Local<v8::Object> = default_val.try_into().map_err(|_| {
            RenderError::Runtime(format!(
                "bundle `{specifier}` `default` export could not be read as an object"
            ))
        })?;
        let fetch_key = v8::String::new(scope, "fetch")
            .ok_or_else(|| RenderError::Runtime("v8 string alloc failed".into()))?;
        let fetch_val = default_obj.get(scope, fetch_key.into()).ok_or_else(|| {
            RenderError::Runtime(format!(
                "bundle `{specifier}` `default` export has no `fetch` property \
                 (workerd shape requires `export default {{ fetch }}`)"
            ))
        })?;
        if fetch_val.is_undefined() {
            return Err(RenderError::Runtime(format!(
                "bundle `{specifier}` `default` export has no `fetch` property \
                 (workerd shape requires `export default {{ fetch }}`)"
            )));
        }
        let _fetch_fn: v8::Local<v8::Function> = fetch_val.try_into().map_err(|_| {
            RenderError::Runtime(format!(
                "bundle `{specifier}` `default.fetch` is not callable \
                 (workerd shape requires `export default {{ fetch }}`)"
            ))
        })?;
        if !install {
            // Shape-only validation (see `validate_bundle_default_shape_only`):
            // do NOT wire this module in as the dispatch_fetch target.
            return Ok(());
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
        let set_bundle_fn: v8::Local<v8::Function> = set_bundle_fn
            .try_into()
            .map_err(|_| RenderError::Runtime("__zfb.setBundle is not a function".into()))?;
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
    pub async fn dispatch_fetch(&mut self, request: HttpRequestLike) -> Result<HttpResponseLike> {
        if !*self.bundle_installed.borrow() {
            let base = "embedded V8 host: dispatch_fetch called before any bundle was loaded \
                 (call execute_module() first)";
            let msg = match self.last_install_error.borrow().as_deref() {
                Some(install_err) => format!("{base}; last install error: {install_err}"),
                None => base.to_string(),
            };
            return Err(RenderError::Runtime(msg));
        }
        // Issue #2015: the outbound-subrequest budget is per DISPATCH,
        // so it is opened here rather than anywhere in the op. Doing it
        // in Rust — not JS — is what stops a `Promise.all` fan-out in
        // bundle code from evading the cap.
        self.begin_dispatch_subrequest_budget();
        // Epic #2012 review fix 1: the mode reaches Rust as well as JS.
        // `__zfb.mode` is advisory — bundle code can call
        // `Deno.core.ops.op_zfb_fetch` without going near the polyfill,
        // and could until this fix swap the whole `__zfb` object out —
        // so the denial that actually holds is the one the op reads out
        // of `OpState`.
        self.install_dispatch_mode(request.mode);
        // Drive the JS-side `__zfb.dispatch(url, method, headers, body, mode)`
        // helper. It returns a Promise; we wait for it via
        // `with_event_loop_promise` which polls the V8 event loop
        // CONCURRENTLY with the promise resolution future. Calling
        // bare `runtime.resolve(...).await` would deadlock because
        // the future depends on microtasks that only fire while the
        // event loop is being polled.
        //
        // Result shape:
        //   { status: number, headers: Array<[string, string]>, body: Uint8Array }
        // `headers` is an ordered pair array, not a `Record`, so duplicate
        // names (notably `set-cookie`) survive the bridge instead of
        // collapsing to their last value.
        let promise = self.invoke_dispatch_js(&request)?;
        let resolve_future = self.runtime.resolve(promise);
        let resolved = self
            .runtime
            .with_event_loop_promise(Box::pin(resolve_future), PollEventLoopOptions::default())
            .await
            .map_err(|e| RenderError::Runtime(format_js_error(&e)))?;
        self.drain_cancelled_fetches().await;
        // Pull the resolved JS object back out as a Rust struct.
        deno_core::scope!(scope, &mut self.runtime);
        let local = v8::Local::new(scope, resolved);
        let parsed: DispatchResult = serde_v8::from_v8(scope, local).map_err(|e| {
            RenderError::Runtime(format!("failed to deserialise dispatch result: {e}"))
        })?;
        // Defensive lower-casing only — the JS side's `Headers` polyfill
        // already stores/iterates lowercase names. Kept as a `Vec`, not
        // refolded into a `BTreeMap`, so duplicate names (e.g. multiple
        // `set-cookie` values) survive rather than being collapsed by a
        // map key collision.
        let headers = parsed
            .headers
            .into_iter()
            .map(|(k, v)| (k.to_lowercase(), v))
            .collect();
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

    /// Open a fresh outbound-subrequest budget for the dispatch that is
    /// about to run (issue #2015).
    ///
    /// A **new counter is installed**, never the existing one zeroed.
    /// `with_event_loop_promise` can return while a `fetch` the handler
    /// started but never awaited is still pending; that orphan holds an
    /// `Rc` to the counter it began on, so zeroing in place would let it
    /// spend this dispatch's budget (and would forgive its own
    /// overspend). Replacing the entry leaves the orphan charging the
    /// dispatch it belongs to and hands the incoming dispatch a budget
    /// no one else can touch. See [`fetch::SubrequestCounter`].
    ///
    /// A host built without the fetch extension has no budget to open;
    /// installing one there would be harmless but pointless, and the op
    /// itself reports the host-op failure if it is ever called in that
    /// state.
    /// Poll the event loop until every cancellation flagged during this
    /// dispatch has actually been observed by the op it was aimed at.
    ///
    /// `op_zfb_fetch_cancel` only marks the `CancelHandle` and wakes it;
    /// the transport is dropped when the cancelable future is **polled
    /// again**. A handler that aborts a fetch and immediately returns
    /// its `Response` resolves the dispatch promise first, and
    /// `with_event_loop_promise` is free to return on that without ever
    /// polling the loop — which would leave the socket open, the
    /// response still buffering, and the registry entry live until
    /// something else happened to run the loop.
    ///
    /// **Honest status: defence in depth, not a repro.** With the
    /// current bridge shape that window is not actually reachable —
    /// `__zfb.dispatch` still has to `await bundle.fetch(req)` and
    /// `await resp.arrayBuffer()` after the abort, and
    /// `with_event_loop_promise` polls the loop across those, which
    /// drives the cancelled op. `an_abort_mid_body_cancels_the_transport_and_closes_the_socket`
    /// therefore passes with this call stubbed out; it is kept because
    /// the guarantee should not depend on that coincidence, and a
    /// future change to the shim's shape could remove it silently.
    ///
    /// Each pass is a **single non-blocking poll**: a cancelled future
    /// resolves on the first one, and polling is deliberately not driven
    /// to completion, since an unrelated in-flight fetch would otherwise
    /// hold the dispatch open until its own deadline. The pass count is
    /// bounded, and the counter is cleared afterwards, so a cancellation
    /// that raced a natural completion cannot charge every later
    /// dispatch for a poll budget nothing will ever consume.
    async fn drain_cancelled_fetches(&mut self) {
        const MAX_DRAIN_POLLS: usize = 16;
        let Some(cancels) = self
            .runtime
            .op_state()
            .borrow()
            .try_borrow::<Rc<fetch::CancelRegistry>>()
            .cloned()
        else {
            return;
        };
        let mut polls = 0;
        while cancels.pending_cancellations() > 0 && polls < MAX_DRAIN_POLLS {
            polls += 1;
            let _ = deno_core::futures::future::poll_fn(|cx| {
                let _ = self
                    .runtime
                    .poll_event_loop(cx, PollEventLoopOptions::default());
                std::task::Poll::Ready(())
            })
            .await;
            // Let the tokio reactor deliver the socket close the dropped
            // transport just triggered before the next pass looks again.
            tokio::task::yield_now().await;
        }
        cancels.clear_pending();
    }

    /// Publish `mode` where [`fetch::op_zfb_fetch`] reads it, in
    /// `OpState` (epic #2012 review fix 1).
    ///
    /// Deliberately **not** cleared when the dispatch settles. A
    /// handler can start a `fetch` it never awaits and still return its
    /// `Response`, at which point `with_event_loop_promise` returns
    /// while that call is still in flight; clearing here would deny the
    /// orphan a capability the dispatch it belongs to genuinely had.
    /// Nothing leaks by leaving it: every dispatch sets the mode
    /// unconditionally, and [`Self::reset_dispatch_mode_for_evaluation`]
    /// puts it back to build-time before any module evaluates, which is
    /// the only other place bundle code runs.
    fn install_dispatch_mode(&mut self, mode: DispatchMode) {
        let op_state = self.runtime.op_state();
        let mut op_state = op_state.borrow_mut();
        if op_state.try_borrow::<fetch::DispatchModeState>().is_some() {
            op_state.put(fetch::DispatchModeState(mode));
        }
    }

    /// Drop back to [`DispatchMode::BuildTime`] before evaluating a
    /// module.
    ///
    /// Module top-level code is bundle code running outside any
    /// dispatch. Since the mode installed by the previous dispatch is
    /// deliberately left standing (see
    /// [`Self::install_dispatch_mode`]), a bundle re-evaluated after a
    /// request-time render would otherwise inherit request-time
    /// capability at its top level. Also resets the JS-side view, so
    /// the polyfill's diagnostics agree with the op's decision.
    fn reset_dispatch_mode_for_evaluation(&mut self) {
        self.install_dispatch_mode(DispatchMode::BuildTime);
        let nonce = serde_json::to_string(&self.mode_nonce).expect("nonce is always valid JSON");
        // Best-effort: a host whose shim failed to install has bigger
        // problems, and the Rust-side reset above is the one that
        // matters.
        let _ = self.runtime.execute_script(
            "zfb:reset_mode",
            format!(
                "globalThis.__zfb && typeof globalThis.__zfb.resetMode === \"function\" \
                 && globalThis.__zfb.resetMode({nonce})"
            ),
        );
    }

    fn begin_dispatch_subrequest_budget(&mut self) {
        let op_state = self.runtime.op_state();
        let mut op_state = op_state.borrow_mut();
        if op_state
            .try_borrow::<Rc<fetch::SubrequestCounter>>()
            .is_some()
        {
            op_state.put(Rc::new(fetch::SubrequestCounter::new()));
        }
    }

    /// Invoke `__zfb.dispatch(...)` for `request` and return the
    /// resulting v8 Promise as a `Global<Value>`. The caller is
    /// responsible for awaiting / resolving it.
    fn invoke_dispatch_js(&mut self, request: &HttpRequestLike) -> Result<v8::Global<v8::Value>> {
        // We construct the call as a small JS expression rather than
        // wrestling with v8::Function::call from Rust — `serde_v8`'s
        // round trip on the input arguments is brittle when the body
        // is a `Uint8Array`, and the expression form is what
        // `deno_core` itself uses internally for similar plumbing.
        let url = request.url.clone();
        let method = request.method.clone();
        // Serialise headers + body as JSON literals embedded in the
        // expression.  Bodies are rare on the SSG path (GETs).
        //
        // Old approach: `Uint8Array.from([b0,b1,…])` — an O(N) numeric-array
        // literal that V8 must re-parse and re-evaluate at dispatch time.  For
        // a 64 KiB body that is ~200 000 characters of source to tokenise.
        //
        // New approach: encode the body as a base64 string in Rust, embed it
        // as a JSON string literal, then decode with a one-liner in the JS
        // expression.  The expression uses only `atob` (present in the host
        // shim's web-polyfills) and `TextEncoder`-free byte math — no extra
        // polyfill needed.  Encoding cost is O(N/3) string concatenation in
        // Rust (fast) vs O(N) number-format-and-join (slow for large bodies).
        let headers_literal = serde_json::to_string(&request.headers).map_err(|e| {
            RenderError::Runtime(format!("encoding request headers as JSON failed: {e}"))
        })?;
        let body_literal = match &request.body {
            None => "undefined".to_string(),
            Some(bytes) if bytes.is_empty() => "undefined".to_string(),
            Some(bytes) => {
                // base64-encode in Rust; decode in JS with atob + Uint8Array.
                // atob is part of the host shim's web-polyfills (web_polyfills.js).
                let b64 = base64_encode(bytes);
                let b64_json =
                    serde_json::to_string(&b64).expect("base64 string is always valid JSON");
                format!(
                    "(()=>{{const s=atob({b64_json});\
                      const u=new Uint8Array(s.length);\
                      for(let i=0;i<s.length;i++)u[i]=s.charCodeAt(i);\
                      return u;}})()"
                )
            }
        };
        let url_literal = serde_json::to_string(&url).map_err(|e| {
            RenderError::Runtime(format!("encoding request URL as JSON failed: {e}"))
        })?;
        let method_literal = serde_json::to_string(&method).map_err(|e| {
            RenderError::Runtime(format!("encoding request method as JSON failed: {e}"))
        })?;
        // Issue #2014: the 5th argument is the per-dispatch mode. The
        // shim sets `__zfb.mode` from it for the duration of the
        // dispatch and restores the previous value in a `finally`, so a
        // throwing request-time dispatch cannot leak request-time
        // capability into the next build-time render.
        let mode_literal = serde_json::to_string(request.mode.as_js_str())
            .expect("dispatch mode spelling is always valid JSON");
        // The 6th argument is this host's mode nonce — without it the
        // shim ignores `mode` and inherits instead (see `mode_nonce`).
        let nonce_literal =
            serde_json::to_string(&self.mode_nonce).expect("nonce is always valid JSON");
        let script = format!(
            "globalThis.__zfb.dispatch({url}, {method}, {headers}, {body}, {mode}, {nonce})",
            url = url_literal,
            method = method_literal,
            headers = headers_literal,
            body = body_literal,
            mode = mode_literal,
            nonce = nonce_literal,
        );
        let result = self
            .runtime
            .execute_script("zfb:dispatch", script)
            .map_err(|e| RenderError::Runtime(format_js_error(&e)))?;
        Ok(result)
    }

    /// Drain the worker console output buffered by the host shim's
    /// console capture (see `js/globals_shim.js`) since the last
    /// drain.
    ///
    /// Returns the buffered lines joined with `\n` — each line carries
    /// a `[level]` prefix (`[log]`, `[warn]`, …) — and clears the
    /// JS-side buffer. Returns an empty string when nothing was
    /// logged, when the shim is missing, or when the drain script
    /// itself fails: this is strictly best-effort diagnostics and
    /// must never mask the render error the caller is about to
    /// surface (issue #700).
    ///
    /// Re-entrancy rule: call only BETWEEN dispatches — after a render
    /// (or module evaluation) completes or fails — never while a
    /// dispatch is in flight on the isolate.
    pub fn drain_console_logs(&mut self) -> String {
        let result = match self.runtime.execute_script(
            "zfb:drain_console_logs",
            "globalThis.__zfb && typeof globalThis.__zfb.drainConsoleLogs === \"function\" \
                 ? globalThis.__zfb.drainConsoleLogs() : \"\"",
        ) {
            Ok(v) => v,
            Err(_) => return String::new(),
        };
        deno_core::scope!(scope, &mut self.runtime);
        let local = v8::Local::new(scope, result);
        local_to_string_if_string(scope, local).unwrap_or_default()
    }

    fn allocate_handle(&self, name: &str) -> ModuleHandle {
        let mut next = self.next_handle_id.borrow_mut();
        let id = *next;
        *next = next.checked_add(1).unwrap_or(1);
        ModuleHandle::new(id, name)
    }

    /// Lookup `(handle, module_id)` for an already-registered module.
    fn module_id_for(&self, handle: &ModuleHandle) -> Option<ModuleId> {
        self.handles.borrow().get(&handle.name).map(|(_, id)| *id)
    }

    /// Shared load + evaluate logic used by both the tolerant
    /// [`RenderHost::execute_module`] trait method and the strict
    /// [`Self::execute_worker_module`] entrypoint. Registers `source`
    /// as the main ESM module under `name`, evaluates it (awaiting
    /// top-level await), and returns the resulting handle plus the
    /// underlying `ModuleId`. Deliberately does NOT call
    /// `install_bundle_default` — callers decide how strictly to
    /// enforce the workerd-shape contract.
    async fn load_and_evaluate_main_module(
        &mut self,
        name: &str,
        source: &str,
    ) -> Result<(ModuleHandle, ModuleId)> {
        // Module top-level code runs outside any dispatch, so it gets
        // the denying default however the previous dispatch left things.
        self.reset_dispatch_mode_for_evaluation();
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
        let handle = self.allocate_handle(name);
        self.handles
            .borrow_mut()
            .insert(handle.name.clone(), (handle.clone(), module_id));
        Ok((handle, module_id))
    }

    /// Strict main-worker load entrypoint (sub-issue #1764). Loads
    /// `source` as the main ESM module under `name` and REQUIRES it to
    /// satisfy the workerd-shape contract — a `default` export that is
    /// an object with a callable `fetch` — failing loudly with the
    /// ORIGINAL [`install_bundle_default`] diagnostic when it doesn't.
    ///
    /// This differs from the tolerant [`RenderHost::execute_module`]
    /// trait method, which callers also use to load utility modules
    /// that carry no `default` export at all; that path swallows a
    /// bad/missing `default` into `last_install_error` so
    /// `dispatch_fetch` fails later, at first request, with a generic
    /// "no bundle loaded" message. Production main-worker boot sites
    /// (`zfb`'s `v8_host_adapter.rs`: the normal bundle boot and the
    /// dev content-trace wrapper boot) call this instead so a
    /// malformed worker fails at startup with the precise reason.
    pub async fn execute_worker_module(
        &mut self,
        name: &str,
        source: &str,
    ) -> Result<ModuleHandle> {
        let (handle, module_id) = self.load_and_evaluate_main_module(name, source).await?;
        self.install_bundle_default(module_id, name)?;
        // Mirrors the tolerant path's success bookkeeping: clear any
        // stale failure recorded from an earlier module.
        *self.last_install_error.borrow_mut() = None;
        Ok(handle)
    }

    /// Load `source` under `name` as a SIDE module (`load_side_es_module`)
    /// rather than the runtime's main module. `deno_core`'s module map
    /// caches modules by resolved specifier, so a later `import` of the
    /// same specifier from a main module (e.g. a wrapper module) resolves
    /// to this already-evaluated instance instead of re-fetching or
    /// re-executing it — this is the documented purpose of
    /// `load_side_es_module` ("utility code that might be later imported
    /// by the main module").
    async fn load_and_evaluate_side_module(
        &mut self,
        name: &str,
        source: &str,
    ) -> Result<ModuleId> {
        // Same reason as `load_and_evaluate_main_module`: evaluation is
        // not a dispatch, so it evaluates at build-time capability.
        self.reset_dispatch_mode_for_evaluation();
        let specifier = synthesise_specifier(name);
        self.loader.register_module(specifier.as_str(), source);
        let module_specifier = ModuleSpecifier::parse(specifier.as_str()).map_err(|e| {
            RenderError::Runtime(format!(
                "embedded V8 host: bad synthetic specifier `{specifier}`: {e}"
            ))
        })?;
        let module_id = self
            .runtime
            .load_side_es_module(&module_specifier)
            .await
            .map_err(|e| RenderError::Runtime(format!("load_side_es_module failed: {e}")))?;
        let evaluate = self.runtime.mod_evaluate(module_id);
        self.runtime
            .run_event_loop(PollEventLoopOptions::default())
            .await
            .map_err(|e| RenderError::Runtime(format_event_loop_error(&e)))?;
        evaluate
            .await
            .map_err(|e| RenderError::Runtime(format!("module evaluation failed: {e}")))?;
        Ok(module_id)
    }

    /// Strictly validate that `source` — loaded as a SIDE module under
    /// `name` — satisfies the workerd-shape contract, WITHOUT installing
    /// it as the `dispatch_fetch` target (sub-issue #1764 follow-up).
    ///
    /// Exists for the dev content-trace boot seam: the generated wrapper
    /// module's own `default` export is always well-shaped and only
    /// touches the INNER worker's `default.fetch` lazily, at dispatch
    /// time. Validating just the wrapper (via [`Self::execute_worker_module`])
    /// would let a malformed inner worker pass boot and only fail on the
    /// first real request. Callers load-and-validate the inner worker
    /// with this method FIRST (registering it under the exact specifier
    /// the wrapper's `import` statement uses — pass a `file://` specifier
    /// as `name` to reuse it verbatim, see [`synthesise_specifier`]), then
    /// run the wrapper through `execute_worker_module` as the main module;
    /// the wrapper's `import` resolves to this already-evaluated instance
    /// rather than re-executing it.
    pub async fn validate_worker_module_shape(&mut self, name: &str, source: &str) -> Result<()> {
        let module_id = self.load_and_evaluate_side_module(name, source).await?;
        self.validate_bundle_default_shape_only(module_id, name)
    }
}

#[async_trait(?Send)]
impl RenderHost for EmbeddedV8RenderHost {
    async fn execute_module(&mut self, name: &str, source: &str) -> Result<ModuleHandle> {
        let (handle, module_id) = self.load_and_evaluate_main_module(name, source).await?;
        // Wire the bundle's default export into the host shim so
        // dispatch_fetch can find it. This is the workerd-shape
        // contract; if the module isn't shaped that way the call
        // here surfaces a clear error.
        match self.install_bundle_default(module_id, name) {
            Ok(()) => {
                // Clear any failure recorded from an earlier module so
                // a stale error does not pollute later dispatch_fetch
                // diagnostics.  (The field is only re-read when
                // bundle_installed is false, so clearing it here is
                // defensive — once a good bundle lands the flag is
                // monotonically true and the field won't be read again.)
                *self.last_install_error.borrow_mut() = None;
            }
            Err(e) => {
                // Some callers (the existing render-orchestrator path)
                // load utility modules that don't carry a `default`
                // export. We tolerate that *non-fatally* for
                // execute_module compatibility — dispatch_fetch will
                // still error if called before a shaped bundle is
                // registered.
                *self.last_install_error.borrow_mut() = Some(e.to_string());
                tracing_warn(&format!(
                    "embedded V8 host: bundle `{name}` not workerd-shaped ({e}); dispatch_fetch disabled until a shaped module loads"
                ));
            }
        }
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
            let default_val = local_ns
                .get(scope, key.into())
                .ok_or_else(|| RenderError::MissingDefaultExport(handle.name.clone()))?;
            if !default_val.is_function() {
                return Err(RenderError::MissingDefaultExport(handle.name.clone()));
            }
            let func: v8::Local<v8::Function> = default_val
                .try_into()
                .map_err(|_| RenderError::MissingDefaultExport(handle.name.clone()))?;
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
            let result =
                result.ok_or_else(|| RenderError::Runtime("default() returned no value".into()))?;
            v8::Global::new(tc, result)
        };
        // If the result was a Promise, await it; else short-circuit.
        // Use `with_event_loop_promise` so microtasks queued by the
        // promise's chain are actually drained.
        let resolve_future = self.runtime.resolve(promise);
        let resolved = self
            .runtime
            .with_event_loop_promise(Box::pin(resolve_future), PollEventLoopOptions::default())
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
fn local_to_string_if_string(isolate: &v8::Isolate, local: v8::Local<v8::Value>) -> Option<String> {
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

/// Build the deno_core extension list.
///
/// Request / Response / Headers / URL live in the JS polyfill
/// (`extensions::WEB_POLYFILLS_SRC`) rather than `deno_fetch` /
/// `deno_web` — see `Cargo.toml`'s comment "Why a polyfill instead of
/// deno_fetch/deno_web" for the trade-off (heavy compile + tricky
/// lazy-load bootstrap vs. ~250 lines of hermetic JS).
///
/// What is NOT expressible in JS is the outbound socket, so
/// [`fetch::zfb_fetch`] registers the one Rust op that owns it
/// (issue #2015). It is an **async** op — the isolate thread must
/// never park on a network read. Nothing in `web_polyfills.js` calls
/// it yet; sub-issue #2016 wires the JS side.
///
/// The OS CSPRNG is the other thing JS cannot reach, so
/// [`crypto::zfb_crypto`] registers [`crypto::op_zfb_random_bytes`]
/// (issue #2017) alongside [`crypto::digest::op_zfb_digest`] (issue
/// #2018). Both are **synchronous** on purpose —
/// `crypto.getRandomValues` is synchronous by specification, the kernel
/// entropy syscall is neither network nor disk I/O, and hashing is
/// CPU-bound over a buffer already in memory; see the `crypto` module
/// header. They are registered unconditionally, in both dispatch modes:
/// unlike `fetch`, neither entropy nor hashing is mode-gated — the SSG
/// denial is about network access.
///
/// Kept as a function so a future swap to `deno_web` / `deno_fetch`
/// is a one-place change.
fn build_extensions() -> Vec<deno_core::Extension> {
    vec![fetch::zfb_fetch::init(), crypto::zfb_crypto::init()]
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
/// `headers` is an ordered pair list (mirroring the JS-side
/// `[name, value]` array the host shim's `dispatch()` builds from
/// `resp.headers.entries()`) rather than a map — a map would collapse
/// duplicate names such as multiple `set-cookie` values down to one.
///
/// `body` is materialised as a `serde_v8::JsBuffer` so the
/// `Uint8Array` from JS round-trips without a Vec<u8> coercion. We
/// then copy out into `Vec<u8>` for the caller-facing
/// [`HttpResponseLike`] (the JsBuffer holds onto a v8 backing store
/// reference; copying decouples the response from the v8 lifetime).
#[derive(Deserialize)]
struct DispatchResult {
    status: u16,
    headers: Vec<(String, String)>,
    body: deno_core::JsBuffer,
}

/// V8-boundary tests for the request-time fetch transport (issue #2015).
///
/// These live here, not in `tests/`, because the properties under test
/// need the host's private `runtime` handle and its `OpState`. The
/// transport's own 18-case behaviour matrix lives in
/// `embedded_v8/fetch/tests.rs`; what is proved here is the part only a
/// real isolate can show:
///
/// - the op really is driven by `deno_core`'s event loop rather than
///   parking the isolate thread (guardrail 1), and
/// - a host-op rejection surfaces to JS as a **rejected promise**, never
///   as a resolved synthetic empty response.
#[cfg(test)]
mod fetch_boundary_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::io::AsyncWriteExt;

    use super::*;
    use crate::embedded_v8::loopback_test_server::{ok_response, LoopbackServer};

    /// Everything in this module is bounded, so a regression that makes
    /// the op block reports a clear failure instead of hanging the test
    /// binary until nextest's `terminate-after`.
    const BOUND: Duration = Duration::from_secs(30);

    /// A server that answers **only once two requests are in flight at
    /// the same time**.
    ///
    /// This is the falsifier for guardrail 1: if `op_zfb_fetch` parked
    /// the isolate thread, the second `op_zfb_fetch(...)` call in
    /// `Promise.all([...])` would never be reached, the barrier would
    /// never release, and the dispatch would never resolve.
    async fn concurrency_barrier_server() -> LoopbackServer {
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        LoopbackServer::spawn(move |req, mut stream| {
            let barrier = barrier.clone();
            async move {
                barrier.wait().await;
                let _ = stream.write_all(&ok_response(&req.target)).await;
                let _ = stream.shutdown().await;
            }
        })
        .await
    }

    /// Run `script` in the host, await the promise it evaluates to, and
    /// return it stringified.
    async fn eval_await_string(host: &mut EmbeddedV8RenderHost, script: String) -> String {
        let promise = host
            .runtime
            .execute_script("zfb:fetch_boundary_test", script)
            .expect("test script evaluates");
        let resolve = host.runtime.resolve(promise);
        let resolved = host
            .runtime
            .with_event_loop_promise(Box::pin(resolve), PollEventLoopOptions::default())
            .await
            .expect("the test promise settles");
        deno_core::scope!(scope, &mut host.runtime);
        let local = v8::Local::new(scope, resolved);
        local.to_rust_string_lossy(scope)
    }

    #[tokio::test]
    async fn the_op_runs_on_the_event_loop_and_does_not_park_the_isolate_thread() {
        let server = concurrency_barrier_server().await;
        let mut host = EmbeddedV8RenderHost::new().expect("host boot");
        // The op refuses a build-time caller outright (epic #2012
        // review fix 1), and this test is about the op's SCHEDULING,
        // not the policy. Establish the precondition a real
        // request-time dispatch would have established; the denial
        // itself is asserted by
        // `a_build_time_dispatch_cannot_reach_the_network_through_the_raw_op`.
        host.install_dispatch_mode(DispatchMode::RequestTime);

        let script = format!(
            r#"
            (async () => {{
              const call = (path) => Deno.core.ops.op_zfb_fetch(
                {{ url: {base:?} + path, method: "GET", headers: [], redirect: "follow", hasBody: false }},
                new Uint8Array(0),
              );
              // Both ops are invoked before either is awaited. The
              // server will not answer until BOTH connections exist, so
              // this can only settle if the first call handed control
              // back to the event loop instead of blocking.
              const results = await Promise.all([call("/one"), call("/two")]);
              const decoder = new TextDecoder();
              return results
                .map((r) => r.status + ":" + decoder.decode(r.body))
                .join("|");
            }})()
            "#,
            base = server.base_url(),
        );

        let got = tokio::time::timeout(BOUND, eval_await_string(&mut host, script))
            .await
            .expect(
                "two concurrent op_zfb_fetch calls never both reached the server within 30s — \
                 the op is blocking the isolate thread",
            );

        assert_eq!(got, "200:/one|200:/two");
        assert_eq!(server.request_count(), 2);
    }

    #[tokio::test]
    async fn a_transport_rejection_reaches_js_as_a_rejected_promise() {
        // Never a synthetic empty `Response`: a silent empty body is
        // indistinguishable from a real 200 with no content, which is
        // exactly the dev/prod divergence epic #2012 exists to remove.
        let mut host = EmbeddedV8RenderHost::new().expect("host boot");
        // Same precondition as above: what is under test is that a
        // TRANSPORT rejection crosses the boundary as a rejected
        // promise, which is unreachable while the build-time denial
        // fires first.
        host.install_dispatch_mode(DispatchMode::RequestTime);
        let script = r#"
            Deno.core.ops.op_zfb_fetch(
              { url: "ftp://example.invalid/data", method: "GET", headers: [], redirect: "follow", hasBody: false },
              new Uint8Array(0),
            ).then(
              (r) => "RESOLVED:" + r.status,
              (e) => "REJECTED:" + e.name + ":" + e.message,
            )
        "#
        .to_string();

        let got = tokio::time::timeout(BOUND, eval_await_string(&mut host, script))
            .await
            .expect("the rejection settles");

        assert_eq!(
            got,
            "REJECTED:TypeError:Fetch API cannot load: ftp://example.invalid/data"
        );
    }

    #[test]
    fn the_extension_installs_the_client_and_the_subrequest_counter() {
        let host = EmbeddedV8RenderHost::new().expect("host boot");
        let op_state = host.runtime.op_state();
        let op_state = op_state.borrow();
        assert!(
            op_state.try_borrow::<fetch::FetchClient>().is_some(),
            "build_extensions() must install the shared outbound client"
        );
        assert!(
            op_state.try_borrow::<Rc<fetch::CancelRegistry>>().is_some(),
            "build_extensions() must install the fetch cancellation registry"
        );
        assert_eq!(
            op_state
                .try_borrow::<fetch::DispatchModeState>()
                .map(|m| m.0),
            Some(DispatchMode::BuildTime),
            "a freshly booted host must sit at the DENYING default until a dispatch says \
             otherwise — this cell, not `__zfb.mode`, is what `op_zfb_fetch` reads"
        );
        assert!(
            op_state
                .try_borrow::<Rc<fetch::SubrequestCounter>>()
                .is_some(),
            "build_extensions() must install the per-dispatch subrequest counter"
        );
    }

    #[tokio::test]
    async fn dispatch_fetch_opens_a_fresh_budget_and_leaves_the_previous_one_alone() {
        let mut host = EmbeddedV8RenderHost::new().expect("host boot");
        host.execute_module(
            "bundle.mjs",
            "export default { async fetch() { return new Response(\"ok\"); } };",
        )
        .await
        .expect("execute the probe bundle");

        let read_counter = |host: &EmbeddedV8RenderHost| {
            let op_state = host.runtime.op_state();
            let op_state = op_state.borrow();
            op_state.borrow::<Rc<fetch::SubrequestCounter>>().clone()
        };

        // Spend the budget as the previous dispatch would have. An op
        // that outlived that dispatch would still be holding this `Rc`.
        let previous = read_counter(&host);
        for _ in 0..5 {
            previous
                .claim("http://zfb.local/", 50)
                .expect("within budget");
        }
        assert_eq!(previous.used(), 5);

        let response = host
            .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
            .await
            .expect("dispatch");
        assert_eq!(response.status, 200);

        let current = read_counter(&host);
        assert!(
            !Rc::ptr_eq(&previous, &current),
            "each dispatch must get its OWN counter — zeroing the shared one in \
             place would let a fetch orphaned by the previous dispatch spend this \
             dispatch's budget"
        );
        assert_eq!(current.used(), 0, "the new dispatch starts at zero");
        assert_eq!(
            previous.used(),
            5,
            "and the orphan keeps charging the dispatch it belongs to, rather than \
             having its overspend forgiven"
        );
    }
}
