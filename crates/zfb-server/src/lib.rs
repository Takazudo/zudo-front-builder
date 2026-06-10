//! `zfb-server` — the dev-mode HTTP server for `zudo-front-builder`.
//!
//! This crate runs an [`axum`] server that serves the in-memory
//! page-cache HTML produced by [`zfb_build`]'s rebuild loop, the built
//! `dist/assets/` tree, and per-request on-disk fallbacks for the
//! project's `dist/` and `public/` directories so static files in
//! `public/` are reachable at the site root (e.g.
//! `public/logo.svg` → `/logo.svg`). Every served HTML
//! response has a small `<script src="/__zfb/livereload.js"></script>`
//! injected before `</body>`. That script opens an SSE connection to
//! `/__zfb/reload` and listens for two event types:
//!
//! - `page` — the browser does a full `location.reload()`.
//! - `css` — the browser bumps the query-string on every
//!   `<link rel="stylesheet">` to swap CSS without losing client-side
//!   state.
//!
//! ## How it plugs into [`zfb_build`]
//!
//! The bin crate that runs `zfb dev` owns:
//!
//! - a [`zfb_build::BuildOrchestrator`] (the rebuild loop),
//! - a [`tokio::sync::broadcast`] channel of [`livereload::ReloadEvent`]s,
//! - a [`routes::PageCache`] of rendered HTML keyed by URL path,
//! - this crate's [`serve`] task.
//!
//! The bin crate wires the orchestrator's `on_outcome` callback so that
//! every non-noop build tick is translated into [`ReloadEvent`]s via
//! [`livereload::outcome_to_events`] and fed into the broadcast
//! channel. The bin crate is also responsible for populating the
//! [`routes::PageCache`] from the orchestrator's render outputs — the
//! server itself only **reads** the cache.
//!
//! See the module docs of [`livereload`] for the full wiring snippet.
//!
//! ## Production caveat
//!
//! This crate is **dev-only**. It always injects the live-reload
//! script, hard-codes `Cache-Control: no-store` on HTML, and exposes a
//! `/__zfb/*` namespace. Production builds emit static files served by
//! a different runtime (Cloudflare Workers, an edge CDN, …) and must
//! not pull in `zfb-server`.

pub mod assets_containment;
pub mod embed;
pub mod embed_handlers;
pub mod host_validation;
pub mod injected_routes;
pub mod inject;
pub mod livereload;
pub mod middleware;
pub mod plugin_middleware;
pub mod routes;
pub mod ssr;

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::Context;
use tokio::net::TcpListener;
use tracing::info;

/// Shared handle to the currently-emitted dev-mode islands bundle URL
/// (issue #377).
///
/// `None` (outer) means "no islands path configured for this server"
/// — the page handler skips head injection entirely. `Some(url)`
/// inside the lock carries the public URL the dev orchestrator wrote
/// last (`/assets/islands.js` for projects without a base prefix, or
/// `/foo/assets/islands.js` when `base: "/foo/"` is configured).
///
/// Reads happen on every served HTML response in dev mode; writes happen
/// once at boot and again on every islands-rebuild tick. The contention
/// is one writer thread vs many short-lived readers — [`RwLock`] is
/// the right shape (not a plain `Mutex`).
///
/// Cloning the [`Arc`] is cheap; the [`AppState`] holds one clone and the
/// bin crate's `run_islands` callback holds another.
pub type IslandsBundleUrl = Arc<RwLock<Option<String>>>;

/// Shared handle to the currently-emitted dev-mode CSS bundle URL
/// (issue #494 / #498).
///
/// Mirrors [`IslandsBundleUrl`] for CSS. `None` (outer) means "no CSS
/// path configured for this server" — the page handler skips `<link>`
/// injection entirely. `Some(url)` inside the lock carries the public
/// URL the dev orchestrator wrote last (`/assets/styles.css` for
/// projects without a base prefix, or `/foo/assets/styles.css` when
/// `base: "/foo/"` is configured).
///
/// Reads happen on every served HTML response in dev mode; writes happen
/// once at boot and again on every CSS-rebuild tick. The contention
/// is one writer thread vs many short-lived readers — [`RwLock`] is
/// the right shape (not a plain `Mutex`).
pub type CssBundleUrl = Arc<RwLock<Option<String>>>;

