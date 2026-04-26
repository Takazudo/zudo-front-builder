//! `zfb preview` command — static-file server for previously built artifacts.
//!
//! Unlike `zfb dev`, this command does no rebuild, no live-reload, and injects
//! no `/__zfb/*` routes. It is a clean static server: serve files from
//! `args.outdir/` over HTTP at `args.port`, fall through to a plain 404 for
//! missing paths, and exit cleanly on Ctrl+C.
//!
//! Directory-style URLs (`/`, `/foo/bar/`) resolve to the matching
//! `index.html` via [`ServeDir`]'s default `append_index_html_on_directories`
//! behavior.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use anyhow::Context;
use axum::Router;
use tower_http::services::ServeDir;

use crate::cli::PreviewArgs;

pub async fn run(args: &PreviewArgs) -> anyhow::Result<()> {
    // Verify the output directory exists *before* binding the port so that
    // missing-build errors don't leave a half-started server behind.
    if !args.outdir.exists() {
        anyhow::bail!(
            "{} does not exist — run zfb build first",
            args.outdir.display()
        );
    }

    let serve_dir = ServeDir::new(&args.outdir);
    let app = Router::new().fallback_service(serve_dir);

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), args.port);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind preview server to {addr}"))?;

    println!("→ preview ready on http://localhost:{}", args.port);

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            // Ignore the result — we want to fall through to a clean exit
            // whether ctrl_c succeeded or the signal handler errored.
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .context("preview server failed")?;

    Ok(())
}
