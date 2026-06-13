//! Embed-as-library lifecycle API for `zfb-server` (sub-task 3.1a /
//! issue #370).
//!
//! Lets a Rust host (Tauri, an axum app, a CLI tool) run zfb's HTTP
//! server in-process. The public surface is intentionally small:
//!
//! - [`Server::builder`] returns a [`ServerBuilder`].
//! - [`ServerBuilder`] chains: [`config_path`](ServerBuilder::config_path),
//!   [`mode`](ServerBuilder::mode), [`bind`](ServerBuilder::bind),
//!   [`with_request_extension`](ServerBuilder::with_request_extension),
//!   [`with_ssr_handler`](ServerBuilder::with_ssr_handler),
//!   [`build`](ServerBuilder::build). The optional
//!   [`with_page_cache`](ServerBuilder::with_page_cache) escape hatch
//!   (live-content seam) is budget-excluded — see below.
//! - [`Server`] terminals: [`serve`](Server::serve) (async, run-to-
//!   completion) and [`serve_in_thread`](Server::serve_in_thread)
//!   (spawn on a dedicated OS thread, return a handle non-blockingly).
//! - [`ServerHandle`] runtime ops: [`addr`](ServerHandle::addr),
//!   [`shutdown`](ServerHandle::shutdown) (idempotent),
//!   [`join`](ServerHandle::join).
//!
//! ## Method budget
//!
//! Per the research at `research/346-embed-as-library-api.md` §3.2 the
//! combined method count on `Server` + `ServerBuilder` must stay
//! ≤ 10. This module ships 9 counting toward the budget —
//! `Server::builder`, `Server::serve`, `Server::serve_in_thread`, plus
//! six builder methods (`config_path`, `mode`, `bind`,
//! `with_request_extension`, `with_ssr_handler`, `build`).
//! `ServerHandle`'s methods are deliberately on a different type and
//! are excluded from the budget. `with_page_cache` is the live-content
//! escape hatch and is likewise budget-excluded by design (§3.2:
//! "optional, not counted toward the ≤ 10 method budget").
//!
//! ## Threading: `serve_in_thread`
//!
//! [`serve_in_thread`](Server::serve_in_thread) mirrors the pattern in
//! `crates/zfb/src/v8_host_adapter.rs` (issue #218): spawn a dedicated
//! OS thread, build a `current_thread` tokio runtime on it, drive the
//! axum serve future inside that runtime, and bridge shutdown via a
//! one-shot channel. The advantage over `tokio::spawn` is that the
//! caller does not need a tokio runtime themselves — a synchronous
//! Tauri `setup` callback can call `serve_in_thread()` directly.
//!
//! ## Request extensions
//!
//! [`with_request_extension`](ServerBuilder::with_request_extension)
//! is a tower middleware layer (see [`crate::middleware`]) — `axum::Extension`
//! is **not** publicly re-exported. The handler retrieves the value via
//! `req.extensions().get::<T>()`.

use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{anyhow, Context as _};
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tracing::info;

use crate::embed_handlers::{erase_handler, EmbedHandler, EmbedHandlerSet, RouteParams};
use crate::middleware::{apply_request_extension_layer, make_injector, RequestExtensionInjector};
use crate::routes::{build_router, AppState, PageCache};
use crate::ssr::SsrRoutesHandle;
use crate::{DevMiddlewareSet, InjectedRouteSet, ReloadEvent, ReloadTx};

use axum::body::Body;
use axum::http::Request;
use axum::response::IntoResponse;

/// Default bind address for an embedded server: `127.0.0.1:0` so the
/// OS picks an ephemeral port. The actual port is readable from
/// [`ServerHandle::addr`] once the server is running.
const DEFAULT_BIND: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 0);

/// Server flavour. Controls which dev-flavoured surfaces the server
/// exposes (live-reload script injection, `/__zfb/*` SSE endpoint,
/// `Cache-Control: no-store` on HTML).
///
/// `Dev` matches today's `zfb dev` byte-for-byte and is the default so
/// existing callers keep working. `Preview` and `Embed` are reserved
/// for follow-up sub-tasks that wire the gating; this enum exists now
/// so the builder API can lock in the public shape and embedders can
/// pass `ServerMode::Embed` without an API break later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ServerMode {
    /// Dev mode (default). Live-reload script is injected into every
    /// HTML response and `/__zfb/reload` is mounted.
    #[default]
    Dev,
    /// Preview mode. Production-shaped output without live-reload but
    /// with the dev-server route table (used by `zfb preview`'s future
    /// migration off its bespoke router).
    Preview,
    /// Embed mode. Same shape as `Preview` but signals "running inside
    /// a host application" — reserved for follow-up wiring of
    /// per-request Tauri context, SSR handlers, etc.
    Embed,
}