pub use embed::{Server, ServerBuilder, ServerHandle, ServerMode};
pub use embed_handlers::{
    EmbedHandler, EmbedHandlerFn, EmbedHandlerFuture, EmbedHandlerSet, RouteParams,
};
pub use host_validation::{apply_host_validation_layer, HostValidation};
pub use inject::{inject_livereload, inject_livereload_into_tree, LIVERELOAD_TAG};
pub use injected_routes::{pattern_matches, InjectedRouteSet};
pub use zfb_build::InjectedRoute;
pub use livereload::{outcome_to_events, IslandsBundleInfo, ReloadEvent, ReloadTx};
pub use plugin_middleware::{
    path_matches_prefix, DevMiddlewareDispatcher, DevMiddlewareSet, PluginDispatchError,
    PluginDispatchOutcome, PluginRegistration, PluginRequest, PluginResponse,
    PluginResponseEncoding,
};
pub use routes::{
    build_router, content_type_for_extension, resolve_content_type, AppState, CachedPage,
    PageCache, DEV_404_BODY,
};
pub use ssr::{
    SsrDispatchError, SsrDispatcher, SsrRequest, SsrResponse, SsrRouteRecord, SsrRouteSet,
    SsrRoutesHandle,
};

/// Options for [`serve`].
///
/// All paths must be absolute. The bin crate is expected to canonicalise
/// `project_root`, `dist_root`, and `public_root` before constructing
/// this struct so static-file serving is independent of the working
/// directory the server is launched from.
#[derive(Clone)]
pub struct ServeOpts {
    /// Project root (the directory `zfb dev` was invoked in). Stored
    /// here for diagnostics and for future use by middleware that
    /// wants to display "served from <project_root>" banners.
    pub project_root: PathBuf,

    /// Build output directory. `/assets/*` is served from
    /// `<dist_root>/assets/`.
    pub dist_root: PathBuf,

    /// Page (HTML) on-disk root used as the page-cache fallback inside
    /// the page handler. Issue #534: this used to alias `dist_root` for
    /// every caller, but in `zfb dev` we must serve dev-rendered HTML
    /// from a directory that the build pipeline does NOT touch (and
    /// vice-versa). Embed and preview-style callers pass the same value
    /// as `dist_root`; `zfb dev` passes its dev-only HTML root
    /// (`<project_root>/.zfb-build/dev-pages/`) so dev's per-route
    /// renders never overwrite the production output `pnpm preview`
    /// later serves.
    pub html_root: PathBuf,

    /// Project public-static directory. Files here are served at the
    /// site root via a per-request on-disk fallback inside the page
    /// handler — `public/logo.svg` → `GET /logo.svg`. The same shape
    /// `zfb build` produces (it copies `public/*` straight into
    /// `dist/`), so dev and prod URLs match.
    pub public_root: PathBuf,

    /// Address to bind. Defaults to `127.0.0.1:3000`.
    pub addr: SocketAddr,

    /// Page cache populated by the bin crate's render loop.
    pub pages: PageCache,

    /// Broadcast sender feeding the SSE live-reload channel. The bin
    /// crate sends [`ReloadEvent`]s into this from
    /// [`zfb_build::BuildOrchestrator::run`]'s `on_outcome` callback.
    pub broadcast: ReloadTx,

    /// Dev-middleware registrations from user plugins (Sub 3 / #108).
    /// `None` = no plugins declared a `devMiddleware` hook; the dev
    /// router skips the plugin-dispatch leg entirely. The bin crate
    /// (`zfb dev`) builds this from the long-lived plugin host.
    pub plugins: Option<DevMiddlewareSet>,

    /// Injected synthetic routes from user plugins' new `setup` hook
    /// (#255). `None` = no plugin called `injectRoute`. The dev
    /// router checks this set on every page-cache miss so the
    /// matched entrypoint is visible to follow-up renderer work
    /// without re-plumbing.
    pub injected_routes: Option<InjectedRouteSet>,

    /// Request-time SSR routes (issue #367 / Gap 1). `None` = the
    /// project has no `prerender = false` pages. When `Some`, the
    /// dev router checks every page-cache miss against this set —
    /// a hit dispatches through the V8 host and returns the rendered
    /// HTML at request time, matching the Cloudflare adapter's
    /// production semantics. See [`crate::ssr`] for the wire shape
    /// and precedence contract.
    ///
    /// The handle is an `Arc<RwLock<Option<SsrRouteSet>>>` (issue #807) so
    /// the per-tick renderer reload can swap in a fresh route set — adding
    /// or removing `prerender = false` routes mid-session becomes visible
    /// to the request dispatcher without a dev-server restart.
    pub ssr_routes: Option<crate::ssr::SsrRoutesHandle>,

