//! `zfb dev` — boot the dev pipeline + dev HTTP server.
//!
//! Wires four crates together per the doc-comment in
//! [`zfb_server`]'s lib.rs:
//!
//! 1. A [`tokio::sync::broadcast`] channel of [`zfb_server::ReloadEvent`]s
//!    that the SSE live-reload route consumes.
//! 2. A [`zfb_server::PageCache`] of rendered HTML keyed by URL path.
//! 3. A [`zfb_build::BuildOrchestrator`] driving the watcher + dep-graph
//!    + asset pipeline; its `on_outcome` callback translates every
//!      non-noop tick into reload events via
//!      [`zfb_server::outcome_to_events`] and broadcasts them.
//! 4. A long-lived [`zfb_build::renderer::RendererState`] (T7) that
//!    owns the embedded V8 host. The asset pipeline's
//!    [`PageRenderer`] callback drives [`zfb_build::renderer::render_one`]
//!    against this state per affected route, so a single edit triggers
//!    one in-process render — not a fresh host boot.
//!
//! Then it binds the address from `args.host:args.port`, prints the
//! ready banner via [`crate::output::ready`], and runs the axum server
//! until Ctrl+C.
//!
//! ## Lifecycle of the renderer state
//!
//! `start(...)` is called once at boot. The returned
//! [`zfb_build::renderer::RendererState`] is wrapped in a [`Drop`]
//! guard so panics, Ctrl-C, or any other early-exit path tears the
//! embedded V8 host down cleanly. Without that guard a panicking
//! dev loop would leak the host resources.
//!
//! ## Configuration
//!
//! Project configuration is loaded via [`crate::config::load_from_dir`]
//! at startup. Today this resolves to a `zfb.config.json` if present, or
//! sensible defaults otherwise; encountering a `zfb.config.ts` produces a
//! clear "not yet supported" error (the TS pipeline is blocked on
//! ADR-001's runtime decision).
//!
//! **Precedence rule:** CLI args (`--host`, `--port`) override the
//! corresponding config values when supplied.
//!
//! ## v1 → wave-3 transition
//!
//! Earlier waves passed a noop [`PageRenderer`] into the orchestrator
//! that returned an empty `RenderedPage` list — the dev cache stayed
//! empty and every request fell through to the dev 404 body.
//!
//! Wave 3 (T7) replaces that noop with a renderer-backed callback. The
//! callback maps each page id to its [`zfb_build::renderer::RouteUniverseEntry`]
//! and drives [`zfb_build::renderer::render_one`] against the long-
//! lived [`zfb_build::renderer::RendererState`]. The result file is
//! read back into a [`zfb_build::RenderedPage`] so the existing
//! atomic-write + reload-broadcast plumbing keeps working unchanged.
//!
//! ### Same gaps as `zfb build`
//!
//! Dynamic / catchall routes (`paths()` runtime expansion) and the
//! Worker-entry wrapping are still pending. See [`crate::commands::build`]
//! for the detailed gap analysis. Today the dev renderer reuses the
//! same plumbing and inherits the same limitations.

use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tokio::sync::broadcast;
use zfb_build::bundler::{bundle, BundleMode, BundlerInput, BundlerOutput};
use zfb_build::renderer::{
    render_one, shutdown, start, Backend, RendererStartInput, RendererState, RouteUniverseEntry,
};
use zfb_build::{
    BuildContext, BuildOrchestrator, BuildOutcome, DevAssetPipeline, OrchestratorConfig,
    PageRenderer, RelDistPath, RenderedPage,
};
use zfb_graph::persist::{load_from_disk, save_to_disk, ManifestDigest};
use zfb_graph::{DependencyGraph, PageId};
use zfb_server::{outcome_to_events, serve, PageCache, ReloadEvent, ServeOpts};

use crate::cli::DevArgs;
use crate::commands::resolve::{resolve_port, resolve_under_root};
use crate::config;
use crate::output;
use crate::render_pipeline::{
    build_route_universe, cfg_framework_to_render, check_runtime_installed,
};