/// A built, ready-to-serve embedded server. Construct via
/// [`Server::builder`]; consume via [`Server::serve`] (async) or
/// [`Server::serve_in_thread`] (spawn on a dedicated OS thread).
///
/// The struct stores all the inputs needed to materialise the axum
/// router at serve time. It is intentionally not `Clone` — both
/// terminals consume `self`.
pub struct Server {
    bind: SocketAddr,
    mode: ServerMode,
    project_root: PathBuf,
    dist_root: PathBuf,
    public_root: PathBuf,
    base: Option<String>,
    trailing_slash: bool,
    pages: PageCache,
    broadcast: ReloadTx,
    plugins: Option<DevMiddlewareSet>,
    injected_routes: Option<InjectedRouteSet>,
    ssr_routes: Option<SsrRoutesHandle>,
    embed_handlers: Option<EmbedHandlerSet>,
    request_extensions: Vec<RequestExtensionInjector>,
}

impl Server {
    /// Entry point for the embed API. Returns a fresh
    /// [`ServerBuilder`] with no required fields set.
    pub fn builder() -> ServerBuilder {
        ServerBuilder::new()
    }

    /// Run the server until `shutdown` resolves, on the caller's tokio
    /// runtime. Equivalent to [`crate::serve_with_listener`] under the
    /// hood; the difference is that this terminal takes care of
    /// binding the listener and threading the request-extension layer
    /// for the embed-API caller.
    ///
    /// Errors:
    ///
    /// - failed to bind [`ServerBuilder::bind`],
    /// - axum's serve loop returned an error.
    pub async fn serve<S>(self, shutdown: S) -> anyhow::Result<()>
    where
        S: Future<Output = ()> + Send + 'static,
    {
        let listener = TcpListener::bind(self.bind)
            .await
            .with_context(|| format!("failed to bind embedded zfb server to {}", self.bind))?;
        self.serve_with_listener(listener, shutdown).await
    }

    /// Spawn the server on a dedicated OS thread with its own
    /// `current_thread` tokio runtime, and return a non-blocking
    /// [`ServerHandle`].
    ///
    /// The thread:
    ///
    /// 1. Builds a `current_thread` tokio runtime.
    /// 2. Binds the listener (so the bound port is known before this
    ///    function returns — embedders read it via
    ///    [`ServerHandle::addr`]).
    /// 3. Drives `axum::serve(...).with_graceful_shutdown(rx)` until
    ///    the handle's shutdown one-shot fires or the process exits.
    ///
    /// Mirrors the pattern at
    /// `crates/zfb/src/v8_host_adapter.rs`: boot signalling via one
    /// channel, shutdown signalling via another. Returns an error if
    /// the thread fails to boot (bind error, runtime error).
    pub fn serve_in_thread(self) -> anyhow::Result<ServerHandle> {
        let bind = self.bind;
        let (boot_tx, boot_rx) = mpsc::sync_channel::<anyhow::Result<SocketAddr>>(0);
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let thread_handle = thread::Builder::new()
            .name("zfb-embed-server".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = boot_tx.send(Err(anyhow!(
                            "failed to build current-thread tokio runtime for embedded zfb server: {e}"
                        )));
                        return Ok(());
                    }
                };