    /// User-supplied `base` config value from `zfb.config.ts` (issue
    /// #229). Passed through verbatim — the dev server normalises it
    /// internally via [`zfb_types::dev_mount_prefix`] into the
    /// canonical `Some("/foo")` / `None` mount-prefix shape.
    ///
    /// When this resolves to a `Some(prefix)` the entire route table
    /// (page cache, assets, public, livereload, plugin middleware)
    /// mounts under `<prefix>/...`; bare `/` redirects to `<prefix>/`,
    /// other unprefixed paths fall through to a 404 with a hint.
    /// When this resolves to `None` (i.e. `base` is `None`, `""`,
    /// `"/"`, or an absolute URL) the route table is identical to the
    /// pre-`base` server byte-for-byte.
    pub base: Option<String>,

    /// Mirror of `zfb.config.ts`'s `trailingSlash` field. Threaded
    /// through to [`crate::routes::AppState::trailing_slash`] so the
    /// in-flight base-rewrite pass appends `/` to extensionless
    /// absolute hrefs the same way the production build does
    /// (sub #234 / zudolab/zudo-doc#1579).
    pub trailing_slash: bool,

    /// Server mode (Dev/Preview/Embed). Threaded through to
    /// [`crate::routes::AppState::mode`] so the router can gate Dev-only
    /// surface — `/__zfb/livereload.js`, `/__zfb/reload`, the
    /// livereload `<script>` injection into HTML, and the default
    /// `Cache-Control: no-store` shaping — to Dev only.
    ///
    /// Defaults to [`crate::ServerMode::Dev`] for byte-for-byte parity
    /// with the historical `serve_with_listener` shape used by `zfb dev`
    /// and the integration-test surface. Embed builds bypass this
    /// struct entirely (they go through [`crate::ServerBuilder`]).
    #[doc(alias = "ServerMode")]
    pub mode: crate::ServerMode,

    /// Shared handle to the current dev-mode islands bundle URL
    /// (issue #377). `None` for projects with no `"use client"`
    /// components (or when the bin crate has not seeded a bundle yet);
    /// `Some(<arc>)` carrying a URL like `/assets/islands.js` that the
    /// page handler splices into every served HTML response's `<head>`
    /// via the byte-level helper in
    /// [`zfb_build::head_inject::inject_prod_head_assets`].
    ///
    /// Dev-only: the page handler only consults this field when
    /// `mode == ServerMode::Dev`. In Preview and Embed modes the dev
    /// orchestrator does not seed the bundle URL anyway, but the gate
    /// is also enforced at the response-shaping side
    /// (see [`crate::routes::page_response_bytes`]) so an Embed caller
    /// that accidentally passes a non-None value still gets
    /// production-shaped output.
    ///
    /// The bin crate (`zfb dev`) builds this handle once at boot, hands
    /// the same `Arc` to the orchestrator's `run_islands` callback (so
    /// rebuild ticks rewrite the URL), and threads it into `ServeOpts`
    /// before calling [`serve`].
    pub islands_bundle_url: Option<crate::IslandsBundleUrl>,

    /// Shared handle to the current dev-mode CSS bundle URL
    /// (issue #494 / #498). Mirrors `islands_bundle_url` for CSS.
    /// `None` for projects with Tailwind disabled, or when the bin crate
    /// has not yet seeded a bundle. When `Some`, the page handler splices
    /// a `<link rel="stylesheet" href="<url>">` tag into every served
    /// HTML response's `<head>` via
    /// [`zfb_build::head_inject::inject_prod_head_assets`].
    ///
    /// Dev-only: gated the same way as `islands_bundle_url` — Preview
    /// and Embed callers never inject even if they pass a non-`None` handle.
    pub css_bundle_url: Option<crate::CssBundleUrl>,

    /// User-supplied `allowedHosts` entries from `zfb.config.ts`
    /// (issue #931 / #919). Only consulted when the server is bound to
    /// a non-loopback interface — see [`crate::host_validation`] for
    /// the matching rules (exact, case-insensitive, port-stripped,
    /// leading-dot suffix). Empty for the default localhost bind.
    pub allowed_hosts: Vec<String>,