/// Default source directories the watcher follows.
const DEFAULT_WATCH_ROOTS: &[&str] = &[
    "pages",
    "content",
    "components",
    "layouts",
    "styles",
    "data",
    "public",
    "zfb.config.json",
    "zfb.config.ts",
];

/// Entry point for `zfb dev`.
pub async fn run(args: &DevArgs) -> Result<()> {
    // 1. Resolve the project root and load configuration.
    let project_root = std::env::current_dir().context("failed to read current working dir")?;

    let cfg = config::load_from_dir(&project_root)
        .await
        .context("failed to load project configuration")?;

    let dist_root = resolve_under_root(&project_root, &cfg.out_dir);
    let public_root = resolve_under_root(&project_root, &cfg.public_dir);

    if !dist_root.exists() {
        std::fs::create_dir_all(&dist_root)
            .with_context(|| format!("failed to create dist dir {}", dist_root.display()))?;
    }

    let host = resolve_host(args.host.as_deref(), cfg.host.as_deref());
    let port = resolve_port(args.port, cfg.port, DEFAULT_DEV_PORT);
    let addr = resolve_addr(host.as_str(), port)?;

    let (tx, _rx) = broadcast::channel::<ReloadEvent>(64);
    let pages = PageCache::new();

    // Sub 3 / #108 — plugin lifecycle. Spawn the host once at boot so
    // `preBuild` runs before the bundler/renderer start, and so dev-
    // middleware registrations can be installed into the dev server.
    // The host is dropped when this `run` returns (Ctrl+C path), which
    // kills the subprocess.
    let plugin_host = crate::commands::plugins::maybe_spawn_host(&cfg).await?;
    if let Some(h) = plugin_host.as_ref() {
        let ctx = zfb_build::BuildHookContext {
            project_root: project_root.clone(),
            out_dir: dist_root.clone(),
            config: serde_json::to_value(&cfg)
                .context("plugin lifecycle: serialise config for preBuild ctx")?,
        };
        h.run_pre_build(&ctx)
            .await
            .map_err(zfb_build::annotate_with_plugin_error)
            .context("preBuild lifecycle hook")?;
    }
    let plugin_set = if let Some(h) = plugin_host.as_ref() {
        crate::commands::plugins::build_dev_middleware_set(h, &project_root, &cfg).await?
    } else {
        None
    };

    // 2. Stand up the long-lived renderer state if the project looks
    //    runnable. We surface failures as a warning + fall back to the
    //    noop renderer so the dev server still boots — the user can
    //    still poke at the dev URL while they fix the underlying
    //    bundler / runtime issue.
    let dev_session = match boot_dev_renderer(&project_root, &cfg, &dist_root) {
        Ok(s) => Some(s),
        Err(err) => {
            output::warn(format!(
                "renderer disabled — falling back to empty page cache: {err:#}",
            ));
            None
        }
    };

    // 3. Build orchestrator setup.
    //
    // Cold-start optimisation: try to reuse a previously persisted
    // graph from `.zfb/graph.bin`. If the manifest digest still
    // matches the current project layout, deserialise and reuse —
    // otherwise build fresh and save the new graph back so the
    // *next* cold start is fast.
    let watch_roots: Vec<PathBuf> = DEFAULT_WATCH_ROOTS.iter().map(PathBuf::from).collect();
    let graph_cache_path = project_root.join(".zfb").join("graph.bin");
    let manifest_digest = compute_manifest_digest(&project_root, &watch_roots);
    let initial_graph = load_persisted_graph(&graph_cache_path, manifest_digest.as_ref());
    // Note: we deliberately do NOT write a fresh empty graph here on
    // a cache miss. If we did, a `zfb dev` killed before the
    // orchestrator's first watcher tick would persist an empty graph
    // tagged with the current digest — and the next cold start would
    // happily reuse that empty cache as authoritative. Save only on
    // shutdown (below), once the graph has actually been populated.
    //
    // Formerly `initial_graph.unwrap_or_default()`. Now explicit: on a
    // cache miss we construct a known-empty graph. Default was removed
    // from DependencyGraph to prevent silent empty-graph construction
    // elsewhere.
    let graph = Arc::new(Mutex::new(initial_graph.unwrap_or_else(DependencyGraph::new)));
    let graph_for_save = Arc::clone(&graph);
    let pipeline = DevAssetPipeline::new();
    let orch_config = OrchestratorConfig::new(&project_root, watch_roots.clone());
    let orchestrator = BuildOrchestrator::new(orch_config, graph, pipeline);

    let render_pages: PageRenderer = match dev_session.as_ref() {
        Some(session) => make_render_callback(session.clone(), dist_root.clone()),
        None => Arc::new(|_pages: &[PageId]| Ok(Vec::new())),
    };

    let ctx = BuildContext {
        dist_root: dist_root.clone(),
        render_pages,
        run_css: None,
        run_islands: None,
        // The bundle-rebuild + renderer-reload wiring (Sub 10) lands
        // here once the dev-mode bundler is available on a per-tick
        // basis; for now leave the hook empty so existing behaviour
        // is preserved (the renderer state stays bound to the
        // boot-time bundle).
        reload_renderer: None,
    };

    // 4. on_outcome — translate each tick into reload events.
    let tx_cb = tx.clone();
    let on_outcome = move |outcome: &BuildOutcome| {
        for ev in outcome_to_events(outcome) {
            let _ = tx_cb.send(ev);
        }
    };

    // 5. Spawn the orchestrator's watcher loop.
    let orch_handle = tokio::spawn(async move {
        if let Err(err) = orchestrator.run(ctx, on_outcome).await {
            output::error(format!("build orchestrator stopped: {err:#}"));
        }
    });

    // 6. Build the serve options and announce readiness.
    let opts = ServeOpts {
        project_root,
        dist_root,
        public_root,
        addr,
        pages,
        broadcast: tx,
        plugins: plugin_set,
    };

    output::ready(&format!("http://{host}:{port}"));

    // 7. Run the server until Ctrl+C. Pass Ctrl+C as the graceful-shutdown
    //    signal so axum drains in-flight connections before exiting. The
    //    renderer guard tears down on drop here — the explicit `shutdown`
    //    call belt-and-braces keeps the surface symmetrical (start ↔ shutdown).
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let result = tokio::select! {
        res = serve(opts, ctrl_c) => {
            orch_handle.abort();
            res
        }
    };

    if let Some(session) = dev_session {
        session.shutdown_explicit();
    }

    // Sub 3 / #108 — tear down the plugin host before exit so the Node
    // subprocess doesn't outlive `zfb dev`. Best-effort: a kill via
    // `kill_on_drop` covers the panic / Ctrl+C path; the explicit
    // shutdown is the graceful one.
    if let Some(h) = plugin_host {
        let _ = h.shutdown().await;
    }

    // Persist the graph one more time before exit so the latest
    // populated state — not just the boot-time fresh one — is what
    // the next cold start sees. Best-effort; warn-and-ignore on
    // failure (don't block shutdown on a disk error).
    if let Some(d) = manifest_digest.as_ref() {
        if let Ok(g) = graph_for_save.lock() {
            if let Err(err) = save_to_disk(&g, d, &graph_cache_path) {
                output::warn(format!(
                    "graph persistence: shutdown write to {} failed (ignored): {err:#}",
                    graph_cache_path.display()
                ));
            }
        }
    }

    result
}