                rt.block_on(async move {
                    let listener = match TcpListener::bind(bind).await {
                        Ok(l) => l,
                        Err(e) => {
                            let _ = boot_tx.send(Err(anyhow!(
                                "failed to bind embedded zfb server to {bind}: {e}"
                            )));
                            return Ok::<(), anyhow::Error>(());
                        }
                    };

                    // Once `bind()` has returned Ok the listener is
                    // accepting on a real port — `local_addr()` is a
                    // syscall against an open fd that has never been
                    // observed to fail in practice. `expect()` here
                    // surfaces the impossible case loudly instead of
                    // silently reporting port 0 back to the embed
                    // caller's ServerHandle::addr (deep-review fix,
                    // PR #376).
                    let actual = listener
                        .local_addr()
                        .expect("listener.local_addr() must succeed after bind");
                    // Signal boot success with the actual bound address
                    // before entering the serve loop.
                    let _ = boot_tx.send(Ok(actual));

                    let shutdown_fut = async move {
                        let _ = shutdown_rx.await;
                    };
                    self.serve_with_listener(listener, shutdown_fut).await
                })
            })
            .context("failed to spawn embedded zfb server OS thread")?;

        // Block until the worker signals either the bound address or a
        // boot error.
        let addr = match boot_rx.recv() {
            Ok(Ok(addr)) => addr,
            Ok(Err(e)) => {
                let _ = thread_handle.join();
                return Err(e);
            }
            Err(_) => {
                let _ = thread_handle.join();
                return Err(anyhow!(
                    "embedded zfb server thread exited during boot without signalling"
                ));
            }
        };

        Ok(ServerHandle {
            addr,
            shutdown_tx: Arc::new(Mutex::new(Some(shutdown_tx))),
            thread: Arc::new(Mutex::new(Some(thread_handle))),
        })
    }

    /// Internal: serve on the supplied listener. Replicates the logic
    /// of [`crate::serve_with_listener`] but threads the
    /// request-extension layer that the embed builder accumulated.
    async fn serve_with_listener<S>(self, listener: TcpListener, shutdown: S) -> anyhow::Result<()>
    where
        S: Future<Output = ()> + Send + 'static,
    {
        let base_prefix = zfb_types::dev_mount_prefix(self.base.as_deref());
        // See the matching expect() in `serve_in_thread` above —
        // local_addr() against an Ok-bound listener is essentially
        // infallible, and silently falling back to the requested
        // `self.bind` (which may carry port 0 for an ephemeral bind)
        // makes log/metrics noise.
        let actual = listener
            .local_addr()
            .expect("listener.local_addr() must succeed after bind");
        // Issue #931: embed callers have no `allowedHosts` knob yet —
        // a non-loopback bind enforces the built-in allowlist
        // (localhost forms + the bound IP); the default loopback bind
        // disables validation entirely.
        let host_validation =
            crate::host_validation::HostValidation::for_bind(actual.ip(), None, &[], self.mode);
        let state = AppState {
            mode: self.mode,
            pages: self.pages,
            broadcast: self.broadcast,
            plugins: self.plugins,
            injected_routes: self.injected_routes,
            ssr_routes: self.ssr_routes,
            embed_handlers: self.embed_handlers,
            dist_root: self.dist_root.clone(),
            // Embed callers do not have a separate dev HTML dir — the
            // page-cache disk fallback reads from the same `dist_root`
            // the build pipeline wrote into. The `html_root` /
            // `dist_root` split only matters in `zfb dev` (issue #534).
            html_root: self.dist_root.clone(),
            public_root: self.public_root.clone(),
            base_prefix,
            trailing_slash: self.trailing_slash,
            // Embed callers do not get the dev-mode islands head
            // injection — the response shaper in `page_response_bytes`
            // is gated to `ServerMode::Dev` anyway, but leaving the
            // handle absent makes the no-injection contract obvious.
            islands_bundle_url: None,
            // Same reasoning as islands — embed callers get no CSS link
            // injection; the Dev-mode gate in `page_response_bytes` also
            // enforces this, but `None` keeps the contract explicit.
            css_bundle_url: None,
            host_validation,
            // Embed callers do not use the render-on-request hook —
            // it is Dev-only and the hook gate in `serve_page` enforces
            // this, but `None` keeps the contract explicit.
            render_on_request_hook: None,
        };
        let router = build_router(state);
        let router = apply_request_extension_layer(router, self.request_extensions);

        info!(
            addr = %actual,
            mode = ?self.mode,
            project_root = %self.project_root.display(),
            dist_root = %self.dist_root.display(),
            public_root = %self.public_root.display(),
            "zfb-server (embed) listening"
        );

        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown)
            .await
            .context("zfb-server: axum::serve returned an error")?;
        Ok(())
    }
}

