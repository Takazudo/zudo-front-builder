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
//! 2. Spawns ONE long-lived miniflare subprocess (pool-of-1) that
//!    serves the entire build. Per-route forks would dominate build
//!    time; one subprocess reuses workerd's hot caches across every
//!    request.
//! 3. Polls the subprocess's stdout for a JSON ready-handshake line
//!    so the renderer never sleep-races: the request loop only starts
//!    once miniflare prints its bound URL.
//! 4. For each SSG route, `GET`s the worker with [`reqwest::blocking`]
//!    and writes the response body to its dist destination. Non-2xx
//!    responses are surfaced as [`RendererError::RenderFailed`] with
//!    a re-projected source location (see step 5).
//! 5. On any worker-thrown error containing a JS stack pointing at the
//!    bundle, the [`sourcemap`] crate walks the bundle's `.map` and
//!    re-projects each frame to the user's `.tsx` / `.mdx` source
//!    location. The error message names the user file so the operator
//!    can fix the page directly without grepping minified bundle
//!    output.
//!
//! ### Dev mode (T7 will consume this)
//!
//! `zfb dev` cannot afford to start a fresh miniflare per file save.
//! [`start`] returns a [`RendererState`] that owns the long-lived
//! subprocess; [`render_one`] drives a single route against it; and
//! [`shutdown`] tears the process down on dev-server exit. The same
//! subprocess and source-map are reused across rebuilds — when the
//! bundle changes on disk, T7 swaps the [`RendererState`] for a new
//! one rather than mutating the old one in-place.
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
//! The bulk of the test suite uses the [`Backend::Existing`] mode to
//! point the renderer at a small in-process fake HTTP server. Spawning
//! real miniflare is reserved for one `#[ignore]`-gated end-to-end
//! test that holds a `flock` on `/tmp/x-wt-teams-zfb-locks/port-<N>.lock`
//! to serialise across parallel worktrees. See the test module at the
//! bottom of the file.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

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

/// The renderer's HTTP backend.
///
/// Production builds always use [`Backend::SpawnMiniflare`]. Tests use
/// [`Backend::Existing`] to skip the subprocess and aim the request
/// loop at a fake HTTP server bound to a random port.
#[derive(Debug, Clone, Default)]
pub enum Backend {
    /// Spawn `node js/miniflare-bootstrap.mjs --bundle <bundle_path>`
    /// from the workspace root, parse the ready-handshake from its
    /// stdout, and use the resolved URL.
    #[default]
    SpawnMiniflare,
    /// Skip the subprocess and use this base URL (e.g.
    /// `http://127.0.0.1:54321/`). The renderer still does the GET
    /// loop, dist write, and source-map error re-projection.
    Existing { base_url: String },
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
    /// and produce a clearer error than miniflare's own 404; that's
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
    /// Captured stderr from the miniflare subprocess (or the empty
    /// string when running against [`Backend::Existing`]). Useful
    /// for surfacing `console.warn` from the user's worker code in
    /// the build log.
    pub miniflare_logs: String,
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
/// infrastructure errors (`Io`, `MiniflareSpawn`, …).
#[derive(Debug, Error)]
pub enum RendererError {
    /// I/O error around dist write or bundle read.
    #[error("renderer I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// Miniflare subprocess failed to start, never sent the
    /// ready-handshake before the timeout, or crashed mid-build.
    #[error("miniflare subprocess error: {0}")]
    MiniflareSpawn(String),
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
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Render the entire SSG portion of `route_universe` to disk.
///
/// Spawns miniflare, drives the request loop, writes files, tears the
/// subprocess down. Always cleans up on error or panic via the
/// [`MiniflareGuard`] drop impl.
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

    let (base_url, guard) = launch(&backend, &bundle_path)?;
    let timeout = request_timeout.unwrap_or(Duration::from_secs(60));
    let client = build_client(timeout)?;
    let sourcemap = load_sourcemap(&sourcemap_path);

    let mut written: Vec<PathBuf> = Vec::with_capacity(ssg_routes.len());
    let mut last_err: Option<RendererError> = None;
    for entry in &ssg_routes {
        match render_one_inner(&client, &base_url, entry, &dist_dir, sourcemap.as_ref()) {
            Ok(path) => written.push(path),
            Err(e) => {
                last_err = Some(e);
                break;
            }
        }
    }

