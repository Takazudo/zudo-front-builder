//! Build-time render **orchestrator** — wave 2 / T6.
//!
//! Ties together the wave-1 outputs of the SSG-render epic:
//!
//! - The bundler ([`crate::bundler`]) emits a single self-contained
//!   Worker bundle (`bundle.mjs` + `bundle.mjs.map`) plus a
//!   [`BundleManifest`] enumerating the routes the worker can serve.
//! - The router (`zfb-router`) and the static `paths()` expansion of
//!   each page module produce the **route universe**: every concrete
//!   URL the build wants on disk.
//! - The TSX frontmatter extractor ([`zfb_content::tsx_frontmatter`])
//!   produces the **prerender map**: page-route → `bool` flagging
//!   whether the page is SSG (default `true`) or SSR-only (`false`).
//!
//! Given those inputs, [`render_all`] does five things:
//!
//! 1. Partitions the route universe into SSG vs SSR using the
//!    prerender map. SSR-only routes are collected into [`SsrManifest`]
//!    and **not** rendered now — they are handed back to the caller
//!    (T7) for use at runtime.
//! 2. Constructs the embedded V8 host (in-process) from the bundle
//!    path. The host is long-lived across the entire build so V8's
//!    module parse cost is paid once.
//! 3. For each SSG route, dispatches the request directly to the
//!    in-process host without a TCP hop. Non-2xx responses are
//!    surfaced as [`RendererError::RenderFailed`] with a re-projected
//!    source location (see step 5).
//! 4. For HTTP backends ([`Backend::Existing`]): `GET`s the worker via
//!    `reqwest::blocking` against the supplied base URL instead.
//! 5. On any worker-thrown error containing a JS stack pointing at the
//!    bundle, the [`sourcemap`] crate walks the bundle's `.map` and
//!    re-projects each frame to the user's `.tsx` / `.mdx` source
//!    location. The error message names the user file so the operator
//!    can fix the page directly without grepping minified bundle output.
//!
//! ### Dev mode (T7 will consume this)
//!
//! `zfb dev` cannot afford to start a fresh host per file save.
//! [`start`] returns a [`RendererState`] that owns the long-lived
//! embedded V8 host; [`render_one`] drives a single route against it;
//! and [`shutdown`] tears the host down on dev-server exit. The same
//! host and source-map are reused across renders — when the bundle
//! changes on disk, T7 swaps the [`RendererState`] for a new one via
//! [`reload`] rather than mutating the old one in-place.
//!
//! ### What this module is NOT responsible for
//!
//! - **Wrapping the bundler output into a Worker entry.** The renderer
//!   takes a `bundle_path` that already exports `default { fetch }`.
//!   The bundler emits that wrapper today (synthetic `entry.mjs` —
//!   see `zfb_build::bundler::write_entry_module`) by importing the
//!   page modules' `routes`/`hydrateIsland` and constructing a
//!   `createPageRouter` instance. Keeping the wrapping decision
//!   (framework choice, `ContentSnapshot` shape, render-to-string
//!   adapter) in the bundler layer means the renderer stays neutral
//!   about which JSX runtime is in play.
//! - **Determining the route universe.** The caller already has the
//!   router scan + `paths()` expansion. The renderer is given the
//!   list and trusts it.
//! - **Reading the prerender flag from disk.** The caller passes the
//!   pre-extracted map.
//!
//! ### Test surface
//!
//! The bulk of the test suite uses [`Backend::Stub`] to answer requests
//! from a closure — no HTTP client, no subprocess, no V8. This eliminates
//! the TCP round-trip overhead from unit tests. See the test module at the
//! bottom of the file.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::bundler::BundleManifest;

// ---------------------------------------------------------------------------
// Public types — input
// ---------------------------------------------------------------------------

/// One concrete URL the build wants on disk.
///
/// The renderer treats `url_path` as opaque: it sends a `GET` for it
/// and writes the response body to `output_path` under `dist_dir`.
/// `route_key` is the page module's route template (e.g. `/blog/[slug]`)
/// — it's the join key between the route universe and the prerender
/// map. The caller produces these by combining `zfb-router`'s scan
/// with each page's `paths()` static expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteUniverseEntry {
    /// Concrete URL path the worker is asked to serve. Examples:
    /// `/`, `/about`, `/blog/hello-world`, `/feed.xml`.
    pub url_path: String,
    /// Filesystem-relative output path under `dist_dir`. Examples:
    /// `index.html`, `about/index.html`, `blog/hello-world/index.html`,
    /// `feed.xml`. The renderer writes exactly here — it does not
    /// invent extensions or rewrite trailing slashes.
    pub output_path: PathBuf,
    /// Page-route template (Hono `:param` style or whatever the
    /// router uses internally — kept opaque). Used as the lookup key
    /// against `prerender_map`.
    pub route_key: String,
}

/// The HTTP-like response returned by [`EmbeddedV8Host::dispatch_fetch`]
/// and by the [`Backend::Stub`] handler closure.
///
/// Carries the minimal surface the renderer needs: body bytes, HTTP
/// status code, and content-type header. The `content_type` field is
/// informational only — the renderer does not parse or branch on it
/// beyond HEAD injection (which reads for `</head>`).
#[derive(Debug, Clone)]
pub struct HttpResponseLike {
    /// HTTP status code (e.g. 200, 404, 500).
    pub status: u16,
    /// Value of the `Content-Type` response header, or an empty string
    /// when absent.
    pub content_type: String,
    /// Full response body bytes.
    pub body: Vec<u8>,
}

/// Trait object boundary for the in-process V8 render host.
///
/// `EmbeddedV8RenderHost` is the sole production impl. The
/// trait is intentionally thin — only `dispatch_fetch` is needed by
/// this crate; the full `RenderHost` trait lives in `zfb-render` for
/// the higher-level render orchestration.
///
/// The trait requires `Send` (but not `Sync`): a V8 isolate must not be
/// called from multiple threads simultaneously (`!Sync`), but ownership
/// can be transferred between threads. In dev mode the host is wrapped
/// in `Mutex<Option<RendererState>>` — the lock provides the single-
/// thread access guarantee; `Send` is needed for the `Mutex` guard
/// to cross the `PageRenderer` closure's `Send` bound.
pub trait EmbeddedV8Host: Send {
    /// Dispatch a synthetic HTTP GET for `url_path` against the loaded
    /// bundle and return the response.
    ///
    /// `url_path` is the URL path component only (e.g. `/about`,
    /// `/blog/hello-world`). The host constructs a full request URL by
    /// prepending a synthetic origin (e.g. `http://localhost`).
    ///
    /// Errors are surfaced as `RendererError::EmbeddedV8` — the caller
    /// does NOT see HTTP errors here; a non-2xx from the bundle handler
    /// is still returned as `Ok(HttpResponseLike { status: 500, … })`.
    /// Only infrastructure-level failures (isolate crash, OOM, etc.)
    /// return `Err`.
    fn dispatch_fetch(&mut self, url_path: &str) -> Result<HttpResponseLike, RendererError>;