/// Builder for [`Server`]. Construct via [`Server::builder`].
///
/// Required: a [`config_path`](ServerBuilder::config_path) call before
/// [`build`](Self::build). Callers that need more control than
/// `config_path` exposes (custom page cache, plugin host wiring, …)
/// can drop to the lower-level [`crate::serve_with_listener`] entry
/// directly — that path remains public and stable.
pub struct ServerBuilder {
    bind: Option<SocketAddr>,
    mode: ServerMode,
    config_path: Option<PathBuf>,
    request_extensions: Vec<RequestExtensionInjector>,
    handlers: Vec<EmbedHandler>,
    page_cache: Option<PageCache>,
}

impl ServerBuilder {
    fn new() -> Self {
        Self {
            bind: None,
            mode: ServerMode::Dev,
            config_path: None,
            request_extensions: Vec::new(),
            handlers: Vec::new(),
            page_cache: None,
        }
    }

    /// Path to a `zfb.config.{json,ts}` file. Drives `outDir`
    /// (→ `dist_root`), `publicDir` (→ `public_root`), `base`, and
    /// `trailingSlash`. The config file's parent directory becomes the
    /// server's `project_root`.
    ///
    /// Both `.json` and `.ts` are supported. A `.ts` config is evaluated
    /// the same way the `zfb` CLI does it (issue #1037) — via the
    /// in-process V8 + esbuild evaluator on default builds, or the `node`
    /// subprocess on slim builds — through the shared `zfb-config-loader`
    /// crate. The evaluation is async; [`build`](Self::build) drives it to
    /// completion on a dedicated thread so the synchronous builder API is
    /// preserved (and works whether or not the caller is inside a tokio
    /// runtime).
    pub fn config_path<P: AsRef<Path>>(mut self, path: P) -> Self {
        self.config_path = Some(path.as_ref().to_path_buf());
        self
    }

    /// Set the server [`ServerMode`]. Default: [`ServerMode::Dev`].
    pub fn mode(mut self, mode: ServerMode) -> Self {
        self.mode = mode;
        self
    }

    /// Socket address to bind. Pass `"127.0.0.1:0".parse()?` for an
    /// OS-assigned ephemeral port; read the actual port back from
    /// [`ServerHandle::addr`] after [`Server::serve_in_thread`]
    /// returns.
    ///
    /// Default: `127.0.0.1:0` (ephemeral) so embed callers don't trip
    /// "Address already in use" with the legacy `127.0.0.1:3000`
    /// default that `zfb dev` uses.
    pub fn bind(mut self, addr: SocketAddr) -> Self {
        self.bind = Some(addr);
        self
    }

    /// Register a value to be cloned into every incoming request's
    /// `http::Extensions` map. The handler retrieves it via
    /// `req.extensions().get::<T>()`.
    ///
    /// `axum::Extension<T>` is **not** required on the handler side
    /// (the embed API deliberately keeps that off its public surface
    /// — see `research/346-embed-as-library-api.md` §3.3).
    ///
    /// Calling this multiple times with different types accumulates
    /// the values; calling twice with the same `T` results in the
    /// second value overwriting the first inside `Extensions::insert`.
    pub fn with_request_extension<T>(mut self, value: T) -> Self
    where
        T: Clone + Send + Sync + 'static,
    {
        self.request_extensions.push(make_injector(value));
        self
    }

