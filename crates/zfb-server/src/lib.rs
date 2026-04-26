//! `zfb-server` — the dev-mode HTTP server for `zudo-front-builder`.
//!
//! This crate runs an [`axum`] server that serves the in-memory
//! page-cache HTML produced by [`zfb_build`]'s rebuild loop, plus the
//! built `dist/assets/` and `public/` directories. Every served HTML
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

pub mod inject;
pub mod livereload;
pub mod routes;

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context;
use tokio::net::TcpListener;
use tracing::info;

pub use inject::{inject_livereload, LIVERELOAD_TAG};
pub use livereload::{outcome_to_events, ReloadEvent, ReloadTx};
pub use routes::{build_router, AppState, PageCache, DEV_404_BODY};

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

    /// Project public-static directory. `/public/*` is served from
    /// here verbatim.
    pub public_root: PathBuf,

    /// Address to bind. Defaults to `127.0.0.1:3000`.
    pub addr: SocketAddr,

    /// Page cache populated by the bin crate's render loop.
    pub pages: PageCache,

    /// Broadcast sender feeding the SSE live-reload channel. The bin
    /// crate sends [`ReloadEvent`]s into this from
    /// [`zfb_build::BuildOrchestrator::run`]'s `on_outcome` callback.
    pub broadcast: ReloadTx,
}

impl ServeOpts {
    /// The default bind address: `127.0.0.1:3000`.
    pub fn default_addr() -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], 3000))
    }
}

/// Run the dev server until the process is shut down.
///
/// Binds [`ServeOpts::addr`], builds the axum router via
/// [`build_router`], and serves it. The future resolves with `Ok(())`
/// only when the server stops cleanly (axum's signal handling).
///
/// Errors:
///
/// - failure to bind the address (port in use, permission denied, …),
/// - axum's serve loop returns an error.
pub async fn serve(opts: ServeOpts) -> anyhow::Result<()> {
    let state = AppState {
        pages: opts.pages,
        broadcast: opts.broadcast,
    };
    let router = build_router(state, opts.dist_root.clone(), opts.public_root.clone());

    let listener = TcpListener::bind(opts.addr)
        .await
        .with_context(|| format!("failed to bind dev server to {}", opts.addr))?;

    let actual = listener.local_addr().unwrap_or(opts.addr);
    info!(
        addr = %actual,
        project_root = %opts.project_root.display(),
        dist_root = %opts.dist_root.display(),
        public_root = %opts.public_root.display(),
        "zfb-server listening"
    );

    axum::serve(listener, router)
        .await
        .context("zfb-server: axum::serve returned an error")?;

    Ok(())
}