    /// Dispatch a synthetic HTTP request with full method / headers /
    /// body against the loaded bundle (issue #367 / Gap 1).
    ///
    /// `url_path` is the same path-and-query shape `dispatch_fetch`
    /// takes (e.g. `/api/submit?since=42`). `headers` keys are
    /// case-insensitive on the wire; the host normalises them. `body`
    /// is the raw bytes the client sent — empty for GET/HEAD, the
    /// POST/PUT payload for write methods.
    ///
    /// Default implementation forwards to [`Self::dispatch_fetch`]
    /// for backwards compatibility — implementers that can carry the
    /// full request shape (the production `ThreadedV8Host`) override
    /// it. The default lets `Backend::Stub` and test doubles keep
    /// working unchanged.
    fn dispatch_fetch_full(
        &mut self,
        url_path: &str,
        method: &str,
        headers: &std::collections::BTreeMap<String, String>,
        body: &[u8],
    ) -> Result<HttpResponseLike, RendererError> {
        // Silence unused warnings on the default path — the override
        // in `ThreadedV8Host` is the load-bearing implementation.
        let _ = (method, headers, body);
        self.dispatch_fetch(url_path)
    }
}

/// Factory type for constructing an [`EmbeddedV8Host`] from a bundle path.
///
/// Wrapping in `Arc` lets [`Backend`] remain `Clone` while still
/// carrying an `Fn` closure with non-Clone captures. The factory is
/// called once per `launch()` / `start()` invocation (i.e. once per
/// build or once per dev-mode `reload()`). The returned host is
/// `Box<dyn EmbeddedV8Host>` so the factory itself does not need to
/// know the concrete type at the call site.
///
/// The factory receives the resolved `bundle_path` (absolute) so
/// callers can pass compat-flags, module-resolver overrides, or other
/// construction-time concerns without plumbing them through the renderer.
pub type EmbeddedV8HostFactory =
    Arc<dyn Fn(&Path) -> Result<Box<dyn EmbeddedV8Host>, RendererError> + Send + Sync>;

/// The renderer's backend selector.
///
/// `Existing` stays for side-by-side testing against a pre-running HTTP
/// server. `EmbeddedV8` is the production path (in-process V8 isolate).
/// `Stub` is the unit-test surface.
///
/// Default (via manual impl) is `EmbeddedV8` with a no-op factory stub
/// that returns an error — callers must supply a real factory before
/// invoking [`render_all`] or [`start`].
#[derive(Clone)]
pub enum Backend {
    /// Skip host construction and use this base URL (e.g.
    /// `http://127.0.0.1:54321/`). The renderer still does the GET
    /// loop, dist write, and source-map error re-projection. Useful for
    /// testing against a pre-running HTTP server without spawning a new host.
    Existing { base_url: String },
    /// In-process V8 isolate (via `EmbeddedV8RenderHost`).
    ///
    /// `host_factory` is called once per build/reload to construct the
    /// host from the bundle path. The factory typically instantiates
    /// `EmbeddedV8RenderHost::new(bundle_path)`. Wrapping in `Arc`
    /// keeps `Backend` cheaply clone-able even though the factory is
    /// a closure.
    EmbeddedV8 { host_factory: EmbeddedV8HostFactory },
    /// In-process stub handler for unit tests.
    ///
    /// The closure receives the URL path (e.g. `/about`) and returns an
    /// [`HttpResponseLike`]. No HTTP client is created; no subprocess is
    /// spawned. This replaces the old `Backend::Existing`-against-a-fake-
    /// HTTP-server pattern so renderer logic is testable without a real
    /// TCP server.
    ///
    /// The closure is wrapped in `Arc` so `Backend` remains `Clone`.
    Stub {
        handler: Arc<dyn Fn(&str) -> HttpResponseLike + Send + Sync>,
    },
}

impl std::fmt::Debug for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Backend::Existing { base_url } => {
                write!(f, "Backend::Existing {{ base_url: {base_url:?} }}")
            }
            Backend::EmbeddedV8 { .. } => write!(f, "Backend::EmbeddedV8 {{ .. }}"),
            Backend::Stub { .. } => write!(f, "Backend::Stub {{ .. }}"),
        }
    }
}

impl Default for Backend {
    /// Default is `Existing { base_url: String::new() }`. Callers that
    /// actually dispatch requests should replace this with a meaningful
    /// backend before calling [`render_all`] or [`start`]. The default
    /// is provided so builders / test harnesses can use `..Default::default()`
    /// in struct literals without specifying every field.
    fn default() -> Self {
        Backend::Existing { base_url: String::new() }
    }
}

/// A live backend ready to dispatch render requests.
///
/// Returned by [`launch`] and owned by both the one-shot [`render_all`]
/// path and the long-lived [`RendererState`] path.
///
/// Variants map to the corresponding [`Backend`] variants:
/// - `Http` — existing HTTP server. Requests go through
///   `reqwest::blocking::Client` against `base_url`.
/// - `EmbeddedV8` — in-process V8 host. Requests are dispatched
///   directly without a TCP hop. The `guard` owns the `Box<dyn
///   EmbeddedV8Host>` so the drop contract (isolate destroy on panic)
///   is enforced even if `terminate()` is never called explicitly.
/// - `Stub` — unit-test closure. Requests are answered in-process by
///   the handler without any I/O.
enum BackendHandle {
    /// HTTP path: `Backend::Existing` (pre-running server).
    Http {
        base_url: String,
        client: reqwest::blocking::Client,
    },
    /// In-process V8 isolate (`EmbeddedV8RenderHost`).
    ///
    /// `guard` owns the host (`guard.host: Option<Box<dyn EmbeddedV8Host>>`).
    /// Dispatch calls `guard.host.as_mut().unwrap().dispatch_fetch(…)`.
    /// `terminate()` calls `guard.terminate()` which drops the host (and
    /// thus the V8 isolate) synchronously.
    EmbeddedV8 {
        guard: EmbeddedV8Guard,
    },
    /// Unit-test stub: answers requests with a closure.
    Stub {
        handler: Arc<dyn Fn(&str) -> HttpResponseLike + Send + Sync>,
    },
}

impl BackendHandle {
    fn dispatch(&mut self, url_path: &str) -> Result<HttpResponseLike, RendererError> {
        match self {
            BackendHandle::Http { base_url, client } => {
                let url = join_url(base_url, url_path);
                let resp = client
                    .get(&url)
                    .send()
                    .map_err(|e| RendererError::Http {
                        url: url.clone(),
                        source: e,
                    })?;
                let status = resp.status().as_u16();
                let content_type = resp
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                let body = resp
                    .bytes()
                    .map_err(|e| RendererError::Http {
                        url: url.clone(),
                        source: e,
                    })?
                    .to_vec();
                Ok(HttpResponseLike { status, content_type, body })
            }
            BackendHandle::EmbeddedV8 { guard } => {
                let host = guard.host.as_mut().expect(
                    "EmbeddedV8 host has been terminated; dispatch called after shutdown"
                );
                host.dispatch_fetch(url_path)
            }
            BackendHandle::Stub { handler } => {
                Ok(handler(url_path))
            }
        }
    }

    fn collect_logs(&self) -> String {
        match self {
            BackendHandle::Http { .. } => String::new(),
            BackendHandle::EmbeddedV8 { guard } => guard.collect_logs(),
            BackendHandle::Stub { .. } => String::new(),
        }
    }

    fn terminate(&mut self) {
        match self {
            BackendHandle::Http { .. } => {}
            BackendHandle::EmbeddedV8 { guard } => guard.terminate(),
            BackendHandle::Stub { .. } => {}
        }
    }
}