    /// Register an async handler for HTTP requests whose URL path
    /// matches `pattern`. The handler is invoked with the inbound
    /// [`Request`] and the captured [`RouteParams`].
    ///
    /// `pattern` is a leading-slash path. Each segment is either a
    /// literal (`/health`), a single-segment parameter (`/users/:id`)
    /// captured under the name following the colon, or a wildcard tail
    /// (`/files/*rest`) capturing the remaining segments under the
    /// name following the asterisk. A wildcard segment must be the
    /// final segment of the pattern. Empty captures are rejected.
    ///
    /// `handler` is `async fn(Request<Body>, RouteParams) -> impl
    /// IntoResponse` (or any callable with that shape). The body type
    /// returned by the handler is converted via
    /// [`axum::response::IntoResponse`] so handlers can yield strings,
    /// tuples (status, body), `http::Response<…>`, or any other
    /// `IntoResponse` value. Per-request `http::Extensions` registered
    /// via [`Self::with_request_extension`] are forwarded into the
    /// request the handler sees, so values can be read with
    /// `req.extensions().get::<T>()`.
    ///
    /// Multiple calls accumulate. Patterns are matched in registration
    /// order on every request, and the **first** pattern that matches
    /// claims the request — subsequent registrations with overlapping
    /// patterns are unreachable.
    ///
    /// ## Precedence
    ///
    /// On every page-shaped request the router consults handlers in
    /// this order (highest priority first):
    ///
    /// 1. plugin dev-middleware (longest-prefix match),
    /// 2. **embed handlers registered here** (`with_ssr_handler`),
    /// 3. request-time JS SSR ([`crate::ssr::SsrRouteSet`]),
    /// 4. in-memory page cache (SSG output),
    /// 5. `<dist>/...` on-disk fallback,
    /// 6. `<public>/...` on-disk fallback,
    /// 7. dev 404 body.
    ///
    /// So a Rust handler wins over the JS SSR dispatcher and the static
    /// file fall-throughs but is shadowed by a plugin that claims the
    /// same path. Deep-review doc fix (PR #376) — the previous comment
    /// implied only "Rust > SSR" and didn't mention the plugin layer.
    pub fn with_ssr_handler<F, Fut, R>(mut self, pattern: impl Into<String>, handler: F) -> Self
    where
        F: Fn(Request<Body>, RouteParams) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: IntoResponse + 'static,
    {
        self.handlers.push(EmbedHandler {
            pattern: pattern.into(),
            handler: erase_handler(handler),
        });
        self
    }

    /// Supply a [`PageCache`] the server should serve from instead of
    /// the empty one [`build`](Self::build) allocates by default.
    ///
    /// The page cache is `Clone` (it wraps an `Arc<RwLock<…>>`), so an
    /// embedder can keep a handle of its own, hand a clone here, and
    /// then drive the server's content at runtime by calling
    /// [`PageCache::insert`] / [`PageCache::replace_all`] /
    /// [`PageCache::remove`] from a file-watcher callback or a rebuild
    /// loop — the running server reads the same shared map. This is the
    /// builder-level entry point for the live-content scenarios that
    /// otherwise require dropping to [`crate::serve_with_listener`] with
    /// a hand-built [`crate::ServeOpts`].
    ///
    /// Calling this more than once keeps the last cache. When never
    /// called, [`build`](Self::build) allocates a fresh empty cache.
    pub fn with_page_cache(mut self, cache: PageCache) -> Self {
        self.page_cache = Some(cache);
        self
    }

    /// Finalise the builder into a [`Server`]. Reads
    /// [`config_path`](Self::config_path) if set.
    ///
    /// Errors when:
    ///
    /// - `config_path` was not called (no way to resolve `dist_root`
    ///   / `public_root`),
    /// - `config_path` is set but the file does not exist, has an
    ///   unsupported extension, or fails to parse / evaluate (both
    ///   `.json` and `.ts` are supported — see
    ///   [`config_path`](Self::config_path)).
    pub fn build(self) -> anyhow::Result<Server> {
        let config_path = self.config_path.ok_or_else(|| {
            anyhow!(
                "ServerBuilder::build: missing project source — call \
                     `.config_path(...)` before `.build()`"
            )
        })?;

        let (project_root, dist_root, public_root, base, trailing_slash) =
            load_embed_config(&config_path)?;

        let bind = self.bind.unwrap_or(DEFAULT_BIND);
        let pages = self.page_cache.unwrap_or_default();
        let (broadcast, _rx) = tokio::sync::broadcast::channel::<ReloadEvent>(64);

        let embed_handlers = if self.handlers.is_empty() {
            None
        } else {
            Some(EmbedHandlerSet::new(self.handlers))
        };

        Ok(Server {
            bind,
            mode: self.mode,
            project_root,
            dist_root,
            public_root,
            base,
            trailing_slash,
            pages,
            broadcast,
            plugins: None,
            injected_routes: None,
            ssr_routes: None,
            embed_handlers,
            request_extensions: self.request_extensions,
        })
    }
}

