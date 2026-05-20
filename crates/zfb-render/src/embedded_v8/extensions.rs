//! Synthetic ESM source for the `node:*` stubs and the host-bridge
//! globals shim.
//!
//! Design rationale: user bundles
//! produced by arbitrary developer code may include top-level
//! `import "node:fs"` (or `node:path`, etc.) for branches that only
//! fire under Workers / production SSR. Those imports must
//! *resolve at module-load time* (so the bundle evaluates at all)
//! but *throw at call time* with a recognisable error. This module
//! ships the JS sources that satisfy that contract.
//!
//! The host's [`super::module_loader::BundleModuleLoader`] is the
//! canonical resolution point for `node:*` and the bridge globals —
//! it serves the JS source out of the constants below. We deliberately
//! do NOT register them as a `deno_core::Extension` because the
//! `extension!` macro's path-relative file inclusion is brittle across
//! `cargo` invocation contexts (workspace root vs crate dir vs test
//! harness vs `cargo install` from a tarball). Inlining via `include_str!`
//! against the crate source dir keeps the build hermetic.
//!
//! ## v1 list
//!
//! - `node:fs`
//! - `node:fs/promises`
//! - `node:path`
//! - `node:url`
//! - `node:buffer`
//! - `node:async_hooks` (added: zudo-doc consumer uses @takazudo/zfb-adapter-cloudflare
//!   which imports AsyncLocalStorage from node:async_hooks at the top level)
//!
//! Wave 3 expansion is allowed — add a specifier here when real
//! user code surfaces a gap.

/// Shared sentinel utility. Imported by every per-specifier stub.
///
/// The named sentinel `__zfbThrowNodeStub` and the exact error
/// message string (`"node:* is not available under the SSG runtime"`)
/// are part of the host's contract — search the codebase for either
/// before renaming.
pub const NODE_STUB_HELPER_SRC: &str = include_str!("js/node_stub.js");

/// `node:fs` stub source. Throws on every member access / call.
pub const NODE_FS_SRC: &str = include_str!("js/node_fs.js");

/// `node:fs/promises` stub source.
pub const NODE_FS_PROMISES_SRC: &str = include_str!("js/node_fs_promises.js");

/// `node:path` stub source.
pub const NODE_PATH_SRC: &str = include_str!("js/node_path.js");

/// `node:url` stub source. (Distinct from the global `URL`, which
/// `deno_web` provides; this is for code paths that import the Node
/// namespace by name.)
pub const NODE_URL_SRC: &str = include_str!("js/node_url.js");

/// `node:buffer` stub source.
pub const NODE_BUFFER_SRC: &str = include_str!("js/node_buffer.js");

/// `node:async_hooks` stub source. Provides throwing-proxy stubs for
/// `AsyncLocalStorage`, `AsyncResource`, and other exports. Needed so
/// consumer bundles that import `@takazudo/zfb-adapter-cloudflare` (which
/// does `import { AsyncLocalStorage } from "node:async_hooks"` at the top
/// level) can evaluate during the SSG paths() step without crashing at
/// module-load time.
pub const NODE_ASYNC_HOOKS_SRC: &str = include_str!("js/node_async_hooks.js");

/// Host-bridge JS source. Installs `globalThis.__zfb` with
/// `setBundle(defaultExport)` and
/// `dispatch(urlStr, method, headersObj, bodyU8)`. Executed once at
/// host boot via `execute_script` (NOT as a module — it sets a global
/// rather than exporting bindings).
pub const HOST_GLOBALS_SHIM_SRC: &str = include_str!("js/globals_shim.js");

/// Web Platform API polyfills for the embedded V8 host: `Request`,
/// `Response`, `Headers`, `URL`, `URLSearchParams`, `fetch`,
/// `TextEncoder`, `TextDecoder`, `atob`, `btoa`, `structuredClone`,
/// and a minimal `crypto`. Installed BEFORE the host-globals shim so
/// the bundle's bootstrap code (which may probe `typeof Request`)
/// sees the constructors. See `js/web_polyfills.js` header for the
/// scope and intentional gaps.
pub const WEB_POLYFILLS_SRC: &str = include_str!("js/web_polyfills.js");

/// Browser-Event globals: `Event`, `CustomEvent`, `EventTarget`.
/// Needed so consumer bundles whose top-level code declares
/// `class X extends Event` (e.g. `@takazudo/zfb-runtime`'s
/// client-router events module) can be loaded by the embedded V8
/// host. See `js/browser_event.js` for the rationale and scope.
pub const BROWSER_EVENT_SRC: &str = include_str!("js/browser_event.js");

/// The v1 set of specifiers the `node:*` stub recognises. Returned in
/// stable order for diagnostics. Helpers / tests use this list to
/// drive parameterised assertions.
pub const NODE_STUB_SPECIFIERS: &[&str] = &[
    "node:fs",
    "node:fs/promises",
    "node:path",
    "node:url",
    "node:buffer",
    "node:async_hooks",
];

/// Lookup the synthetic source for a `node:*` specifier. Returns
/// `None` for any specifier outside [`NODE_STUB_SPECIFIERS`] — the
/// loader treats that as "not a node stub, fall through".
pub fn node_stub_source(specifier: &str) -> Option<&'static str> {
    match specifier {
        "node:fs" => Some(NODE_FS_SRC),
        "node:fs/promises" => Some(NODE_FS_PROMISES_SRC),
        "node:path" => Some(NODE_PATH_SRC),
        "node:url" => Some(NODE_URL_SRC),
        "node:buffer" => Some(NODE_BUFFER_SRC),
        "node:async_hooks" => Some(NODE_ASYNC_HOOKS_SRC),
        _ => None,
    }
}

/// Resolve the `ext:zfb_node_stubs/<file>` specifiers used inside the
/// stub modules' `import` statements (each per-specifier stub imports
/// the shared helper from `ext:zfb_node_stubs/node_stub.js`). Returns
/// `None` for any specifier outside the helper set.
pub fn ext_node_stubs_source(specifier: &str) -> Option<&'static str> {
    match specifier {
        "ext:zfb_node_stubs/node_stub.js" => Some(NODE_STUB_HELPER_SRC),
        _ => None,
    }
}
