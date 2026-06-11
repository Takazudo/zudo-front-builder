//! Render-on-request hook seam for `zfb-server` (issue #1020).
//!
//! Mirrors the [`crate::ssr`] pattern: the bin crate owns the actual
//! renderer (a `DevRenderSession`) and implements [`RenderOnRequestHook`]
//! there; the server holds a live handle and knows nothing beyond the
//! trait boundary.
//!
//! ## Purpose
//!
//! During `zfb dev` the build pipeline writes rendered HTML to `html_root`
//! on every file-system watcher tick. A page request that arrives while
//! the watcher hasn't fired yet (or before the first build finishes) would
//! serve a stale or absent file. The render-on-request hook lets the dev
//! router trigger a targeted render for the exact URL being requested, so
//! `html_root` is fresh before `serve_page` falls through to its
//! `read_from_dist` leg.
//!
//! ## Precedence
//!
//! Within `serve_page` the hook fires **after** the 405 gate (GET/HEAD
//! only) and **before** the in-memory [`crate::routes::PageCache`] lookup.
//! The hook is a side-effect call — it has no return value visible to the
//! server; after `await` the server resumes the normal `PageCache →
//! html_root → public` waterfall unchanged.
//!
//! Full precedence chain (highest → lowest):
//!
//! 1. plugin dev-middleware,
//! 2. embed handlers,
//! 3. request-time SSR (`prerender = false` — this module is a separate
//!    concern),
//! 4. **render-on-request hook** ← this module (Dev + GET/HEAD only),
//! 5. in-memory page cache,
//! 6. `html_root` on-disk read,
//! 7. `public_root` on-disk read,
//! 8. dev 404.
//!
//! ## Threading model
//!
//! Mirrors `SsrDispatcher`: the concrete implementation in the bin crate
//! parks the renderer on a dedicated thread and routes calls through a
//! channel. The server only holds an `Arc<dyn RenderOnRequestHook>` behind
//! an `Arc<RwLock<Option<…>>>` and is agnostic to that threading detail.
//!
//! The live handle uses the same short-read-lock discipline as the SSR
//! routes handle: snapshot the `Arc` under a short read lock, drop the
//! lock, then `await` the hook — so a concurrent writer (e.g. a session
//! reload) is never blocked by an in-flight render.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;

/// Live handle threaded through [`crate::ServeOpts`] and
/// [`crate::routes::AppState`].
///
/// `None` (outer) means "no render-on-request hook configured" — all
/// existing legs are byte-identical. `Some` wraps the actual hook
/// behind an `Arc<RwLock<Option<Arc<dyn RenderOnRequestHook>>>>` so
/// the bin crate can swap in a new session after a hot-reload without
/// restarting the dev server.
///
/// Mirrors [`crate::ssr::SsrRoutesHandle`].
pub type RenderOnRequestHandle = Arc<RwLock<Option<Arc<dyn RenderOnRequestHook>>>>;

/// Trait implemented by the bin crate's renderer adapter.
///
/// The dev router holds an `Arc<dyn RenderOnRequestHook>` and calls
/// `render_if_stale` before the page-cache / disk-read legs. The
/// implementor is responsible for:
///
/// - deciding whether `html_root/<url_path>/index.html` (or equivalent)
///   is stale or absent,
/// - triggering a render and writing the output to `html_root` if needed,
/// - logging any errors internally — the server treats the call as
///   best-effort and falls through to the existing disk/cache legs
///   regardless of whether a render actually happened.
///
/// ## `Send + Sync` requirement
///
/// The dev server runs on a multi-threaded tokio runtime. The hook must
/// be safe to share across handler tasks. Concrete implementations wrap
/// the underlying renderer behind a channel (see
/// `crates/zfb/src/v8_host_adapter.rs` for the same pattern) so the
/// trait object itself is trivially `Send + Sync`.
///
/// ## Error handling contract
///
/// `render_if_stale` has no return value. Errors (renderer crash, I/O
/// failure, stale detection logic) are the implementor's responsibility.
/// The server does **not** surface hook errors as HTTP error responses —
/// it simply falls through to the next leg. This matches the spirit of
/// the hook: "make the disk fresh as a best effort; serve whatever is
/// there."
#[async_trait]
pub trait RenderOnRequestHook: Send + Sync {
    /// Optionally trigger a render for `url_path`.
    ///
    /// `url_path` is the request path without query string, e.g.
    /// `/blog/hello` or `/`. The implementor may use this to decide
    /// which page file to render. It does NOT include the `base` prefix
    /// — the server strips the mount prefix before calling so the
    /// implementor sees the same shape the production adapter sees.
    ///
    /// The implementor should write the result to the `html_root`
    /// directory the server was configured with. After this call returns,
    /// `serve_page` will attempt to read from `html_root` as usual.
    async fn render_if_stale(&self, url_path: &str);
}