/// Runtime handle for a server started by [`Server::serve_in_thread`].
///
/// Returned non-blockingly: the handle's [`addr`](Self::addr) is the
/// actual bound socket (useful when `bind = 127.0.0.1:0`), and
/// [`shutdown`](Self::shutdown) sends the graceful-shutdown signal.
/// [`shutdown`](Self::shutdown) is **idempotent** — calling it a second
/// time is a no-op (the captured one-shot sender has already been
/// `take`n out of the internal `Mutex<Option<…>>`).
///
/// The handle is `Clone` so embedders can hand copies to multiple
/// shutdown call-sites (Tauri's `on_window_event`, Ctrl-C handler,
/// etc.); all copies share the same one-shot sender and join handle
/// behind `Arc<Mutex<…>>`.
///
/// Methods on this type do NOT count toward the
/// `Server` + `ServerBuilder` ≤ 10 budget (handle ops are deliberately
/// on a separate type — see `research/346-embed-as-library-api.md`
/// §3.2).
#[derive(Clone)]
pub struct ServerHandle {
    addr: SocketAddr,
    shutdown_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    thread: Arc<Mutex<Option<thread::JoinHandle<anyhow::Result<()>>>>>,
}

impl ServerHandle {
    /// The actual bound socket address. When the builder was given
    /// `127.0.0.1:0` this is the OS-assigned port.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Send the graceful-shutdown signal to the server thread.
    /// Idempotent — a second call is a no-op (returns `Ok(())`
    /// without sending). The server thread itself exits once axum's
    /// graceful-shutdown future resolves; call [`Self::join`] to
    /// wait for that.
    pub fn shutdown(&self) -> anyhow::Result<()> {
        let mut guard = self
            .shutdown_tx
            .lock()
            .map_err(|_| anyhow!("ServerHandle shutdown lock poisoned"))?;
        if let Some(tx) = guard.take() {
            // Sender::send returns Err if the receiver was dropped
            // (server already exited). That's fine — the goal was
            // "ask the server to stop" and the server has already
            // stopped, so we report success.
            let _ = tx.send(());
        }
        Ok(())
    }

    /// Wait for the server thread to exit and propagate its result.
    /// Returns:
    ///
    /// - `Ok(Ok(()))` — server exited cleanly after the graceful
    ///   shutdown future resolved.
    /// - `Ok(Err(e))` — axum's serve loop returned an error before
    ///   exit.
    /// - `Err(e)` — the OS thread panicked, or `join` was called
    ///   twice (the handle is single-shot for joining; subsequent
    ///   calls return an error).
    pub fn join(&self) -> anyhow::Result<anyhow::Result<()>> {
        let handle = self
            .thread
            .lock()
            .map_err(|_| anyhow!("ServerHandle join lock poisoned"))?
            .take();
        let Some(handle) = handle else {
            return Err(anyhow!(
                "ServerHandle::join already called — thread is single-shot"
            ));
        };
        handle
            .join()
            .map_err(|_| anyhow!("embedded zfb server thread panicked"))
    }
}

// --- config_path loader -------------------------------------------------
//
// Loads just the four fields the embedded server cares about from the
// project's `zfb.config.{json,ts}`. JSON is read + parsed directly; TS is
// evaluated via the shared `zfb-config-loader` crate (issue #1037) — the
// same esbuild + V8 / node evaluator the `zfb` CLI uses — so an embedder can
// point the builder at an existing `zfb.config.ts` without maintaining a
// parallel hand-synced `zfb.config.json`.

/// Minimal mirror of the embed-relevant subset of `zfb.config.{json,ts}`.
/// Field names match the canonical TS shape (`#[serde(rename_all =
/// "camelCase")]`) so callers can author one config file that works
/// for both `zfb dev` and the embed API.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmbedConfig {
    /// `outDir` — defaults to `dist` if absent.
    #[serde(default = "default_out_dir")]
    out_dir: PathBuf,
    /// `publicDir` — defaults to `public` if absent.
    #[serde(default = "default_public_dir")]
    public_dir: PathBuf,
    /// `base` — None / omitted / `""` / `"/"` is treated as "no mount
    /// prefix" by `zfb_types::dev_mount_prefix`.
    #[serde(default)]
    base: Option<String>,
    /// `trailingSlash` — defaults to `false`.
    #[serde(default)]
    trailing_slash: bool,
}

fn default_out_dir() -> PathBuf {
    PathBuf::from("dist")
}

fn default_public_dir() -> PathBuf {
    PathBuf::from("public")
}