    /// The host string the user explicitly bound (`--host mydev.local`
    /// / `host` in config), always allowed by the host validator so the
    /// URL the CLI banner prints never 403s. `None` when the caller has
    /// no user-supplied host string in scope (tests, embed).
    pub bound_host: Option<String>,
}

impl ServeOpts {
    /// The default bind address: `127.0.0.1:3000`.
    pub fn default_addr() -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], 3000))
    }
}

/// Run the dev server until `shutdown` resolves or the process exits.
///
/// Binds [`ServeOpts::addr`], builds the axum router via
/// [`build_router`], and serves it. The future resolves with `Ok(())`
/// once either the `shutdown` future completes (graceful stop) or axum
/// returns an error.
///
/// Pass `std::future::pending()` to run indefinitely until the process
/// is killed (matches the old behaviour).
///
/// Errors:
///
/// - failure to bind the address (port in use, permission denied, …),
/// - axum's serve loop returns an error.
///
/// This is a thin wrapper around [`serve_with_listener`]: it binds
/// `opts.addr` itself and hands the resulting [`TcpListener`] off.
/// Callers that need to know the actual bound port (e.g. integration
/// tests using ephemeral port 0) should bind their own listener and
/// call [`serve_with_listener`] directly.
pub async fn serve<S>(opts: ServeOpts, shutdown: S) -> anyhow::Result<()>
where
    S: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind(opts.addr)
        .await
        .with_context(|| format!("failed to bind dev server to {}", opts.addr))?;
    serve_with_listener(opts, listener, shutdown).await
}

/// Run the dev server on a pre-bound [`TcpListener`].
///
/// Useful when the caller needs to know the actual bound socket address
/// before the server starts accepting connections — for example
/// integration tests that bind `127.0.0.1:0` and then read
/// [`TcpListener::local_addr`] to learn the OS-chosen port.
///
/// `opts.addr` is ignored in favour of the listener's actual local
/// address (which is what gets logged).
///
/// The server runs until `shutdown` resolves. Pass
/// `std::future::pending()` to run until the process exits.
pub async fn serve_with_listener<S>(
    opts: ServeOpts,
    listener: TcpListener,
    shutdown: S,
) -> anyhow::Result<()>
where
    S: Future<Output = ()> + Send + 'static,
{
    // Issue #229: normalise `base` once at boot. The shared helper
    // (`zfb_types::dev_mount_prefix`) collapses every accepted shape
    // (None / "" / "/" / "/foo" / "/foo/" / "https://cdn…") into the
    // single canonical form the routing layer wants, so the rest of
    // the server only deals with `Option<String>` of the
    // leading-slash, no-trailing-slash kind.
    let base_prefix = zfb_types::dev_mount_prefix(opts.base.as_deref());
    // Issue #931: host validation keys off the listener's ACTUAL bound
    // address (not `opts.addr`, which is ignored by this entry point) —
    // a loopback bind disables enforcement entirely, anything else
    // enforces the allowlist.
    let actual = listener.local_addr().unwrap_or(opts.addr);
    let host_validation = HostValidation::for_bind(
        actual.ip(),
        opts.bound_host.as_deref(),
        &opts.allowed_hosts,
        opts.mode,
    );
    let state = AppState {
        mode: opts.mode,
        pages: opts.pages,
        broadcast: opts.broadcast,
        plugins: opts.plugins,
        injected_routes: opts.injected_routes,
        ssr_routes: opts.ssr_routes,
        // Rust-side embed handlers are an embed-API only seam — the
        // legacy `serve` / `serve_with_listener` entry points used by
        // `zfb dev` / `zfb preview` never register any. The embed
        // builder threads its own `AppState` directly.
        embed_handlers: None,
        dist_root: opts.dist_root.clone(),
        // Issue #534 — see `ServeOpts::html_root` for the dev / preview
        // / embed contract.
        html_root: opts.html_root.clone(),
        public_root: opts.public_root.clone(),
        base_prefix,
        trailing_slash: opts.trailing_slash,
        islands_bundle_url: opts.islands_bundle_url,
        css_bundle_url: opts.css_bundle_url,
        host_validation,
    };
    let router = build_router(state);

    info!(
        addr = %actual,
        project_root = %opts.project_root.display(),
        dist_root = %opts.dist_root.display(),
        public_root = %opts.public_root.display(),
        "zfb-server listening"
    );

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
        .context("zfb-server: axum::serve returned an error")?;

    Ok(())
}
