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
//!   [`build`](ServerBuilder::build).
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
//! ≤ 10. This module ships 9 — `Server::builder`, `Server::serve`,
//! `Server::serve_in_thread`, plus six builder methods
//! (`config_path`, `mode`, `bind`, `with_request_extension`,
//! `with_ssr_handler`, `build`). `ServerHandle`'s methods are
//! deliberately on a different type and are excluded from the budget.
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
use crate::ssr::SsrRouteSet;
use crate::{DevMiddlewareSet, InjectedRouteSet, ReloadEvent, ReloadTx};

use axum::body::Body;
use axum::http::Request;
use axum::response::IntoResponse;

/// Default bind address for an embedded server: `127.0.0.1:0` so the
/// OS picks an ephemeral port. The actual port is readable from
/// [`ServerHandle::addr`] once the server is running.
const DEFAULT_BIND: SocketAddr = SocketAddr::new(
    std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
    0,
);

/// Server flavour. Controls which dev-flavoured surfaces the server
/// exposes (live-reload script injection, `/__zfb/*` SSE endpoint,
/// `Cache-Control: no-store` on HTML).
///
/// `Dev` matches today's `zfb dev` byte-for-byte and is the default so
/// existing callers keep working. `Preview` and `Embed` are reserved
/// for follow-up sub-tasks that wire the gating; this enum exists now
/// so the builder API can lock in the public shape and embedders can
/// pass `ServerMode::Embed` without an API break later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerMode {
    /// Dev mode (default). Live-reload script is injected into every
    /// HTML response and `/__zfb/reload` is mounted.
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

impl Default for ServerMode {
    fn default() -> Self {
        Self::Dev
    }
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
    ssr_routes: Option<SsrRouteSet>,
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
        let (boot_tx, boot_rx) =
            mpsc::sync_channel::<anyhow::Result<SocketAddr>>(0);
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

                    let actual = listener
                        .local_addr()
                        .unwrap_or(bind);
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
    async fn serve_with_listener<S>(
        self,
        listener: TcpListener,
        shutdown: S,
    ) -> anyhow::Result<()>
    where
        S: Future<Output = ()> + Send + 'static,
    {
        let base_prefix = zfb_types::dev_mount_prefix(self.base.as_deref());
        let state = AppState {
            mode: self.mode,
            pages: self.pages,
            broadcast: self.broadcast,
            plugins: self.plugins,
            injected_routes: self.injected_routes,
            ssr_routes: self.ssr_routes,
            embed_handlers: self.embed_handlers,
            dist_root: self.dist_root.clone(),
            public_root: self.public_root.clone(),
            base_prefix,
            trailing_slash: self.trailing_slash,
        };
        let router = build_router(state);
        let router = apply_request_extension_layer(router, self.request_extensions);

        let actual = listener
            .local_addr()
            .unwrap_or(self.bind);
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
}

impl ServerBuilder {
    fn new() -> Self {
        Self {
            bind: None,
            mode: ServerMode::Dev,
            config_path: None,
            request_extensions: Vec::new(),
            handlers: Vec::new(),
        }
    }

    /// Path to a `zfb.config.{json,ts}` file. Drives `outDir`
    /// (→ `dist_root`), `publicDir` (→ `public_root`), `base`, and
    /// `trailingSlash`. The config file's parent directory becomes the
    /// server's `project_root`.
    ///
    /// Currently `.json` is supported directly; `.ts` returns a clear
    /// error pointing the caller at the `zfb` CLI for TS-config
    /// support (the TS loader requires `node` + esbuild and lives in
    /// the bin crate). This restriction will be lifted in a follow-up
    /// once the TS loader is hoisted into a sharable place.
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
    /// patterns are unreachable. Registered handlers short-circuit the
    /// server's other request-time and static-file fall-throughs (see
    /// the `with_ssr_handler` section of the embed-as-library guide for
    /// the full precedence table).
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

    /// Finalise the builder into a [`Server`]. Reads
    /// [`config_path`](Self::config_path) if set.
    ///
    /// Errors when:
    ///
    /// - `config_path` was not called (no way to resolve `dist_root`
    ///   / `public_root`),
    /// - `config_path` is set but the file does not exist or fails to
    ///   parse (JSON only in this milestone; `.ts` is not yet loadable
    ///   from this crate).
    pub fn build(self) -> anyhow::Result<Server> {
        let config_path = self
            .config_path
            .ok_or_else(|| {
                anyhow!(
                    "ServerBuilder::build: missing project source — call \
                     `.config_path(...)` before `.build()`"
                )
            })?;

        let (project_root, dist_root, public_root, base, trailing_slash) =
            load_embed_config(&config_path)?;

        let bind = self.bind.unwrap_or(DEFAULT_BIND);
        let pages = PageCache::new();
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

// --- config_path JSON loader -------------------------------------------------
//
// Loads just the four fields the embedded server cares about from the
// project's `zfb.config.json`. A full TS-config loader requires `node`
// + esbuild and lives in the `zfb` bin crate; the embed API's
// `config_path` only supports JSON in this milestone. The error
// message surfaces the missing TS support clearly so callers know
// where to look.

/// Minimal mirror of the embed-relevant subset of `zfb.config.json`.
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

    if ext == "ts" {
        return Err(anyhow!(
            "ServerBuilder::config_path: `.ts` configs are not yet \
             loadable from `zfb-server`'s embed API in this milestone. \
             Convert to `zfb.config.json`, or use the `zfb` CLI which \
             ships the `node`+esbuild TS loader. (Sub-task 3.1a / #370.)"
        ));
    }
    if ext != "json" {
        return Err(anyhow!(
            "ServerBuilder::config_path: unsupported extension `{}` on {}. \
             Expected `.json` (or `.ts`, which is not yet supported here).",
            ext,
            config_path.display()
        ));
    }

    let text = std::fs::read_to_string(config_path)
        .with_context(|| format!("could not read {}", config_path.display()))?;
    let cfg: EmbedConfig = serde_json::from_str(&text)
        .with_context(|| format!("could not parse {} as JSON", config_path.display()))?;

    let project_root = config_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let project_root = project_root
        .canonicalize()
        .unwrap_or(project_root);

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

    #[test]
    fn config_path_ts_returns_clear_unsupported_error() {
        let tmp = tempfile::tempdir().unwrap();
        let ts_path = tmp.path().join("zfb.config.ts");
        std::fs::write(&ts_path, "export default {}\n").unwrap();
        let err = ServerBuilder::new()
            .config_path(&ts_path)
            .build()
            .err()
            .expect("expected error");
        let msg = format!("{err:#}");
        assert!(msg.contains("not yet loadable"), "got: {msg}");
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
        let expected_root = tmp.path().canonicalize().unwrap_or_else(|_| tmp.path().to_path_buf());
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
