//! `zfb dev` — boot the dev pipeline + dev HTTP server.
//!
//! Wires three crates together per the doc-comment in
//! [`zfb_server`]'s lib.rs:
//!
//! 1. A [`tokio::sync::broadcast`] channel of [`zfb_server::ReloadEvent`]s
//!    that the SSE live-reload route consumes.
//! 2. A [`zfb_server::PageCache`] of rendered HTML keyed by URL path.
//! 3. A [`zfb_build::BuildOrchestrator`] driving the watcher + dep-graph
//!    + asset pipeline; its `on_outcome` callback translates every
//!    non-noop tick into reload events via
//!    [`zfb_server::outcome_to_events`] and broadcasts them.
//!
//! Then it binds the address from `args.host:args.port`, prints the
//! ready banner, and runs the axum server until Ctrl+C.
//!
//! ## v1 scope-down: noop page renderer
//!
//! Wiring the real `zfb-render` page renderer (which depends on
//! `deno_core_host` and the SSR bundle) is deliberately deferred — Sub 4
//! owns only `crates/zfb/src/commands/dev.rs` and the v1 acceptance is
//! "the dev server boots and serves SOMETHING (even the dev_404 body)
//! with live-reload working". So we hand the orchestrator a
//! [`zfb_build::PageRenderer`] that returns an empty render set: the
//! orchestrator still drives the watcher + dep-graph + reload broadcast
//! correctly, the [`zfb_server::PageCache`] simply stays empty, and
//! every request falls through to [`zfb_server::DEV_404_BODY`]. Real
//! renderer integration (and the cache-population side of the
//! `on_outcome` callback) is a follow-up after Wave 2 lands.
//!
//! Configuration loading (`crate::config::load_from_dir`) is also
//! deferred per the manager's plan — it's Sub 3's territory and gets
//! wired in after merge. For now we work directly off
//! [`std::env::current_dir`] and a fixed list of watch roots.

use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tokio::sync::broadcast;
use zfb_build::{
    BuildContext, BuildOrchestrator, BuildOutcome, DevAssetPipeline, OrchestratorConfig,
    PageRenderer,
};
use zfb_graph::{DependencyGraph, PageId};
use zfb_server::{outcome_to_events, serve, PageCache, ReloadEvent, ServeOpts};

use crate::cli::DevArgs;

/// Default source directories the watcher follows. Missing entries are
/// silently skipped by `zfb_watcher::Watcher::start`, so it's fine to
/// list paths that don't exist in every project.
const DEFAULT_WATCH_ROOTS: &[&str] = &[
    "pages",
    "content",
    "components",
    "layouts",
    "styles",
    "data",
    "public",
    "zfb.config.ts",
];

/// Entry point for `zfb dev`.
pub async fn run(args: &DevArgs) -> Result<()> {
    // 1. Resolve the project root. TODO(Sub 3 follow-up): replace with
    //    `crate::config::load_from_dir(&cwd).await` once config wiring
    //    lands; for v1 we operate on the current dir directly.
    let project_root = std::env::current_dir().context("failed to read current working dir")?;
    let dist_root = project_root.join("dist");
    let public_root = project_root.join("public");

    // ServeDir on a missing directory just 404s, but creating dist/
    // up-front avoids a noisy warning the first time the dev server
    // boots in a brand-new project. We don't create public/ — the user
    // owns that.
    if !dist_root.exists() {
        std::fs::create_dir_all(&dist_root)
            .with_context(|| format!("failed to create dist dir {}", dist_root.display()))?;
    }

    // 2. Resolve the bind address. `args.host` may be "localhost",
    //    "127.0.0.1", "0.0.0.0", etc., so we go through ToSocketAddrs.
    let addr = resolve_addr(&args.host, args.port)?;

    // 3. Live-reload broadcast channel. 64 slots is plenty for a dev
    //    server: one event per build tick and a handful of subscribers.
    let (tx, _rx) = broadcast::channel::<ReloadEvent>(64);

    // 4. Page cache shared between the orchestrator's render outputs
    //    and the server's GET handlers.
    let pages = PageCache::new();

    // 5. Build orchestrator setup. Empty dep graph for v1 — the
    //    resolver/loader that populates it is out of scope for Sub 4.
    let graph = Arc::new(Mutex::new(DependencyGraph::new()));
    let pipeline = DevAssetPipeline::new();
    let watch_roots: Vec<PathBuf> = DEFAULT_WATCH_ROOTS.iter().map(PathBuf::from).collect();
    let config = OrchestratorConfig::new(&project_root, watch_roots);
    let orchestrator = BuildOrchestrator::new(config, graph, pipeline);

    // Noop page renderer — see crate-level scope-down note above.
    let render_pages: PageRenderer = Arc::new(|_pages: &[PageId]| Ok(Vec::new()));
    let ctx = BuildContext {
        dist_root: dist_root.clone(),
        render_pages,
        run_css: None,
        run_islands: None,
    };

    // 6. Wire on_outcome: translate each non-noop tick to ReloadEvents
    //    and broadcast them. Page cache population is intentionally
    //    skipped here because the noop renderer never emits any
    //    RenderedPage outputs (BuildOutcome only carries page IDs, not
    //    HTML, so the cache-fill path lives next to the renderer
    //    itself — that's a follow-up).
    let tx_cb = tx.clone();
    let on_outcome = move |outcome: &BuildOutcome| {
        for ev in outcome_to_events(outcome) {
            // `send` fails only if there are no live subscribers; that's
            // fine — the next subscriber will pick up the next event.
            let _ = tx_cb.send(ev);
        }
    };

    // 7. Spawn the orchestrator's watcher loop.
    let orch_handle = tokio::spawn(async move {
        if let Err(err) = orchestrator.run(ctx, on_outcome).await {
            // The orchestrator's loop logs its own per-tick errors and
            // keeps going; reaching here means the watcher itself died.
            // Surface that to stderr so the user sees something rather
            // than a silent dead dev server.
            eprintln!("zfb dev: build orchestrator stopped: {err:#}");
        }
    });

    // 8. Build the serve options and announce readiness just before
    //    handing control to axum. Plain println for now — Sub 7 may
    //    upgrade this to colored output.
    let opts = ServeOpts {
        project_root,
        dist_root,
        public_root,
        addr,
        pages,
        broadcast: tx,
    };

    println!("→ ready on http://{}:{}", args.host, args.port);

    // 9. Run the server, racing against Ctrl+C. axum::serve has no
    //    graceful-shutdown wiring in the zfb-server crate today, so on
    //    Ctrl+C we abort the orchestrator task and let the runtime
    //    drop the server when this future returns. Process exits 0.
    tokio::select! {
        res = serve(opts) => {
            // serve() returned on its own — propagate any error.
            orch_handle.abort();
            res
        }
        _ = tokio::signal::ctrl_c() => {
            orch_handle.abort();
            Ok(())
        }
    }
}

/// Resolve `host:port` into a single [`SocketAddr`] via the OS resolver,
/// preferring the first hit. Errors carry the raw input so the user can
/// see what went wrong.
fn resolve_addr(host: &str, port: u16) -> Result<SocketAddr> {
    let pair = format!("{host}:{port}");
    let mut iter = pair
        .to_socket_addrs()
        .with_context(|| format!("could not resolve bind address {pair}"))?;
    iter.next()
        .ok_or_else(|| anyhow::anyhow!("no socket addresses resolved for {pair}"))
}