/// Inputs to [`render_all`].
#[derive(Debug, Clone)]
pub struct RendererInput {
    /// Path to the self-contained worker bundle. The bundle MUST be
    /// loadable as a workerd Module (`export default { fetch }`).
    /// In production this is the wrapped output of T3's bundler — the
    /// wrapper itself is generated by T7. The renderer only reads
    /// the path; it does not parse or modify the bundle.
    pub bundle_path: PathBuf,
    /// Companion `.map` next to the bundle. Used for stack-frame
    /// re-projection when the worker throws. May not exist when the
    /// caller ran the bundler in mock mode; the renderer tolerates a
    /// missing or unreadable file by skipping the re-projection
    /// (frames stay at `bundle.mjs:line:col`).
    pub sourcemap_path: PathBuf,
    /// Routes the bundle declares it can serve, from
    /// [`crate::bundler::BundlerOutput::manifest`]. Currently used for
    /// diagnostics only — the canonical list of *what to render* is
    /// `route_universe`. In a future iteration the renderer may
    /// cross-check that every prerendered URL maps to a declared route
    /// and produce a clearer error than a generic 404; that's
    /// out of scope for T6.
    pub manifest: BundleManifest,
    /// Where to write the SSG output. Created if missing.
    pub dist_dir: PathBuf,
    /// Every concrete URL the build wants on disk, after `paths()`
    /// expansion. SSR-only entries (whose `route_key` maps to
    /// `prerender == false` in [`RendererInput::prerender_map`]) are
    /// quietly partitioned into the [`SsrManifest`] and skipped.
    pub route_universe: Vec<RouteUniverseEntry>,
    /// Page-route template → `prerender` flag. Default behaviour
    /// (page is SSG) corresponds to `true`. A missing key is treated
    /// as `true` so that pages whose frontmatter extraction failed
    /// don't silently disappear from the build output.
    pub prerender_map: BTreeMap<String, bool>,
    /// Where the renderer goes for HTTP. See [`Backend`].
    pub backend: Backend,
    /// Optional cap on per-request HTTP timeout. Defaults to 60s.
    /// Build-time SSR can be slow (CPU-bound JSX renders), so this
    /// is intentionally generous; the dev loop overrides via
    /// [`RendererStartInput::request_timeout`].
    pub request_timeout: Option<Duration>,
    /// Prod-only head-asset injection switch.
    ///
    /// When `Some`, every successful HTML response is post-processed
    /// before atomic-write: a stable `<link rel="stylesheet">` tag and
    /// any number of `<script type="module">` tags are spliced
    /// immediately before the page's `</head>`. When `None`, HTML is
    /// written byte-for-byte as the worker emitted it — matching dev's
    /// existing behaviour. See [`crate::head_inject`] for the
    /// rewrite contract (passthrough on missing close tag, idempotent
    /// against the exact tag bytes, etc.).
    ///
    /// Dev mode (via [`start`] / [`render_one`]) does **not** plumb
    /// this field — the dev pipeline runs without `CssRunner` /
    /// `IslandsRunner` and would otherwise ship references to assets
    /// the dev server never emits.
    pub prod_head_assets: Option<crate::head_inject::ProdHeadAssets>,
}

/// Inputs to [`start`] (dev-mode long-lived state).
#[derive(Debug, Clone)]
pub struct RendererStartInput {
    /// See [`RendererInput::bundle_path`].
    pub bundle_path: PathBuf,
    /// See [`RendererInput::sourcemap_path`].
    pub sourcemap_path: PathBuf,
    /// See [`Backend`].
    pub backend: Backend,
    /// Per-request HTTP timeout. Defaults to 30s in dev (faster fail
    /// for the watch loop) when `None`.
    pub request_timeout: Option<Duration>,
}

// ---------------------------------------------------------------------------
// Public types — output
// ---------------------------------------------------------------------------

/// Output of [`render_all`].
#[derive(Debug, Clone)]
pub struct RendererOutput {
    /// Absolute paths of every file the renderer wrote, in
    /// `route_universe` order. Empty if every route was SSR-only.
    pub ssg_files_written: Vec<PathBuf>,
    /// SSR-only routes that the renderer deliberately skipped.
    /// T7 hands this to the runtime SSR adapter so a deployed worker
    /// knows which URLs to serve dynamically.
    pub ssr_manifest: SsrManifest,
    /// Diagnostic output from the embedded V8 host (console output
    /// captured during rendering). Empty when there was no output.
    pub runtime_logs: String,
}

/// Routes the build deliberately did NOT prerender.
///
/// Field shape is deliberately small — T7 picks it up and either
/// embeds it in a manifest the runtime adapter consults, or hands it
/// to a deployment helper that wires up dynamic-route handlers.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SsrManifest {
    /// SSR-only entries from the route universe.
    pub routes: Vec<SsrRouteEntry>,
}

/// A single SSR-only route.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SsrRouteEntry {
    /// Page-route template (the value of
    /// [`RouteUniverseEntry::route_key`]).
    pub route_key: String,
    /// Concrete URL the runtime handler should serve.
    pub url_path: String,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Top-level error type.
///
/// Variants are deliberately narrow so T7 can pattern-match on
/// `RenderFailed` (the most common operator-facing variant) versus
/// infrastructure errors (`Io`, `EmbeddedV8`, …).
#[derive(Debug, Error)]
pub enum RendererError {
    /// I/O error around dist write or bundle read.
    #[error("renderer I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// HTTP layer (reqwest) failure independent of the worker — DNS,
    /// connection refused, etc.
    #[error("http request to {url} failed: {source}")]
    Http {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    /// The worker returned a non-2xx response. `user_location` is
    /// populated when the source-map walk succeeded.
    #[error("render failed for {url} (status {status}): {body}{location}",
        location = user_location.as_ref().map(|l| format!(" — at {l}")).unwrap_or_default(),
    )]
    RenderFailed {
        url: String,
        status: u16,
        body: String,
        /// `Some("pages/foo.tsx:42:10")` if a sourcemap re-projection
        /// resolved a frame in the response body. `None` when no map
        /// was readable, or no frame in the body matched the bundle.
        user_location: Option<String>,
    },
    /// The in-process V8 host encountered an infrastructure-level error
    /// (isolate crash, OOM, module load failure, etc.). Non-2xx responses
    /// from the bundle's `fetch` handler are surfaced as
    /// [`RendererError::RenderFailed`], not as this variant.
    #[error("embedded V8 host error: {0}")]
    EmbeddedV8(String),
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Render the entire SSG portion of `route_universe` to disk.
///
/// Constructs the embedded V8 host (or uses the supplied HTTP base URL),
/// drives the request loop, writes files, tears the backend down. Always
/// cleans up on error or panic via the guard's drop impl.
pub fn render_all(input: RendererInput) -> Result<RendererOutput, RendererError> {
    let RendererInput {
        bundle_path,
        sourcemap_path,
        manifest: _manifest,
        dist_dir,
        route_universe,
        prerender_map,
        backend,
        request_timeout,
        prod_head_assets,
    } = input;

    fs::create_dir_all(&dist_dir).map_err(|e| RendererError::Io {
        path: dist_dir.clone(),
        source: e,
    })?;

    // Partition: SSG (render now) vs SSR (defer to runtime).
    let mut ssg_routes: Vec<RouteUniverseEntry> = Vec::new();
    let mut ssr_routes: Vec<SsrRouteEntry> = Vec::new();
    for entry in route_universe {
        let prerender = prerender_map
            .get(&entry.route_key)
            .copied()
            .unwrap_or(true);
        if prerender {
            ssg_routes.push(entry);
        } else {
            ssr_routes.push(SsrRouteEntry {
                route_key: entry.route_key,
                url_path: entry.url_path,
            });
        }
    }

    let timeout = request_timeout.unwrap_or(Duration::from_secs(60));
    let mut handle = launch(&backend, &bundle_path, timeout)?;
    let sourcemap = load_sourcemap(&sourcemap_path);

    let mut written: Vec<PathBuf> = Vec::with_capacity(ssg_routes.len());
    let mut last_err: Option<RendererError> = None;
    for entry in &ssg_routes {
        match render_one_inner(
            &mut handle,
            entry,
            &dist_dir,
            sourcemap.as_ref(),
            prod_head_assets.as_ref(),
        ) {
            Ok(path) => written.push(path),
            Err(e) => {
                last_err = Some(e);
                break;
            }
        }
    }

    let logs = handle.collect_logs();
    handle.terminate();
    drop(handle);

    if let Some(err) = last_err {
        // Surface embedded V8 host logs so the operator can see what
        // the worker printed before the 500. Without this, the logs
        // are discarded and the user sees only "Internal Server Error"
        // with no context.
        if !logs.trim().is_empty() {
            eprintln!("[zfb] backend logs at render failure:\n{logs}");
        }
        return Err(err);
    }

    Ok(RendererOutput {
        ssg_files_written: written,
        ssr_manifest: SsrManifest { routes: ssr_routes },
        runtime_logs: logs,
    })
}