/// Resolve the four embed-relevant fields from a `zfb.config.{json,ts}`
/// path. Returns `(project_root, dist_root, public_root, base,
/// trailing_slash)` with all paths absolute (relative paths inside the
/// config are joined against the config file's parent directory).
fn load_embed_config(
    config_path: &Path,
) -> anyhow::Result<(PathBuf, PathBuf, PathBuf, Option<String>, bool)> {
    if !config_path.exists() {
        return Err(anyhow!(
            "ServerBuilder::config_path: file not found: {}",
            config_path.display()
        ));
    }
    let ext = config_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let cfg: EmbedConfig = match ext {
        "json" => {
            let text = std::fs::read_to_string(config_path)
                .with_context(|| format!("could not read {}", config_path.display()))?;
            serde_json::from_str(&text)
                .with_context(|| format!("could not parse {} as JSON", config_path.display()))?
        }
        "ts" => load_embed_config_from_ts(config_path)?,
        other => {
            return Err(anyhow!(
                "ServerBuilder::config_path: unsupported extension `{}` on {}. \
                 Expected `.json` or `.ts`.",
                other,
                config_path.display()
            ));
        }
    };

    let project_root = config_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let project_root = project_root.canonicalize().unwrap_or(project_root);

    let dist_root = if cfg.out_dir.is_absolute() {
        cfg.out_dir
    } else {
        project_root.join(&cfg.out_dir)
    };
    let public_root = if cfg.public_dir.is_absolute() {
        cfg.public_dir
    } else {
        project_root.join(&cfg.public_dir)
    };

    Ok((
        project_root,
        dist_root,
        public_root,
        cfg.base,
        cfg.trailing_slash,
    ))
}