    let logs = guard.collect_logs();
    drop(guard);

    if let Some(err) = last_err {
        return Err(err);
    }

    Ok(RendererOutput {
        ssg_files_written: written,
        ssr_manifest: SsrManifest { routes: ssr_routes },
        miniflare_logs: logs,
    })
}

/// Long-lived dev-mode renderer state.
///
/// Owns the miniflare subprocess and the parsed sourcemap. Use [`start`]
/// to construct, [`render_one`] to drive a single route, [`shutdown`]
/// to tear down cleanly. Drop also tears down (idempotent) so a
/// panicking dev loop still kills the worker.
pub struct RendererState {
    base_url: String,
    client: reqwest::blocking::Client,
    sourcemap: Option<sourcemap::SourceMap>,
    guard: MiniflareGuard,
}

impl std::fmt::Debug for RendererState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RendererState")
            .field("base_url", &self.base_url)
            .field("has_sourcemap", &self.sourcemap.is_some())
            .finish()
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
    let (base_url, guard) = launch(&backend, &bundle_path)?;
    let client = build_client(request_timeout.unwrap_or(Duration::from_secs(30)))?;
    let sourcemap = load_sourcemap(&sourcemap_path);
    Ok(RendererState {
        base_url,
        client,
        sourcemap,
        guard,
    })
}

/// Drive one route against an existing dev-mode state and write it to
/// disk under `dist_dir`.
pub fn render_one(
    state: &RendererState,
    entry: &RouteUniverseEntry,
    dist_dir: &Path,
) -> Result<PathBuf, RendererError> {
    fs::create_dir_all(dist_dir).map_err(|e| RendererError::Io {
        path: dist_dir.to_path_buf(),
        source: e,
    })?;
    render_one_inner(
        &state.client,
        &state.base_url,
        entry,
        dist_dir,
        state.sourcemap.as_ref(),
    )
}

/// Tear the dev-mode renderer down. Idempotent — calling on an
/// already-shut-down state is a no-op.
pub fn shutdown(state: RendererState) -> Result<(), RendererError> {
    let RendererState { mut guard, .. } = state;
    guard.terminate();
    Ok(())
}

/// Reload the dev-mode renderer against a (possibly new) bundle.
///
/// Module-level reload across miniflare's worker boundary is fragile —
/// workerd loads its bundle once at boot and does not expose an
/// in-process "swap module" hook. So this implementation goes for the
/// safe-and-simple path: tear the existing subprocess down (via the
/// existing [`shutdown`]) and respawn one against the new bundle path.
/// Callers should invoke this whenever a TSX page edit, a layout edit,
/// or an exported handler change has rebuilt the worker bundle on disk
/// — typically driven from the dev pipeline's reload-renderer hook
/// (see [`crate::pipeline::BuildContext::reload_renderer`]).
///
/// The returned [`RendererState`] takes ownership of the new
/// subprocess. The previous state is consumed by value to make the
/// "old subprocess is gone" invariant statically obvious.
///
/// `request_timeout` defaults to 30s when `None` (same as
/// [`start`]).
///
/// On `Backend::Existing`, this is effectively a no-op restart: it
/// rebuilds the [`reqwest`] client and re-reads the source map, but
/// no subprocess is killed/spawned. That keeps the dev pipeline's
/// "always reload before render" code path correct under both real
/// miniflare and the in-process fake server tests use.
pub fn reload(
    previous: RendererState,
    input: RendererStartInput,
) -> Result<RendererState, RendererError> {
    // Dropping the previous state via `shutdown` happens first so the
    // old subprocess is fully gone before we spawn the new one. With
    // `Backend::Existing` the call is a cheap no-op.
    shutdown(previous)?;
    start(input)
}

// ---------------------------------------------------------------------------
// Internals — request loop
// ---------------------------------------------------------------------------