/// Long-lived dev-mode renderer state.
///
/// Owns the backend handle (embedded V8 host or existing HTTP server)
/// and the parsed sourcemap. Use [`start`] to construct, [`render_one`]
/// to drive a single route, [`shutdown`] to tear down cleanly. Drop
/// also tears down (idempotent) so a panicking dev loop still cleans up.
pub struct RendererState {
    sourcemap: Option<sourcemap::SourceMap>,
    handle: BackendHandle,
}

impl std::fmt::Debug for RendererState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RendererState")
            .field("has_sourcemap", &self.sourcemap.is_some())
            .field("handle", &self.handle)
            .finish()
    }
}

impl std::fmt::Debug for BackendHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendHandle::Http { base_url, .. } => {
                write!(f, "BackendHandle::Http {{ base_url: {base_url:?} }}")
            }
            BackendHandle::EmbeddedV8 { .. } => write!(f, "BackendHandle::EmbeddedV8 {{ .. }}"),
            BackendHandle::Stub { .. } => write!(f, "BackendHandle::Stub {{ .. }}"),
        }
    }
}

impl RendererState {
    /// The base URL of the running HTTP server (e.g.
    /// `http://127.0.0.1:54321/`). Returns `None` for
    /// `Backend::EmbeddedV8` and `Backend::Stub`, where there is no
    /// HTTP server. Use this with [`Backend::Existing`] to reuse the
    /// same server for a subsequent [`render_all`] call.
    pub fn base_url(&self) -> Option<&str> {
        match &self.handle {
            BackendHandle::Http { base_url, .. } => Some(base_url),
            _ => None,
        }
    }

    /// Borrow the embedded V8 host mutably so callers can dispatch
    /// fetch requests directly (e.g. from `eval_deferred_paths`).
    ///
    /// Returns `None` when the backend is not `EmbeddedV8` (i.e. when
    /// it is `Http` or `Stub`), or when the host has already been
    /// terminated via [`shutdown`].
    pub fn embedded_v8_host_mut(&mut self) -> Option<&mut dyn EmbeddedV8Host> {
        match &mut self.handle {
            BackendHandle::EmbeddedV8 { guard } => {
                guard.host.as_deref_mut().map(|h| h as &mut dyn EmbeddedV8Host)
            }
            _ => None,
        }
    }
}

/// Start the dev-mode renderer.
pub fn start(input: RendererStartInput) -> Result<RendererState, RendererError> {
    let RendererStartInput {
        bundle_path,
        sourcemap_path,
        backend,
        request_timeout,
    } = input;
    let timeout = request_timeout.unwrap_or(Duration::from_secs(30));
    let handle = launch(&backend, &bundle_path, timeout)?;
    let sourcemap = load_sourcemap(&sourcemap_path);
    Ok(RendererState {
        sourcemap,
        handle,
    })
}

/// Drive one route against an existing dev-mode state and write it to
/// disk under `dist_dir`.
pub fn render_one(
    state: &mut RendererState,
    entry: &RouteUniverseEntry,
    dist_dir: &Path,
) -> Result<PathBuf, RendererError> {
    fs::create_dir_all(dist_dir).map_err(|e| RendererError::Io {
        path: dist_dir.to_path_buf(),
        source: e,
    })?;
    render_one_inner(
        &mut state.handle,
        entry,
        dist_dir,
        state.sourcemap.as_ref(),
        // Dev mode never injects prod head assets — see the
        // `prod_head_assets` field doc on `RendererInput`.
        None,
    )
}

/// Tear the dev-mode renderer down. Idempotent — calling on an
/// already-shut-down state is a no-op.
pub fn shutdown(state: RendererState) -> Result<(), RendererError> {
    let RendererState { mut handle, .. } = state;
    handle.terminate();
    Ok(())
}

/// Reload the dev-mode renderer against a (possibly new) bundle.
///
/// The old embedded V8 isolate is dropped (destroyed) and a new one is
/// created from the new bundle path. The "destroy + recreate" pattern is
/// the simplest correct approach; module-re-evaluation as a hot-reload
/// optimisation is out of scope for this epic.
///
/// **Reload latency expectation:** the embedded V8 host destroy + recreate
/// is expected to take 200–800 ms per bundle because every reload
/// re-parses the bundle. This is accepted for v1.
///
/// Callers should invoke this whenever a TSX page edit, a layout edit, or
/// an exported handler change has rebuilt the worker bundle on disk —
/// typically driven from the dev pipeline's reload-renderer hook (see
/// [`crate::pipeline::BuildContext::reload_renderer`]).
///
/// The returned [`RendererState`] takes ownership of the new backend. The
/// previous state is consumed by value to make the "old backend is gone"
/// invariant statically obvious.
///
/// `request_timeout` defaults to 30s when `None` (same as [`start`]).
///
/// On `Backend::Existing`, this is effectively a no-op restart: it
/// rebuilds the [`reqwest`] client and re-reads the source map without
/// touching any host. That keeps the dev pipeline's "always reload before
/// render" code path correct under the `Backend::Stub` test path too.
pub fn reload(
    previous: RendererState,
    input: RendererStartInput,
) -> Result<RendererState, RendererError> {
    // Dropping the previous state via `shutdown` happens first so the old
    // backend is fully torn down before we construct the new one. With
    // `Backend::Existing` the call is a cheap no-op. With `Backend::EmbeddedV8`
    // it drops the V8 isolate synchronously before the new one is created.
    shutdown(previous)?;
    start(input)
}

// ---------------------------------------------------------------------------
// Internals — request loop
// ---------------------------------------------------------------------------