/// Evaluate a `zfb.config.ts` via the shared `zfb-config-loader` crate and
/// deserialise its `default` export into the embed-relevant [`EmbedConfig`]
/// subset (issue #1037).
///
/// The evaluator is async (it spawns esbuild and, on slim builds, `node`),
/// but [`ServerBuilder::build`] is synchronous. We run the load on a
/// dedicated OS thread that owns a `current_thread` tokio runtime — the same
/// pattern [`Server::serve_in_thread`] uses — so this works whether or not
/// the caller is already inside a tokio runtime (a bare `Runtime::block_on`
/// from within a runtime would panic). The project root passed to the
/// evaluator is the config file's parent directory.
///
/// The config path is canonicalised to an absolute path first. The loader
/// runs esbuild with `current_dir(project_root)` and passes `ts_path` as the
/// entry, so a relative `config_path` (e.g. `"site/zfb.config.ts"`, or a bare
/// `"zfb.config.ts"` whose `parent()` is the empty path) would otherwise make
/// esbuild `chdir` to the wrong place or double-join the directory segment.
fn load_embed_config_from_ts(config_path: &Path) -> anyhow::Result<EmbedConfig> {
    let ts_path = config_path
        .canonicalize()
        .with_context(|| format!("could not resolve {}", config_path.display()))?;
    let project_root = ts_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let loaded = thread::scope(|scope| {
        scope
            .spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .context("failed to build tokio runtime for TS config evaluation")?;
                rt.block_on(zfb_config_loader::load_from_ts_file(
                    &ts_path,
                    &project_root,
                    &zfb_config_loader::LoadOptions::default(),
                ))
            })
            .join()
            .map_err(|_| anyhow!("TS config evaluation thread panicked"))?
    })
    .with_context(|| format!("evaluating {}", config_path.display()))?;

    serde_json::from_value(loaded.config)
        .with_context(|| format!("could not parse {} as a zfb config", config_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_mode_default_is_dev() {
        assert_eq!(ServerMode::default(), ServerMode::Dev);
    }

    #[test]
    fn config_path_missing_file_errors_clearly() {
        let err = ServerBuilder::new()
            .config_path("/nonexistent/zfb.config.json")
            .build()
            .err()
            .expect("expected error");
        let msg = format!("{err:#}");
        assert!(msg.contains("file not found"), "got: {msg}");
    }

    /// `.ts` configs are no longer rejected at the extension check (issue
    /// #1037 hoisted the TS evaluator into `zfb-config-loader`). The
    /// extension branch now routes to evaluation rather than returning the
    /// old "not yet loadable from `zfb-server`'s embed API" hard error.
    ///
    /// This test does NOT assert a successful load: actually evaluating the
    /// `.ts` needs esbuild (default builds also need the in-process V8
    /// isolate), neither of which is staged in this crate's unit-test env —
    /// `zfb-server` ships no `EMBEDDED_VENDOR` snapshot, by design. The
    /// successful-evaluation path is covered by `zfb`'s `config.rs` V8 tests
    /// (run in CI with `embed_v8`). Here we only prove the hard-error branch
    /// is gone: whatever error surfaces (esbuild-missing in CI, or success on
    /// a machine with `ZFB_ESBUILD_BIN` set) must NOT be the old milestone
    /// rejection or the unsupported-extension message.
    #[test]
    fn config_path_ts_no_longer_hard_errors_as_unsupported() {
        let tmp = tempfile::tempdir().unwrap();
        let ts_path = tmp.path().join("zfb.config.ts");
        std::fs::write(&ts_path, "export default {}\n").unwrap();
        let result = ServerBuilder::new().config_path(&ts_path).build();
        if let Err(err) = result {
            let msg = format!("{err:#}");
            assert!(
                !msg.contains("not yet loadable"),
                "`.ts` must no longer hard-error as a milestone gap; got: {msg}"
            );
            assert!(
                !msg.contains("unsupported extension"),
                "`.ts` must be a recognised extension; got: {msg}"
            );
        }
        // Ok(_) (esbuild + evaluator available locally) is also a pass.
    }

    #[test]
    fn config_path_json_resolves_dist_and_public_relative_to_config() {
        let tmp = tempfile::tempdir().unwrap();
        let json_path = tmp.path().join("zfb.config.json");
        std::fs::write(
            &json_path,
            r#"{"outDir":"build","publicDir":"static","base":"/foo/","trailingSlash":true}"#,
        )
        .unwrap();
        let (project_root, dist_root, public_root, base, trailing_slash) =
            load_embed_config(&json_path).unwrap();
        // Canonicalised project root must point at the config's parent.
        let expected_root = tmp
            .path()
            .canonicalize()
            .unwrap_or_else(|_| tmp.path().to_path_buf());
        assert_eq!(project_root, expected_root);
        assert_eq!(dist_root, project_root.join("build"));
        assert_eq!(public_root, project_root.join("static"));
        assert_eq!(base.as_deref(), Some("/foo/"));
        assert!(trailing_slash);
    }

    #[test]
    fn build_without_config_path_errors() {
        let err = Server::builder().build().err().expect("expected error");
        let msg = format!("{err:#}");
        assert!(msg.contains("missing project source"), "got: {msg}");
    }

    #[tokio::test]
    async fn with_page_cache_supplies_caller_cache_to_server() {
        let tmp = tempfile::tempdir().unwrap();
        let json_path = tmp.path().join("zfb.config.json");
        std::fs::write(&json_path, r#"{}"#).unwrap();

        // A cache the caller pre-seeds and keeps a handle to.
        let cache = PageCache::new();
        cache.insert("/live", "<h1>live</h1>").await;

        let server = Server::builder()
            .config_path(&json_path)
            .with_page_cache(cache.clone())
            .build()
            .unwrap();

        // The server must serve from the supplied cache, not a fresh
        // empty one — the pre-seeded entry is visible on the server's
        // `pages`, and a later write through the caller's handle is too.
        assert!(server.pages.get("/live").await.is_some());
        cache.insert("/runtime", "<h1>runtime</h1>").await;
        assert!(server.pages.get("/runtime").await.is_some());
    }

    #[tokio::test]
    async fn build_without_page_cache_allocates_empty_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let json_path = tmp.path().join("zfb.config.json");
        std::fs::write(&json_path, r#"{}"#).unwrap();
        let server = Server::builder().config_path(&json_path).build().unwrap();
        assert!(server.pages.get("/live").await.is_none());
    }

    #[test]
    fn bind_defaults_to_ephemeral_localhost() {
        let tmp = tempfile::tempdir().unwrap();
        let json_path = tmp.path().join("zfb.config.json");
        std::fs::write(&json_path, r#"{}"#).unwrap();
        // The smoke test exercises the actual serve path; here we
        // just check the default bind value got plumbed through.
        let server = Server::builder().config_path(&json_path).build().unwrap();
        assert_eq!(server.bind, DEFAULT_BIND);
    }
}