fn build_client(timeout: Duration) -> Result<reqwest::blocking::Client, RendererError> {
    reqwest::blocking::Client::builder()
        .timeout(timeout)
        // We hit 127.0.0.1; no system proxy can reach loopback usefully
        // and would only ever break the build.
        .no_proxy()
        .build()
        .map_err(|e| RendererError::Http {
            url: "<client-builder>".into(),
            source: e,
        })
}

fn render_one_inner(
    client: &reqwest::blocking::Client,
    base_url: &str,
    entry: &RouteUniverseEntry,
    dist_dir: &Path,
    sourcemap: Option<&sourcemap::SourceMap>,
) -> Result<PathBuf, RendererError> {
    let url = join_url(base_url, &entry.url_path);
    let resp = client
        .get(&url)
        .send()
        .map_err(|e| RendererError::Http {
            url: url.clone(),
            source: e,
        })?;
    let status = resp.status();
    let body = resp.bytes().map_err(|e| RendererError::Http {
        url: url.clone(),
        source: e,
    })?;
    if !status.is_success() {
        let body_str = String::from_utf8_lossy(&body).into_owned();
        let user_location = sourcemap.and_then(|sm| reproject_first_frame(&body_str, sm));
        return Err(RendererError::RenderFailed {
            url,
            status: status.as_u16(),
            body: body_str,
            user_location,
        });
    }
    let dest = dist_dir.join(&entry.output_path);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| RendererError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    fs::write(&dest, &body).map_err(|e| RendererError::Io {
        path: dest.clone(),
        source: e,
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
// Internals — miniflare subprocess
// ---------------------------------------------------------------------------

const BOOTSTRAP_JS: &str = include_str!("../js/miniflare-bootstrap.mjs");

/// JSON shape of the bootstrap's stdout handshake line.
#[derive(Debug, Deserialize)]
struct ReadyHandshake {
    event: String,
    url: String,
}

/// RAII guard that owns the subprocess and the stderr-capture thread.
///
/// Dropping the guard sends SIGTERM, joins the log thread (with a
/// short bound), and SIGKILLs if the process refuses to exit. Panics
/// in the renderer therefore never leak a child process.
struct MiniflareGuard {
    child: Option<Child>,
    log_buf: Arc<Mutex<String>>,
    log_thread: Option<thread::JoinHandle<()>>,
    /// Holds the `NamedTempFile` that backs the bootstrap script.
    /// Held for its drop side-effect (the file is unlinked on drop);
    /// never read directly.
    #[allow(dead_code)]
    bootstrap_keepalive: Option<tempfile::NamedTempFile>,
}

impl MiniflareGuard {
    fn empty() -> Self {
        Self {
            child: None,
            log_buf: Arc::new(Mutex::new(String::new())),
            log_thread: None,
            bootstrap_keepalive: None,
        }
    }

    fn collect_logs(&self) -> String {
        self.log_buf.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn terminate(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        // Send SIGTERM via libc on unix; on Windows we can only kill.
        #[cfg(unix)]
        {
            // SAFETY: child PID is valid until we wait/kill.
            let pid = child.id() as i32;
            unsafe {
                libc_kill(pid, SIGTERM);
            }
        }
        #[cfg(not(unix))]
        {
            let _ = child.kill();
        }

        // Bounded wait for graceful exit.
        let deadline = Instant::now() + Duration::from_millis(2_000);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
            }
        }

        if let Some(handle) = self.log_thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for MiniflareGuard {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(unix)]
const SIGTERM: i32 = 15;

#[cfg(unix)]
extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

#[cfg(unix)]
unsafe fn libc_kill(pid: i32, sig: i32) {
    kill(pid, sig);
}

/// Resolve the directory the miniflare subprocess should run in. We
/// need `node_modules/miniflare` to be resolvable from `cwd`, so we
/// walk up from `bundle_path` until we hit a directory that contains
/// `node_modules/miniflare`. As a final fallback, use the workspace
/// root located via `CARGO_MANIFEST_DIR/../..`.
fn resolve_subprocess_cwd(bundle_path: &Path) -> Result<PathBuf, RendererError> {
    let start = bundle_path.parent().unwrap_or_else(|| Path::new("."));
    let mut cur: Option<&Path> = Some(start);
    while let Some(p) = cur {
        if p.join("node_modules").join("miniflare").exists() {
            return Ok(p.to_path_buf());
        }
        cur = p.parent();
    }
    // Last resort: workspace root inferred from CARGO_MANIFEST_DIR
    // (`crates/zfb-build`). This is the path real production builds
    // hit, since pnpm hoists `miniflare` to the workspace root.
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(workspace) = here.parent().and_then(|p| p.parent()) {
        if workspace.join("node_modules").join("miniflare").exists() {
            return Ok(workspace.to_path_buf());
        }
    }
    Err(RendererError::MiniflareSpawn(format!(
        "could not locate node_modules/miniflare relative to bundle path {} or workspace root",
        bundle_path.display()
    )))
}

/// Spawn (or do not) the backend. Returns the resolved base URL and a
/// guard that owns any spawned subprocess.
fn launch(backend: &Backend, bundle_path: &Path) -> Result<(String, MiniflareGuard), RendererError> {
    match backend {
        Backend::Existing { base_url } => Ok((base_url.clone(), MiniflareGuard::empty())),
        Backend::SpawnMiniflare => spawn_miniflare(bundle_path),
    }
}

fn spawn_miniflare(bundle_path: &Path) -> Result<(String, MiniflareGuard), RendererError> {
    let bundle_path = if bundle_path.is_absolute() {
        bundle_path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| RendererError::Io {
                path: bundle_path.to_path_buf(),
                source: e,
            })?
            .join(bundle_path)
    };
    if !bundle_path.exists() {
        return Err(RendererError::MiniflareSpawn(format!(
            "bundle does not exist: {}",
            bundle_path.display()
        )));
    }

    let cwd = resolve_subprocess_cwd(&bundle_path)?;

    // Materialise the bootstrap script INSIDE the resolved cwd (which
    // is guaranteed to contain `node_modules/miniflare`) so Node's
    // ESM resolver finds the `miniflare` package without needing
    // `NODE_PATH` or experimental flags. We can't drop it in a random
    // /tmp dir: Node walks up *the file's* directory tree, not the
    // cwd, looking for `node_modules`, and a /tmp dir has none in its
    // ancestry.
    //
    // `NamedTempFile` gives us a unique random name + automatic
    // cleanup on drop, so two parallel renderer instances never
    // collide and a panicking caller never leaves a stray file in
    // the user's working tree.
    let bootstrap_named = tempfile::Builder::new()
        .prefix("zfb-renderer-bootstrap-")
        .suffix(".mjs")
        .tempfile_in(&cwd)
        .map_err(|e| RendererError::Io {
            path: cwd.clone(),
            source: e,
        })?;
    fs::write(bootstrap_named.path(), BOOTSTRAP_JS).map_err(|e| RendererError::Io {
        path: bootstrap_named.path().to_path_buf(),
        source: e,
    })?;
    let bootstrap_path = bootstrap_named.path().to_path_buf();

    let mut cmd = Command::new("node");
    cmd.current_dir(&cwd);
    cmd.arg(&bootstrap_path);
    cmd.arg("--bundle");
    cmd.arg(&bundle_path);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| {
        RendererError::MiniflareSpawn(format!(
            "failed to spawn `node {}`: {}",
            bootstrap_path.display(),
            e
        ))
    })?;

    // Capture stderr in a background thread.
    let log_buf = Arc::new(Mutex::new(String::new()));
    let log_thread = if let Some(stderr) = child.stderr.take() {
        let buf = log_buf.clone();
        Some(thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                let Ok(line) = line else {
                    break;
                };
                if let Ok(mut g) = buf.lock() {
                    g.push_str(&line);
                    g.push('\n');
                }
            }
        }))
    } else {
        None
    };

    // Read the ready-handshake from stdout (one line of JSON). On
    // failure, drain stderr so the error message tells the operator
    // why workerd refused to start.
    let stdout = child.stdout.take().ok_or_else(|| {
        RendererError::MiniflareSpawn("subprocess stdout pipe missing".into())
    })?;
    let ready_url = match read_ready_handshake(stdout, Duration::from_secs(30)) {
        Ok(u) => u,
        Err(RendererError::MiniflareSpawn(msg)) => {
            // Drain stderr to surface workerd's diagnostic.
            let _ = child.kill();
            let _ = child.wait();
            if let Some(handle) = log_thread {
                let _ = handle.join();
            }
            let logs = log_buf.lock().map(|s| s.clone()).unwrap_or_default();
            let snippet = if logs.len() > 4096 {
                format!("{}…(truncated)", &logs[..4096])
            } else {
                logs
            };
            return Err(RendererError::MiniflareSpawn(format!(
                "{msg}\n--- miniflare stderr ---\n{snippet}"
            )));
        }
        Err(other) => return Err(other),
    };

    let guard = MiniflareGuard {
        child: Some(child),
        log_buf,
        log_thread,
        bootstrap_keepalive: Some(bootstrap_named),
    };
    Ok((ready_url, guard))
}