fn build_http_client(timeout: Duration) -> Result<reqwest::blocking::Client, RendererError> {
    // `reqwest::blocking::Client::builder().build()` spins up its own
    // dedicated tokio runtime under the hood and synchronously waits
    // on it. When the surrounding code is itself running inside an
    // async runtime (zfb's CLI uses `tokio::main` to drive
    // `commands::build::run`), the inner runtime's drop path panics
    // with `Cannot drop a runtime in a context where blocking is not
    // allowed`. Constructing the client on a fresh OS thread gives
    // reqwest a clean, runtime-free environment.
    std::thread::scope(|s| {
        s.spawn(|| {
            reqwest::blocking::Client::builder()
                .timeout(timeout)
                // We hit 127.0.0.1; no system proxy can reach
                // loopback usefully and would only ever break the
                // build.
                .no_proxy()
                .build()
        })
        .join()
        .expect("client-builder thread panicked")
    })
    .map_err(|e| RendererError::Http {
        url: "<client-builder>".into(),
        source: e,
    })
}

/// Dispatch one route request through `handle` and write the result to
/// disk. Works uniformly across HTTP (existing server), embedded V8,
/// and stub backends.
fn render_one_inner(
    handle: &mut BackendHandle,
    entry: &RouteUniverseEntry,
    dist_dir: &Path,
    sourcemap: Option<&sourcemap::SourceMap>,
    prod_head_assets: Option<&crate::head_inject::ProdHeadAssets>,
) -> Result<PathBuf, RendererError> {
    let resp = handle.dispatch(&entry.url_path)?;
    let status = resp.status;
    let body = resp.body;

    // The URL used in error messages is the rendered path. For HTTP
    // backends the full URL is available inside `BackendHandle::Http`;
    // for EmbeddedV8 / Stub we synthesise one for readability.
    let url_for_err = entry.url_path.clone();

    if !(200..300).contains(&status) {
        let body_str = String::from_utf8_lossy(&body).into_owned();
        let user_location = sourcemap.and_then(|sm| reproject_first_frame(&body_str, sm));
        return Err(RendererError::RenderFailed {
            url: url_for_err,
            status,
            body: body_str,
            user_location,
        });
    }
    // Validate `output_path` before joining: the value comes from the
    // route universe (router + page modules), but a malformed entry
    // could carry an absolute or `..`-escaping relative path. Reject
    // those at the write boundary so a hostile or buggy page module
    // cannot corrupt files outside dist.
    let dest = crate::atomic::validate_output_path(dist_dir, &entry.output_path).map_err(|e| {
        RendererError::Io {
            path: entry.output_path.clone(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()),
        }
    })?;
    // Prod-only head injection. When `prod_head_assets` is `Some` the
    // helper splices `<link>` / `<script type="module">` tags before
    // `</head>`. Non-HTML output (no `</head>`) and dev mode (no
    // `prod_head_assets`) round-trip unchanged.
    //
    // We only attempt the rewrite when the body parses as UTF-8; binary
    // payloads (rare — favicons via the route universe, etc.) bypass
    // the rewriter and are written verbatim. `from_utf8` on bytes the
    // client returned is cheap because `reqwest` does not validate.
    let written_bytes: std::borrow::Cow<'_, [u8]> = match prod_head_assets {
        Some(assets) if !assets.is_empty() => match std::str::from_utf8(&body) {
            Ok(text) => match crate::head_inject::inject_prod_head_assets(text, assets) {
                std::borrow::Cow::Borrowed(_) => std::borrow::Cow::Borrowed(body.as_ref()),
                std::borrow::Cow::Owned(s) => std::borrow::Cow::Owned(s.into_bytes()),
            },
            Err(_) => std::borrow::Cow::Borrowed(body.as_ref()),
        },
        _ => std::borrow::Cow::Borrowed(body.as_ref()),
    };
    // Atomic write is consistent with both pipelines (dev and prod) and
    // prevents a half-written .html file from being observed if the
    // process is killed mid-write. atomic_write also creates the parent
    // directory internally, so we don't need a separate
    // `create_dir_all` call.
    crate::atomic::atomic_write(&dest, &written_bytes).map_err(|e| RendererError::Io {
        path: dest.clone(),
        source: std::io::Error::other(format!("{e:#}")),
    })?;
    Ok(dest)
}

fn join_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    if path.starts_with('/') {
        format!("{base}{path}")
    } else {
        format!("{base}/{path}")
    }
}

// ---------------------------------------------------------------------------
// Internals — embedded V8 guard
// ---------------------------------------------------------------------------

/// RAII guard for an in-process V8 host.
///
/// Dropping the `Box<dyn EmbeddedV8Host>` destroys the V8 isolate
/// synchronously. The guard's sole responsibility is ensuring the host
/// is dropped (and thus the isolate destroyed) even on panic.
///
/// `terminate()` is idempotent — calling it after the host has already
/// been taken out is a no-op.
struct EmbeddedV8Guard {
    host: Option<Box<dyn EmbeddedV8Host>>,
}

impl EmbeddedV8Guard {
    fn new(host: Box<dyn EmbeddedV8Host>) -> Self {
        Self { host: Some(host) }
    }

    fn terminate(&mut self) {
        // Drops the V8 isolate synchronously. Subsequent dispatch calls
        // through BackendHandle::EmbeddedV8 will find `guard.host = None`
        // and panic — that is intentional: terminate() is only called at
        // shutdown, never during active rendering.
        self.host.take();
    }

    /// Collect diagnostic output from the host. The embedded V8 host
    /// captures console output internally; this surfaces it as a string
    /// for inclusion in error messages. Returns an empty string when the
    /// host has already been terminated or when it produced no output.
    fn collect_logs(&self) -> String {
        // The embedded host captures console output through the V8
        // console extension. The exact retrieval API is not yet
        // defined; for now we return an empty string. Once the host
        // exposes a `drain_console_logs() -> String` method on the
        // trait, add it to `EmbeddedV8Host` and call it here.
        String::new()
    }
}

impl Drop for EmbeddedV8Guard {
    fn drop(&mut self) {
        self.terminate();
    }
}

/// Construct the active [`BackendHandle`] for this render job.
///
/// For `Backend::Existing` this creates the reqwest client only.
/// For `EmbeddedV8` it calls the user-supplied factory to construct the
/// in-process V8 host. For `Stub` it simply wraps the closure.
fn launch(
    backend: &Backend,
    bundle_path: &Path,
    timeout: Duration,
) -> Result<BackendHandle, RendererError> {
    match backend {
        Backend::Existing { base_url } => {
            let client = build_http_client(timeout)?;
            Ok(BackendHandle::Http {
                base_url: base_url.clone(),
                client,
            })
        }
        Backend::EmbeddedV8 { host_factory } => {
            // Resolve the bundle path to an absolute path before calling
            // the factory, so the factory impl never needs to handle
            // relative paths.
            let abs_bundle = if bundle_path.is_absolute() {
                bundle_path.to_path_buf()
            } else {
                std::env::current_dir()
                    .map_err(|e| RendererError::Io {
                        path: bundle_path.to_path_buf(),
                        source: e,
                    })?
                    .join(bundle_path)
            };
            let host = host_factory(&abs_bundle)?;
            let guard = EmbeddedV8Guard::new(host);
            Ok(BackendHandle::EmbeddedV8 { guard })
        }
        Backend::Stub { handler } => Ok(BackendHandle::Stub {
            handler: handler.clone(),
        }),
    }
}

// ---------------------------------------------------------------------------
// Internals — sourcemap re-projection
// ---------------------------------------------------------------------------