/// Compute the manifest digest for the current project, or return
/// `None` if the digest itself could not be computed (e.g. permission
/// denied while walking sources). On `None` the caller should bypass
/// the persistence layer entirely — never falsely reuse a stale
/// graph.
fn compute_manifest_digest(
    project_root: &Path,
    watch_roots: &[PathBuf],
) -> Option<ManifestDigest> {
    // Config files that, when changed, must invalidate the graph
    // even though they live next to (not under) the watched roots.
    // Both JSON and TS are listed; missing ones are silently
    // skipped by the digest builder.
    let cfg_files = [
        PathBuf::from("zfb.config.json"),
        PathBuf::from("zfb.config.ts"),
    ];
    match ManifestDigest::compute(project_root, watch_roots, &cfg_files) {
        Ok(d) => Some(d),
        Err(err) => {
            output::warn(format!(
                "graph persistence: manifest digest failed (cache disabled): {err:#}"
            ));
            None
        }
    }
}

/// Try to reuse a persisted graph. Returns `Some(graph)` only when
/// the on-disk file exists and its digest matches the live one. All
/// other outcomes (no digest, missing file, mismatch, IO error) map
/// to `None` so the caller falls back to a fresh graph.
fn load_persisted_graph(
    graph_cache_path: &Path,
    digest: Option<&ManifestDigest>,
) -> Option<DependencyGraph> {
    let d = digest?;
    match load_from_disk(graph_cache_path, d) {
        Ok(Some(g)) => Some(g),
        Ok(None) => None,
        Err(err) => {
            output::warn(format!(
                "graph persistence: load from {} failed (ignored): {err:#}",
                graph_cache_path.display()
            ));
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Renderer plumbing
// ---------------------------------------------------------------------------

/// Long-lived dev-session state that owns the renderer subprocess and
/// the route table. Cloned by the [`PageRenderer`] callback so each
/// orchestrator tick can map page ids → URLs.
#[derive(Clone)]
struct DevRenderSession {
    inner: Arc<DevRenderInner>,
}

struct DevRenderInner {
    /// Mapped from the page module's project-relative source path
    /// (which is what the dependency graph keys on) to the renderer
    /// entry. Built once at boot from the router scan.
    routes_by_source: HashMap<PathBuf, RouteUniverseEntry>,
    /// Mutex-wrapped renderer state. The orchestrator's callback runs
    /// on the watcher's thread; render_one is sync and short, so a
    /// global lock is fine here.
    renderer: Mutex<Option<RendererState>>,
}

impl DevRenderSession {
    /// Drive a single page id against the renderer. Returns a
    /// [`RenderedPage`] populated with the bytes the renderer just
    /// wrote, so the dev pipeline's atomic-write + cache layer can
    /// fold the result through the existing reload broadcast.
    fn render_one(&self, page: &PageId, dist_dir: &Path) -> Result<Option<RenderedPage>> {
        let entry = match self.inner.routes_by_source.get(page.path()) {
            Some(e) => e.clone(),
            None => return Ok(None),
        };
        let mut lock = self.inner.renderer.lock().unwrap_or_else(|p| {
            tracing::warn!(site = "DevRenderSession", "mutex poisoned, recovered");
            p.into_inner()
        });
        let state = lock
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("renderer not started"))?;
        let written = render_one(state, &entry, dist_dir).map_err(anyhow::Error::from)?;
        let html = std::fs::read_to_string(&written)
            .with_context(|| format!("failed to read rendered page {}", written.display()))?;
        // RouteUniverseEntry::output_path is a PathBuf validated by the
        // router/render_pipeline (relative, no escapes). Wrap it in
        // RelDistPath for the pipeline's type contract. If the path is
        // somehow invalid, surface an error rather than silently skipping.
        let output_path = RelDistPath::new(entry.output_path)
            .with_context(|| format!("renderer returned invalid output_path for {:?}", page))?;
        Ok(Some(RenderedPage {
            page: page.clone(),
            output_path,
            html,
            content_type: None,
        }))
    }

    /// Tear down the underlying [`RendererState`] cleanly. Safe to call
    /// multiple times — subsequent calls are a no-op.
    fn shutdown_explicit(&self) {
        let mut lock = self.inner.renderer.lock().unwrap_or_else(|p| {
            tracing::warn!(site = "DevRenderSession", "mutex poisoned, recovered");
            p.into_inner()
        });
        if let Some(state) = lock.take() {
            let _ = shutdown(state);
        }
    }
}

impl Drop for DevRenderInner {
    fn drop(&mut self) {
        // Defence-in-depth: even if `shutdown_explicit` was missed
        // (panicking dev loop, early ?-return), the inner Arc's drop
        // tears the subprocess down. Errors from shutdown are
        // swallowed here — the process is already going away.
        if let Ok(mut g) = self.renderer.lock() {
            if let Some(state) = g.take() {
                let _ = shutdown(state);
            }
        }
    }
}

/// Bring up the renderer and the route map for the dev session.
///
/// On any error, returns it unchanged so the caller decides whether to
/// fall back to a noop renderer or hard-fail. Today the dev command
/// chooses to fall back so the user still gets a reachable HTTP server
/// while they fix the underlying issue.
fn boot_dev_renderer(
    project_root: &Path,
    cfg: &config::Config,
    dist_root: &Path,
) -> Result<DevRenderSession> {
    check_runtime_installed(project_root)?;

    let pages_dir = project_root.join("pages");
    if !pages_dir.is_dir() {
        return Err(anyhow::anyhow!(
            "no pages/ directory under {}",
            project_root.display()
        ));
    }
    let router = zfb_router::Router::scan(&pages_dir).map_err(anyhow::Error::from)?;

    let plan = build_route_universe(router.routes());
    if plan.static_routes.is_empty() {
        return Err(anyhow::anyhow!(
            "no static routes to render — dev mode skips renderer boot"
        ));
    }

    // Dev mode does not embed a content snapshot — runtime paths()
    // evaluation is a build-mode feature. When dev mode starts the
    // worker, `getCollection(...)` will see an empty snapshot.
    let mut bundler_input = BundlerInput::for_project(
        project_root.to_path_buf(),
        cfg_framework_to_render(cfg.framework),
        BundleMode::Development,
        dist_root.join(".zfb-build"),
        None,
    );
    // Mirror the production build CLI: surface project-side
    // `node_modules/` and `tsconfig.json#compilerOptions.paths` to the
    // bundler so esbuild can resolve user-installed packages and TS
    // path aliases. Helpers live in `commands/build.rs`; they are
    // intentionally re-used so dev and prod stay in lockstep.
    if let Some(nm) = crate::commands::build::detect_project_node_modules(project_root) {
        bundler_input.node_modules_dir = Some(nm);
    }
    bundler_input.tsconfig_paths = crate::commands::build::read_tsconfig_paths(project_root);
    // Per-collection content materialisation so dev-mode SSR also
    // installs `globalThis.__zfb.content` and renders MDX bodies as
    // JSX (#506). Mirrors the production-build wiring in build.rs.
    bundler_input.content_collections = cfg
        .collections
        .iter()
        .map(|c| zfb_build::ContentCollectionSpec::new(c.name.clone(), c.path.clone()))
        .collect();
    // Thread the opt-in `stripMdExt` flag through so the dev-mode
    // bundler (which feeds the embedded V8 host) honours the same setting as
    // `zfb build`. The dev loader at `crates/zfb-render/src/loader.rs`
    // also reads this flag for in-process MDX rendering, so dev preview
    // matches built dist (zfb#127 / #129).
    bundler_input.strip_md_ext = cfg.strip_md_ext;
    let bundler_out: BundlerOutput = bundle(bundler_input).context("bundler step failed")?;

    let state = start(RendererStartInput {
        bundle_path: bundler_out.bundle_path.clone(),
        sourcemap_path: bundler_out.sourcemap_path.clone(),
        backend: Backend::EmbeddedV8,
        request_timeout: None,
    })
    .map_err(anyhow::Error::from)
    .context("renderer start failed")?;

    // Build the source-path → entry map once. Router source paths are
    // project-relative; PageId keys on the same value (the orchestrator
    // tracks pages by their source path).
    let mut routes_by_source: HashMap<PathBuf, RouteUniverseEntry> = HashMap::new();
    for route in router.routes() {
        if let Some(entry) = plan
            .static_routes
            .iter()
            .find(|e| e.route_key == route.template())
        {
            routes_by_source.insert(route.source_path.clone(), entry.clone());
        }
    }

    Ok(DevRenderSession {
        inner: Arc::new(DevRenderInner {
            routes_by_source,
            renderer: Mutex::new(Some(state)),
        }),
    })
}

/// Build the [`PageRenderer`] callback that the orchestrator hands to
/// [`DevAssetPipeline`].
fn make_render_callback(session: DevRenderSession, dist_dir: PathBuf) -> PageRenderer {
    Arc::new(move |pages: &[PageId]| {
        let mut out = Vec::with_capacity(pages.len());
        for page in pages {
            match session.render_one(page, &dist_dir) {
                Ok(Some(rendered)) => out.push(rendered),
                Ok(None) => {
                    // Page not in the renderer's route map (e.g.
                    // dynamic route deferred to the paths() follow-up,
                    // or a page that was never in the router scan).
                    // Intentionally a no-op so the watcher tick still
                    // succeeds — the orchestrator will report the page
                    // as not-rendered, the user sees the warning at
                    // boot, and other pages keep rebuilding.
                }
                Err(err) => {
                    // One page's render failure must not kill the
                    // watcher. Log and continue.
                    output::error(format!(
                        "renderer error for {}: {err:#}",
                        page.path().display()
                    ));
                }
            }
        }
        Ok(out)
    })
}

const DEFAULT_DEV_HOST: &str = "localhost";
const DEFAULT_DEV_PORT: u16 = 3000;

fn resolve_host(cli: Option<&str>, cfg: Option<&str>) -> String {
    cli.or(cfg).unwrap_or(DEFAULT_DEV_HOST).to_owned()
}

fn resolve_addr(host: &str, port: u16) -> Result<SocketAddr> {
    let pair = format!("{host}:{port}");
    let mut iter = pair
        .to_socket_addrs()
        .with_context(|| format!("could not resolve bind address {pair}"))?;
    iter.next()
        .ok_or_else(|| anyhow::anyhow!("no socket addresses resolved for {pair}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn resolve_host_prefers_cli_over_config() {
        assert_eq!(resolve_host(Some("0.0.0.0"), Some("127.0.0.1")), "0.0.0.0");
    }

    #[test]
    fn resolve_host_falls_back_to_config_when_cli_absent() {
        assert_eq!(resolve_host(None, Some("127.0.0.1")), "127.0.0.1");
    }

    #[test]
    fn resolve_host_falls_back_to_builtin_when_neither_supplied() {
        assert_eq!(resolve_host(None, None), DEFAULT_DEV_HOST);
    }

    #[test]
    fn default_watch_roots_includes_zfb_config_json() {
        assert!(DEFAULT_WATCH_ROOTS.contains(&"zfb.config.json"));
        assert!(DEFAULT_WATCH_ROOTS.contains(&"zfb.config.ts"));
    }

    /// The render callback must:
    /// 1. Be tolerant of unknown page ids (dynamic routes, etc.) —
    ///    return an empty list, never error.
    /// 2. Hand back a `RenderedPage` for every page id mapped to a
    ///    [`RouteUniverseEntry`].
    ///
    /// We exercise both via a [`DevRenderSession`] that uses a `None`
    /// renderer; an unknown page id therefore always lands on the
    /// `Ok(None)` branch and never reaches the lock.
    #[test]
    fn render_callback_drops_unknown_pages_silently() {
        let session = DevRenderSession {
            inner: Arc::new(DevRenderInner {
                routes_by_source: HashMap::new(),
                renderer: Mutex::new(None),
            }),
        };
        let cb = make_render_callback(session, PathBuf::from("/tmp/dist"));
        let pages = vec![PageId::new(PathBuf::from("pages/index.tsx"))];
        let out = cb(&pages).unwrap();
        assert!(out.is_empty());
    }

    /// On a known page id, but a `None` renderer (boot half-failed),
    /// the callback logs an error and returns an empty list — the
    /// watcher must keep going.
    #[test]
    fn render_callback_keeps_watcher_alive_on_render_error() {
        let mut routes = HashMap::new();
        routes.insert(
            PathBuf::from("pages/index.tsx"),
            RouteUniverseEntry {
                url_path: "/".into(),
                output_path: PathBuf::from("index.html"),
                route_key: "/".into(),
            },
        );
        let session = DevRenderSession {
            inner: Arc::new(DevRenderInner {
                routes_by_source: routes,
                renderer: Mutex::new(None),
            }),
        };
        let cb = make_render_callback(session, PathBuf::from("/tmp/dist"));
        let pages = vec![PageId::new(PathBuf::from("pages/index.tsx"))];
        let out = cb(&pages).unwrap();
        assert!(out.is_empty(), "errors must yield empty list, not panic");
    }
}