/// Read the bootstrap's stdout one line at a time, parsing JSON until
/// we see `{"event":"ready", ...}`. Other JSON shapes are ignored;
/// non-JSON lines are silently dropped (workerd occasionally writes
/// `[mf:inf]` style lines on stdout depending on the version).
fn read_ready_handshake(
    stdout: std::process::ChildStdout,
    timeout: Duration,
) -> Result<String, RendererError> {
    let deadline = Instant::now() + timeout;
    let reader = BufReader::new(stdout);
    let mut lines = reader.lines();
    while Instant::now() < deadline {
        match lines.next() {
            Some(Ok(line)) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(parsed) = serde_json::from_str::<ReadyHandshake>(trimmed) {
                    if parsed.event == "ready" {
                        return Ok(parsed.url);
                    }
                }
            }
            Some(Err(e)) => {
                return Err(RendererError::MiniflareSpawn(format!(
                    "stdout read error: {e}"
                )));
            }
            None => {
                return Err(RendererError::MiniflareSpawn(
                    "subprocess closed stdout before ready handshake".into(),
                ));
            }
        }
    }
    Err(RendererError::MiniflareSpawn(format!(
        "subprocess did not emit ready handshake within {timeout:?}",
    )))
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
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Tiny single-threaded HTTP server for tests. Handles one or more
    /// connections, replies with whatever the route table says, and
    /// shuts down when the [`StopFlag`] flips.
    struct FakeServer {
        port: u16,
        stop: Arc<AtomicBool>,
        handle: Option<thread::JoinHandle<()>>,
    }

    /// (status, content-type, body). Boxed so closures with different
    /// captures can be plugged into the fake server.
    type FakeResponse = (u16, &'static str, Vec<u8>);
    type Handler = Arc<dyn Fn(&str) -> FakeResponse + Send + Sync>;

    impl FakeServer {
        fn start(handler: Handler) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let port = listener.local_addr().unwrap().port();
            let stop = Arc::new(AtomicBool::new(false));
            let stop_clone = stop.clone();
            let handle = thread::spawn(move || {
                while !stop_clone.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let h = handler.clone();
                            thread::spawn(move || handle_conn(&mut stream, &*h));
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
            });
            FakeServer {
                port,
                stop,
                handle: Some(handle),
            }
        }

        fn base_url(&self) -> String {
            format!("http://127.0.0.1:{}/", self.port)
        }
    }

    impl Drop for FakeServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }

    fn handle_conn(stream: &mut TcpStream, handler: &dyn Fn(&str) -> FakeResponse) {
        stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .ok();
        let mut buf = [0u8; 4096];
        let n = stream.read(&mut buf).unwrap_or(0);
        let req = std::str::from_utf8(&buf[..n]).unwrap_or("");
        // Parse request line `GET /path HTTP/1.1`.
        let path = req
            .split_whitespace()
            .nth(1)
            .map(|s| s.to_string())
            .unwrap_or_else(|| "/".to_string());
        let (status, ctype, body) = handler(&path);
        let status_line = match status {
            200 => "200 OK",
            404 => "404 Not Found",
            500 => "500 Internal Server Error",
            _ => "500 Internal Server Error",
        };
        let resp = format!(
            "HTTP/1.1 {status_line}\r\nContent-Type: {ctype}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
            len = body.len()
        );
        let _ = stream.write_all(resp.as_bytes());
        let _ = stream.write_all(&body);
        let _ = stream.shutdown(Shutdown::Both);
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
        let handler: Handler = Arc::new(|path: &str| match path {
            "/" => (
                200,
                "text/html; charset=utf-8",
                b"<html><body><h1>Home</h1></body></html>".to_vec(),
            ),
            "/about" => (
                200,
                "text/html; charset=utf-8",
                b"<html><body>About</body></html>".to_vec(),
            ),
            "/feed.xml" => (200, "application/xml", b"<rss/>".to_vec()),
            _ => (404, "text/plain", b"nope".to_vec()),
        });
        let srv = FakeServer::start(handler);

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
            backend: Backend::Existing {
                base_url: srv.base_url(),
            },
            request_timeout: Some(Duration::from_secs(5)),
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
        let handler: Handler = Arc::new(|_| {
            (200, "text/html", b"<html>ok</html>".to_vec())
        });
        let srv = FakeServer::start(handler);
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
            backend: Backend::Existing {
                base_url: srv.base_url(),
            },
            request_timeout: Some(Duration::from_secs(5)),
        })
        .unwrap();
        assert_eq!(out.ssg_files_written.len(), 1);
        assert!(out.ssr_manifest.routes.is_empty());
    }

    #[test]
    fn non_2xx_response_surfaces_render_failed_with_body() {
        let handler: Handler = Arc::new(|_| {
            (
                500,
                "text/plain",
                b"Error: boom\n  at fetch (bundle.mjs:42:7)\n".to_vec(),
            )
        });
        let srv = FakeServer::start(handler);
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
            backend: Backend::Existing {
                base_url: srv.base_url(),
            },
            request_timeout: Some(Duration::from_secs(5)),
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

        // Fake server: emit a fake stack trace pointing at bundle.mjs:1:1.
        let handler: Handler = Arc::new(|_| {
            (
                500,
                "text/plain",
                b"Error: explode\n  at fetch (bundle.mjs:1:1)\n".to_vec(),
            )
        });
        let srv = FakeServer::start(handler);
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
            backend: Backend::Existing {
                base_url: srv.base_url(),
            },
            request_timeout: Some(Duration::from_secs(5)),
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

    #[test]
    fn reload_swaps_renderer_state_against_existing_backend() {
        // Sub 10: when the SSR bundle changes, callers swap the
        // `RendererState` for a fresh one. Under `Backend::Existing`
        // this is effectively "tear down + start" — no subprocess is
        // killed/spawned, but the new state's HTTP client is wired
        // up and ready.
        let handler: Handler = Arc::new(|_| (200, "text/html", b"<p>after</p>".to_vec()));
        let srv = FakeServer::start(handler);

        let tmp = tempfile::tempdir().unwrap();
        let bundle_path = tmp.path().join("bundle.mjs");
        fs::write(&bundle_path, "// dev bundle\n").unwrap();
        let map_path = tmp.path().join("bundle.mjs.map");

        let state = start(RendererStartInput {
            bundle_path: bundle_path.clone(),
            sourcemap_path: map_path.clone(),
            backend: Backend::Existing {
                base_url: srv.base_url(),
            },
            request_timeout: Some(Duration::from_secs(5)),
        })
        .expect("initial start");

        // The reload consumes the previous state and returns a fresh
        // one. We then drive a single render_one against the new
        // state to confirm it's wired up to the live fake server.
        let reloaded = reload(
            state,
            RendererStartInput {
                bundle_path,
                sourcemap_path: map_path,
                backend: Backend::Existing {
                    base_url: srv.base_url(),
                },
                request_timeout: Some(Duration::from_secs(5)),
            },
        )
        .expect("reload");

        let dist = tempfile::tempdir().unwrap();
        let entry = RouteUniverseEntry {
            url_path: "/".into(),
            output_path: PathBuf::from("index.html"),
            route_key: "/".into(),
        };
        let written = render_one(&reloaded, &entry, dist.path()).expect("render_one");
        let body = fs::read_to_string(written).unwrap();
        assert!(body.contains("after"));

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

    /// Real-miniflare end-to-end test. Heavy: spawns Node, binds an
    /// ephemeral port, drives one HTTP request. Gated under
    /// `#[ignore]` so it does not run by default. Kick it on with:
    ///
    ///     cargo test --package zfb-build -- --include-ignored \
    ///         renderer::tests::end_to_end_spawn_miniflare_renders_route
    ///
    /// Uses the project-wide flock pattern at
    /// `/tmp/x-wt-teams-zfb-locks/port-1.lock` so parallel worktrees
    /// do not race.
    #[test]
    #[ignore = "spawns miniflare; run with --include-ignored"]
    fn end_to_end_spawn_miniflare_renders_route() {
        use fs2::FileExt as _;

        // Resolve the workspace root via CARGO_MANIFEST_DIR so the
        // test works regardless of pwd. miniflare is hoisted there.
        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace = here.parent().unwrap().parent().unwrap().to_path_buf();
        let modules = workspace.join("node_modules/miniflare");
        if !modules.exists() {
            eprintln!(
                "[end_to_end_spawn_miniflare_renders_route] {} missing; \
                 run `pnpm install` from the workspace root to enable.",
                modules.display()
            );
            return;
        }

        // Cross-worktree port lock.
        let lock_dir = Path::new("/tmp/x-wt-teams-zfb-locks");
        let _ = fs::create_dir_all(lock_dir);
        let lock_path = lock_dir.join("port-1.lock");
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .expect("open port lock");
        lock_file.lock_exclusive().expect("acquire port lock");

        // Self-contained worker bundle: serves /, /feed.xml, throws
        // for /error so we can poke the sourcemap path. The bundle is
        // hand-rolled because we do NOT want to drag esbuild into
        // this test — that's bundler_integration.rs's job.
        let tmp = tempfile::tempdir().expect("tempdir");
        let bundle_path = tmp.path().join("bundle.mjs");
        fs::write(
            &bundle_path,
            r#"
                export default {
                  async fetch(request) {
                    const url = new URL(request.url);
                    if (url.pathname === "/") {
                      return new Response("<html><body><h1>Home</h1></body></html>", {
                        headers: { "Content-Type": "text/html; charset=utf-8" },
                      });
                    }
                    if (url.pathname === "/feed.xml") {
                      return new Response("<rss/>", {
                        headers: { "Content-Type": "application/xml" },
                      });
                    }
                    if (url.pathname === "/error") {
                      throw new Error("boom from bundle");
                    }
                    return new Response("not found", { status: 404 });
                  },
                };
            "#,
        )
        .unwrap();

        // Stage a node_modules/miniflare symlink next to the bundle so
        // resolve_subprocess_cwd picks it up. Saves having to walk to
        // the workspace root in tests.
        let bundle_node_modules = tmp.path().join("node_modules");
        fs::create_dir_all(&bundle_node_modules).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            workspace.join("node_modules/miniflare"),
            bundle_node_modules.join("miniflare"),
        )
        .expect("symlink miniflare");

        let dist = tempfile::tempdir().unwrap();
        let universe = vec![
            RouteUniverseEntry {
                url_path: "/".into(),
                output_path: PathBuf::from("index.html"),
                route_key: "/".into(),
            },
            RouteUniverseEntry {
                url_path: "/feed.xml".into(),
                output_path: PathBuf::from("feed.xml"),
                route_key: "/feed.xml".into(),
            },
        ];

        let started = Instant::now();
        let out = render_all(RendererInput {
            bundle_path: bundle_path.clone(),
            sourcemap_path: tmp.path().join("bundle.mjs.map"),
            manifest: dummy_manifest(),
            dist_dir: dist.path().to_path_buf(),
            route_universe: universe,
            prerender_map: BTreeMap::new(),
            backend: Backend::SpawnMiniflare,
            request_timeout: Some(Duration::from_secs(15)),
        })
        .expect("render_all against real miniflare");
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(30),
            "miniflare e2e took too long: {elapsed:?}"
        );

        assert_eq!(out.ssg_files_written.len(), 2);
        let index = fs::read_to_string(dist.path().join("index.html")).unwrap();
        assert!(index.contains("Home"));
        let feed = fs::read_to_string(dist.path().join("feed.xml")).unwrap();
        assert_eq!(feed.trim(), "<rss/>");
    }
}