fn load_sourcemap(path: &Path) -> Option<sourcemap::SourceMap> {
    let raw = fs::read(path).ok()?;
    sourcemap::SourceMap::from_reader(raw.as_slice()).ok()
}

/// Walk the response body for the first `bundle.mjs:LINE:COL` style
/// frame and re-project it through the source map. Returns
/// `"<source>:line:col"` (1-based line numbers) when we find one,
/// `None` otherwise.
///
/// We deliberately only project the first frame: build errors usually
/// originate in user code and the deepest user frame is typically the
/// first in a workerd traceback. A multi-frame walk is reserved for
/// the more sophisticated diagnostics layer T7+ may add.
fn reproject_first_frame(body: &str, sm: &sourcemap::SourceMap) -> Option<String> {
    // Walk every candidate frame, not just the first parsed one: the
    // first frame in a workerd traceback is often `at fetch
    // (worker.js:1:1)` (the synthetic harness entry), not the user's
    // code. Take the first candidate that *resolves* to a source
    // mapping. If none resolves we return None and the caller leaves
    // `user_location` unset.
    find_frame_candidates(body).into_iter().find_map(|cap| {
        let token = sm.lookup_token(cap.line.saturating_sub(1), cap.col.saturating_sub(1))?;
        let source = token
            .get_source()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "<unknown>".to_string());
        Some(format!(
            "{source}:{line}:{col}",
            line = token.get_src_line() + 1,
            col = token.get_src_col() + 1,
        ))
    })
}

#[derive(Debug)]
struct FrameCandidate {
    line: u32,
    col: u32,
}

