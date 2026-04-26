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
//! ready banner via [`crate::output::ready`], and runs the axum server
//! until Ctrl+C.
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
//! corresponding config values **unconditionally**. `clap` defaults
//! `args.host` and `args.port` to concrete values, so we cannot cheaply
//! distinguish "user passed `--port`" from "user accepted the default";
//! treating CLI as authoritative keeps the rule predictable and avoids
//! `ArgMatches` plumbing. `out_dir` and `public_dir` come from config
//! (resolved against the project root via [`resolve_under_root`]) since
//! there is no CLI override for them on `zfb dev`.
//!
//! ## v1 scope-down: noop page renderer (still deferred)
//!
//! Wiring the real `zfb-render` page renderer remains deferred — it
//! depends on `deno_core_host`, which is still a skeleton pending
//! ADR-001's JS-runtime decision. So we hand the orchestrator a
//! [`zfb_build::PageRenderer`] that returns an empty render set: the
//! orchestrator still drives the watcher + dep-graph + reload broadcast
//! correctly, the [`zfb_server::PageCache`] simply stays empty, and
//! every request falls through to [`zfb_server::DEV_404_BODY`]. Real
//! renderer integration (and the cache-population side of the
//! `on_outcome` callback) will land once `DenoCoreHost` is real.
//!
//! ## Output
//!
//! Status lines (ready banner, orchestrator failures) go through
//! [`crate::output`] so colour/no-colour and stdout/stderr conventions
//! are handled centrally.

use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
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
use crate::config;
use crate::output;

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
    // 1. Resolve the project root and load configuration.
    let project_root = std::env::current_dir().context("failed to read current working dir")?;

    // Errors propagate to main(), which renders them through
    // output::format_error — see main.rs for the centralization rationale.
    let cfg = config::load_from_dir(&project_root)
        .await
        .context("failed to load project configuration")?;

    // 2. Resolve filesystem roots from config (relative paths join onto
    //    the project root; absolute paths used as-is).
    let dist_root = resolve_under_root(&project_root, &cfg.out_dir);
    let public_root = resolve_under_root(&project_root, &cfg.public_dir);

    // ServeDir on a missing directory just 404s, but creating dist/
    // up-front avoids a noisy warning the first time the dev server
    // boots in a brand-new project. We don't create public/ — the user
    // owns that.
    if !dist_root.exists() {
        std::fs::create_dir_all(&dist_root)
            .with_context(|| format!("failed to create dist dir {}", dist_root.display()))?;
    }

    // 3. Resolve the bind address. CLI args win unconditionally over
    //    config values — see the precedence note in the module doc.
    let addr = resolve_addr(&args.host, args.port)?;

    // 4. Live-reload broadcast channel. 64 slots is plenty for a dev
    //    server: one event per build tick and a handful of subscribers.
    let (tx, _rx) = broadcast::channel::<ReloadEvent>(64);

    // 5. Page cache shared between the orchestrator's render outputs
    //    and the server's GET handlers.
    let pages = PageCache::new();

    // 6. Build orchestrator setup. Empty dep graph for v1 — the
    //    resolver/loader that populates it is out of scope for this
    //    sub-task.
    let graph = Arc::new(Mutex::new(DependencyGraph::new()));
    let pipeline = DevAssetPipeline::new();
    let watch_roots: Vec<PathBuf> = DEFAULT_WATCH_ROOTS.iter().map(PathBuf::from).collect();
    let orch_config = OrchestratorConfig::new(&project_root, watch_roots);
    let orchestrator = BuildOrchestrator::new(orch_config, graph, pipeline);

    // Noop page renderer — see crate-level scope-down note above.
    let render_pages: PageRenderer = Arc::new(|_pages: &[PageId]| Ok(Vec::new()));
    let ctx = BuildContext {
        dist_root: dist_root.clone(),
        render_pages,
        run_css: None,
        run_islands: None,
    };

    // 7. Wire on_outcome: translate each non-noop tick to ReloadEvents
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

    // 8. Spawn the orchestrator's watcher loop.
    let orch_handle = tokio::spawn(async move {
        if let Err(err) = orchestrator.run(ctx, on_outcome).await {
            // The orchestrator's loop logs its own per-tick errors and
            // keeps going; reaching here means the watcher itself died.
            // Surface that via the structured error helper so the user
            // sees something rather than a silent dead dev server.
            output::error(&format!("build orchestrator stopped: {err:#}"));
        }
    });

    // 9. Build the serve options and announce readiness just before
    //    handing control to axum.
    let opts = ServeOpts {
        project_root,
        dist_root,
        public_root,
        addr,
        pages,
        broadcast: tx,
    };

    output::ready(&format!("http://{}:{}", args.host, args.port));

    // 10. Run the server, racing against Ctrl+C. axum::serve has no
    //     graceful-shutdown wiring in the zfb-server crate today, so on
    //     Ctrl+C we abort the orchestrator task and let the runtime
    //     drop the server when this future returns. Process exits 0.
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

/// Resolve a config-supplied path against the project root.
///
/// - If `p` is absolute, it is returned unchanged so users can point at
///   directories outside the project (e.g. a shared `dist/` on a CI box).
/// - Otherwise it is joined onto `project_root`.
///
/// This deliberately does **not** canonicalise — the directory may not
/// exist yet (`dist/` is created lazily right after this call), and we
/// want to preserve the user's spelling for log output.
fn resolve_under_root(project_root: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        project_root.join(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_under_root_joins_relative_paths() {
        let root = Path::new("/tmp/proj");
        let p = Path::new("dist");
        assert_eq!(resolve_under_root(root, p), PathBuf::from("/tmp/proj/dist"));
    }

    #[test]
    fn resolve_under_root_joins_nested_relative_paths() {
        let root = Path::new("/tmp/proj");
        let p = Path::new("build/out");
        assert_eq!(
            resolve_under_root(root, p),
            PathBuf::from("/tmp/proj/build/out")
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_under_root_keeps_absolute_paths_as_is() {
        let root = Path::new("/tmp/proj");
        let p = Path::new("/var/www/dist");
        assert_eq!(resolve_under_root(root, p), PathBuf::from("/var/www/dist"));
    }

    #[test]
    fn resolve_under_root_handles_dot_relative() {
        // `./public` should still anchor under the project root rather
        // than be silently rewritten — `Path::join` preserves the `.`,
        // which is fine because filesystem APIs treat it as a no-op.
        let root = Path::new("/tmp/proj");
        let p = Path::new("./public");
        let resolved = resolve_under_root(root, p);
        // The resolved path must start with the project root. We don't
        // require an exact textual match because `join` may or may not
        // collapse the `.` depending on platform conventions.
        assert!(
            resolved.starts_with(root),
            "expected {resolved:?} to start with {root:?}"
        );
    }
}