/// Naive parser for `bundle.mjs:LINE:COL` substrings. Doesn't try to
/// fully parse v8 / workerd stack frames; we only need a line+col
/// hint that points at the bundle.
fn find_frame_candidates(body: &str) -> Vec<FrameCandidate> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            // Read the digit run.
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            // Expect ':'
            if i >= bytes.len() || bytes[i] != b':' {
                continue;
            }
            let mid = i + 1;
            let mut j = mid;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j == mid {
                continue;
            }
            let line: u32 = body[start..i].parse().unwrap_or(0);
            let col: u32 = body[mid..j].parse().unwrap_or(0);
            // Require the digit pair to be preceded by something that
            // looks like a path. We're strict-ish: insist on a `.mjs`
            // or `.js` immediately before, OR explicitly `bundle`.
            // This dodges random `key:value` pairs like `status: 500`.
            let prefix = &body[..start];
            let looks_like_frame = prefix.ends_with(".mjs:")
                || prefix.ends_with(".js:")
                || prefix.ends_with("bundle:");
            if line > 0 && col > 0 && looks_like_frame {
                out.push(FrameCandidate { line, col });
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Build a [`Backend::Stub`] from a closure that maps URL path → response.
    /// The closure signature mirrors the public [`HttpResponseLike`] shape.
    fn stub_backend(
        f: impl Fn(&str) -> HttpResponseLike + Send + Sync + 'static,
    ) -> Backend {
        Backend::Stub {
            handler: Arc::new(f),
        }
    }

    /// Build a stub response with status 200 and `text/html` content type.
    fn html_ok(body: &'static str) -> HttpResponseLike {
        HttpResponseLike {
            status: 200,
            content_type: "text/html; charset=utf-8".into(),
            body: body.as_bytes().to_vec(),
        }
    }

    fn dummy_manifest() -> BundleManifest {
        BundleManifest {
            framework: "preact".into(),
            jsx_import_source: "preact".into(),
            hydrate_shim_specifier: "zfb:internal/preact/hydrate".into(),
            bundle_basename: "bundle.mjs".into(),
            routes: vec![],
        }
    }

    #[test]
    fn render_all_partitions_ssg_vs_ssr_and_writes_files() {
        let backend = stub_backend(|path| match path {
            "/" => HttpResponseLike {
                status: 200,
                content_type: "text/html; charset=utf-8".into(),
                body: b"<html><body><h1>Home</h1></body></html>".to_vec(),
            },
            "/about" => HttpResponseLike {
                status: 200,
                content_type: "text/html; charset=utf-8".into(),
                body: b"<html><body>About</body></html>".to_vec(),
            },
            "/feed.xml" => HttpResponseLike {
                status: 200,
                content_type: "application/xml".into(),
                body: b"<rss/>".to_vec(),
            },
            _ => HttpResponseLike {
                status: 404,
                content_type: "text/plain".into(),
                body: b"nope".to_vec(),
            },
        });

        let dist = tempfile::tempdir().unwrap();
        let mut prerender_map = BTreeMap::new();
        // /preview is SSR-only; the renderer must SKIP it.
        prerender_map.insert("/preview".to_string(), false);
        // / and /about default to SSG (no entry needed; absent → true).
        // /feed.xml: explicit true.
        prerender_map.insert("/feed.xml".to_string(), true);

        let universe = vec![
            RouteUniverseEntry {
                url_path: "/".into(),
                output_path: PathBuf::from("index.html"),
                route_key: "/".into(),
            },
            RouteUniverseEntry {
                url_path: "/about".into(),
                output_path: PathBuf::from("about/index.html"),
                route_key: "/about".into(),
            },
            RouteUniverseEntry {
                url_path: "/feed.xml".into(),
                output_path: PathBuf::from("feed.xml"),
                route_key: "/feed.xml".into(),
            },
            RouteUniverseEntry {
                url_path: "/preview".into(),
                output_path: PathBuf::from("preview/index.html"),
                route_key: "/preview".into(),
            },
        ];

        let out = render_all(RendererInput {
            bundle_path: PathBuf::from("/dev/null"),
            sourcemap_path: PathBuf::from("/dev/null"),
            manifest: dummy_manifest(),
            dist_dir: dist.path().to_path_buf(),
            route_universe: universe,
            prerender_map,
            backend,
            request_timeout: None,
            prod_head_assets: None,
        })
        .expect("render_all");

        // SSG side: 3 files written in input order.
        assert_eq!(out.ssg_files_written.len(), 3);
        let index = fs::read_to_string(dist.path().join("index.html")).unwrap();
        assert!(index.contains("Home"));
        let about = fs::read_to_string(dist.path().join("about/index.html")).unwrap();
        assert!(about.contains("About"));
        let feed = fs::read_to_string(dist.path().join("feed.xml")).unwrap();
        assert!(feed.contains("<rss/>"));
        // Sentinel: zero pages contain the v0 stub string.
        for p in &out.ssg_files_written {
            let body = fs::read_to_string(p).unwrap();
            assert!(
                !body.contains("<h1>zfb build (v1 stub)</h1>"),
                "v0 stub leaked into {}",
                p.display()
            );
        }
        // SSR side: only /preview.
        assert_eq!(out.ssr_manifest.routes.len(), 1);
        assert_eq!(out.ssr_manifest.routes[0].url_path, "/preview");
    }

    #[test]
    fn missing_prerender_entry_defaults_to_ssg() {
        let dist = tempfile::tempdir().unwrap();
        let universe = vec![RouteUniverseEntry {
            url_path: "/".into(),
            output_path: PathBuf::from("index.html"),
            route_key: "/".into(),
        }];
        let out = render_all(RendererInput {
            bundle_path: PathBuf::from("/dev/null"),
            sourcemap_path: PathBuf::from("/dev/null"),
            manifest: dummy_manifest(),
            dist_dir: dist.path().to_path_buf(),
            route_universe: universe,
            prerender_map: BTreeMap::new(),
            backend: stub_backend(|_| html_ok("<html>ok</html>")),
            request_timeout: None,
            prod_head_assets: None,
        })
        .unwrap();
        assert_eq!(out.ssg_files_written.len(), 1);
        assert!(out.ssr_manifest.routes.is_empty());
    }

    #[test]
    fn non_2xx_response_surfaces_render_failed_with_body() {
        let dist = tempfile::tempdir().unwrap();
        let universe = vec![RouteUniverseEntry {
            url_path: "/error".into(),
            output_path: PathBuf::from("error/index.html"),
            route_key: "/error".into(),
        }];
        let err = render_all(RendererInput {
            bundle_path: PathBuf::from("/dev/null"),
            sourcemap_path: PathBuf::from("/dev/null"),
            manifest: dummy_manifest(),
            dist_dir: dist.path().to_path_buf(),
            route_universe: universe,
            prerender_map: BTreeMap::new(),
            backend: stub_backend(|_| HttpResponseLike {
                status: 500,
                content_type: "text/plain".into(),
                body: b"Error: boom\n  at fetch (bundle.mjs:42:7)\n".to_vec(),
            }),
            request_timeout: None,
            prod_head_assets: None,
        })
        .unwrap_err();
        match err {
            RendererError::RenderFailed { status, body, .. } => {
                assert_eq!(status, 500);
                assert!(body.contains("Error: boom"));
            }
            other => unreachable!("expected RenderFailed, got {other:?}"),
        }
    }

    #[test]
    fn render_all_without_prod_head_assets_writes_html_byte_for_byte() {
        // Dev-no-regression assertion: when `prod_head_assets` is None,
        // the renderer must write the worker's HTML response to disk
        // unchanged. A future regression that inadvertently always
        // injects (e.g. a default-Some) would trip this.
        let raw_html = b"<!doctype html><html><head><title>T</title></head><body>x</body></html>";
        let raw_html_owned = raw_html.to_vec();
        let dist = tempfile::tempdir().unwrap();
        let universe = vec![RouteUniverseEntry {
            url_path: "/".into(),
            output_path: PathBuf::from("index.html"),
            route_key: "/".into(),
        }];
        render_all(RendererInput {
            bundle_path: PathBuf::from("/dev/null"),
            sourcemap_path: PathBuf::from("/dev/null"),
            manifest: dummy_manifest(),
            dist_dir: dist.path().to_path_buf(),
            route_universe: universe,
            prerender_map: BTreeMap::new(),
            backend: Backend::Stub {
                handler: Arc::new(move |_| HttpResponseLike {
                    status: 200,
                    content_type: "text/html; charset=utf-8".into(),
                    body: raw_html_owned.clone(),
                }),
            },
            request_timeout: None,
            prod_head_assets: None,
        })
        .expect("render_all");
        let written = fs::read(dist.path().join("index.html")).unwrap();
        assert_eq!(written, raw_html);
    }

    #[test]
    fn render_all_with_prod_head_assets_injects_link_and_script() {
        let raw_html = b"<!doctype html><html><head><title>T</title></head><body>x</body></html>";
        let raw_html_owned = raw_html.to_vec();
        let dist = tempfile::tempdir().unwrap();
        let universe = vec![RouteUniverseEntry {
            url_path: "/".into(),
            output_path: PathBuf::from("index.html"),
            route_key: "/".into(),
        }];
        let assets = crate::head_inject::ProdHeadAssets {
            css_url: Some("/assets/styles.css".into()),
            island_module_urls: vec!["/assets/islands.js".into()],
        };
        render_all(RendererInput {
            bundle_path: PathBuf::from("/dev/null"),
            sourcemap_path: PathBuf::from("/dev/null"),
            manifest: dummy_manifest(),
            dist_dir: dist.path().to_path_buf(),
            route_universe: universe,
            prerender_map: BTreeMap::new(),
            backend: Backend::Stub {
                handler: Arc::new(move |_| HttpResponseLike {
                    status: 200,
                    content_type: "text/html; charset=utf-8".into(),
                    body: raw_html_owned.clone(),
                }),
            },
            request_timeout: None,
            prod_head_assets: Some(assets),
        })
        .expect("render_all");
        let written = fs::read_to_string(dist.path().join("index.html")).unwrap();
        let close_at = written.find("</head>").unwrap();
        let link_at = written.find("<link rel=\"stylesheet\"").unwrap();
        let script_at = written.find("src=\"/assets/islands.js\"").unwrap();
        assert!(link_at < close_at);
        assert!(script_at < close_at);
        assert!(written.contains("<title>T</title>"));
        assert!(written.contains("<body>x</body>"));
    }

    #[test]
    fn render_all_passes_through_non_html_routes_when_assets_present() {
        // `feed.xml` carries no `</head>` — head_inject must passthrough
        // even when `prod_head_assets` is Some. Critical: the route
        // universe in real builds mixes HTML and non-HTML outputs.
        let xml_body = b"<?xml version=\"1.0\"?><rss/>";
        let xml_owned = xml_body.to_vec();
        let dist = tempfile::tempdir().unwrap();
        let universe = vec![RouteUniverseEntry {
            url_path: "/feed.xml".into(),
            output_path: PathBuf::from("feed.xml"),
            route_key: "/feed.xml".into(),
        }];
        let assets = crate::head_inject::ProdHeadAssets {
            css_url: Some("/assets/styles.css".into()),
            island_module_urls: vec![],
        };
        render_all(RendererInput {
            bundle_path: PathBuf::from("/dev/null"),
            sourcemap_path: PathBuf::from("/dev/null"),
            manifest: dummy_manifest(),
            dist_dir: dist.path().to_path_buf(),
            route_universe: universe,
            prerender_map: BTreeMap::new(),
            backend: Backend::Stub {
                handler: Arc::new(move |_| HttpResponseLike {
                    status: 200,
                    content_type: "application/xml".into(),
                    body: xml_owned.clone(),
                }),
            },
            request_timeout: None,
            prod_head_assets: Some(assets),
        })
        .expect("render_all");
        let written = fs::read(dist.path().join("feed.xml")).unwrap();
        assert_eq!(written, xml_body);
    }

    #[test]
    fn sourcemap_reprojection_finds_user_file() {
        // Hand-roll a sourcemap that maps bundle line 1 col 1 → src
        // pages/error.tsx line 5 col 3. The simplest way to do this
        // robustly across `sourcemap` crate versions is to use its
        // builder API.
        let mut builder = sourcemap::SourceMapBuilder::new(None);
        let src_id = builder.add_source("pages/error.tsx");
        builder.set_source_contents(src_id, Some("export function boom() { throw new Error('x'); }"));
        // dst (generated) line 0 col 0 maps to src (original) line 4
        // col 2 (both 0-based in the builder). That corresponds to
        // user-visible line 5 col 3.
        builder.add_raw(0, 0, 4, 2, Some(src_id), None, false);
        let sm = builder.into_sourcemap();

        // Body that mentions the bundle frame.
        let body = "TypeError: boom\n  at fetch (bundle.mjs:1:1)\n";
        let projected = reproject_first_frame(body, &sm).expect("reprojection");
        assert!(
            projected.starts_with("pages/error.tsx:5:"),
            "got {projected}"
        );
    }

    #[test]
    fn sourcemap_missing_file_does_not_panic() {
        let sm = load_sourcemap(Path::new("/no/such/path"));
        assert!(sm.is_none());
    }

    #[test]
    fn join_url_handles_trailing_and_leading_slashes() {
        assert_eq!(
            join_url("http://127.0.0.1:1234/", "/about"),
            "http://127.0.0.1:1234/about"
        );
        assert_eq!(
            join_url("http://127.0.0.1:1234", "/about"),
            "http://127.0.0.1:1234/about"
        );
        assert_eq!(
            join_url("http://127.0.0.1:1234/", "about"),
            "http://127.0.0.1:1234/about"
        );
    }

    #[test]
    fn render_all_includes_user_location_when_bundle_throws() {
        // Compose a sourcemap on disk that maps bundle line 1 col 1
        // to pages/error.tsx line 5 col 3.
        let tmp = tempfile::tempdir().unwrap();
        let bundle_path = tmp.path().join("bundle.mjs");
        let map_path = tmp.path().join("bundle.mjs.map");
        fs::write(&bundle_path, "// not loaded by this test\n").unwrap();

        let mut builder = sourcemap::SourceMapBuilder::new(None);
        let src_id = builder.add_source("pages/error.tsx");
        builder.set_source_contents(src_id, Some("export function boom() { throw new Error('x'); }"));
        builder.add_raw(0, 0, 4, 2, Some(src_id), None, false);
        let mut buf = Vec::new();
        builder.into_sourcemap().to_writer(&mut buf).unwrap();
        fs::write(&map_path, &buf).unwrap();

        // Backend::Stub simulates a bundle-thrown error with a stack frame
        // pointing at bundle.mjs:1:1. This is the key acceptance criterion
        // for this sub: renderer logic stays testable without booting a real
        // V8 isolate.
        let dist = tempfile::tempdir().unwrap();
        let universe = vec![RouteUniverseEntry {
            url_path: "/".into(),
            output_path: PathBuf::from("index.html"),
            route_key: "/".into(),
        }];

        let err = render_all(RendererInput {
            bundle_path,
            sourcemap_path: map_path,
            manifest: dummy_manifest(),
            dist_dir: dist.path().to_path_buf(),
            route_universe: universe,
            prerender_map: BTreeMap::new(),
            backend: stub_backend(|_| HttpResponseLike {
                status: 500,
                content_type: "text/plain".into(),
                body: b"Error: explode\n  at fetch (bundle.mjs:1:1)\n".to_vec(),
            }),
            request_timeout: None,
            prod_head_assets: None,
        })
        .unwrap_err();

        match err {
            RendererError::RenderFailed { user_location, body, .. } => {
                let loc = user_location.expect("expected user_location to be projected");
                assert!(
                    loc.starts_with("pages/error.tsx:5:"),
                    "expected pages/error.tsx:5:* got {loc}; body={body}"
                );
            }
            other => unreachable!("expected RenderFailed, got {other:?}"),
        }
    }

    /// Key acceptance criterion from the spec: `Backend::Stub` simulates a
    /// non-2xx response carrying a `bundle.mjs:line:col` frame and the
    /// renderer asserts `user_location: Some("pages/foo.tsx:42:10")`
    /// re-projection. Tests renderer logic end-to-end without V8.
    #[test]
    fn stub_backend_sourcemap_reprojection_user_location() {
        let tmp = tempfile::tempdir().unwrap();
        let map_path = tmp.path().join("bundle.mjs.map");

        // Map bundle line 42 col 10 → pages/foo.tsx line 42 col 10.
        // (Builder uses 0-based; user-visible is 1-based.)
        let mut builder = sourcemap::SourceMapBuilder::new(None);
        let src_id = builder.add_source("pages/foo.tsx");
        builder.set_source_contents(src_id, Some("// foo"));
        // bundle dst line 41 col 9 (0-based) → pages/foo.tsx line 41 col 9
        builder.add_raw(41, 9, 41, 9, Some(src_id), None, false);
        let mut buf = Vec::new();
        builder.into_sourcemap().to_writer(&mut buf).unwrap();
        fs::write(&map_path, &buf).unwrap();

        let dist = tempfile::tempdir().unwrap();
        let universe = vec![RouteUniverseEntry {
            url_path: "/foo".into(),
            output_path: PathBuf::from("foo/index.html"),
            route_key: "/foo".into(),
        }];

        let err = render_all(RendererInput {
            bundle_path: PathBuf::from("/dev/null"),
            sourcemap_path: map_path,
            manifest: dummy_manifest(),
            dist_dir: dist.path().to_path_buf(),
            route_universe: universe,
            prerender_map: BTreeMap::new(),
            backend: stub_backend(|_| HttpResponseLike {
                status: 500,
                content_type: "text/plain".into(),
                body: b"RenderError: oops\n  at render (bundle.mjs:42:10)\n".to_vec(),
            }),
            request_timeout: None,
            prod_head_assets: None,
        })
        .unwrap_err();

        match err {
            RendererError::RenderFailed { user_location, .. } => {
                let loc = user_location.expect("user_location must be set");
                assert!(
                    loc.starts_with("pages/foo.tsx:42:"),
                    "expected pages/foo.tsx:42:* got {loc}"
                );
            }
            other => unreachable!("expected RenderFailed, got {other:?}"),
        }
    }

    #[test]
    fn reload_swaps_renderer_state_via_stub_backend() {
        // Reload semantics: destroy old backend, construct new one.
        // Under Backend::Stub this is zero-cost (no subprocess) but
        // confirms the sequence: start → reload → render_one → shutdown.
        let tmp = tempfile::tempdir().unwrap();
        let bundle_path = tmp.path().join("bundle.mjs");
        fs::write(&bundle_path, "// dev bundle\n").unwrap();
        let map_path = tmp.path().join("bundle.mjs.map");

        let state = start(RendererStartInput {
            bundle_path: bundle_path.clone(),
            sourcemap_path: map_path.clone(),
            backend: stub_backend(|_| html_ok("<p>before</p>")),
            request_timeout: None,
        })
        .expect("initial start");

        // The reload consumes the previous state and returns a fresh one.
        // We then drive render_one against the new state to confirm it's
        // wired up to the (different) stub.
        let mut reloaded = reload(
            state,
            RendererStartInput {
                bundle_path,
                sourcemap_path: map_path,
                backend: stub_backend(|_| html_ok("<p>after</p>")),
                request_timeout: None,
            },
        )
        .expect("reload");

        let dist = tempfile::tempdir().unwrap();
        let entry = RouteUniverseEntry {
            url_path: "/".into(),
            output_path: PathBuf::from("index.html"),
            route_key: "/".into(),
        };
        let written = render_one(&mut reloaded, &entry, dist.path()).expect("render_one");
        let body = fs::read_to_string(written).unwrap();
        assert!(body.contains("after"), "expected 'after' body, got: {body}");

        // Idempotent shutdown still works after reload.
        shutdown(reloaded).expect("shutdown");
    }

    #[test]
    fn frame_candidate_parser_only_matches_path_lookalikes() {
        let body = "status: 500, at file (bundle.mjs:42:7), random 1:2 noise";
        let cands = find_frame_candidates(body);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].line, 42);
        assert_eq!(cands[0].col, 7);
    }

}
