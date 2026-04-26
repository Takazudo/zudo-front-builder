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
//!
//! ## Config wiring
//!
//! Loads `zfb.config.json` (or surfaces a clear "ts not yet supported" error
//! for `zfb.config.ts`) via [`crate::config::load_from_dir`] from the current
//! working directory. The `--port` flag is `Option<u16>` (no clap default),
//! so port resolution layers as "CLI flag > `zfb.config.json` value > built-in
//! default (`4321`)" — the same rule used by `zfb dev`. `--outdir` keeps a
//! clap default because the preview command does not consult config for it
//! today. Output uses [`crate::output`] helpers for consistent styling with
//! the other zfb commands.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use anyhow::Context;
use axum::Router;
use tower_http::services::ServeDir;

use crate::cli::PreviewArgs;
use crate::config;
use crate::output;

pub async fn run(args: &PreviewArgs) -> anyhow::Result<()> {
    // Resolve the project root from the current working directory and load
    // the project config (if any). A missing config file is fine — it
    // returns `Config::default()`. Any *real* error (e.g. invalid JSON,
    // unsupported zfb.config.ts) is surfaced via the output helpers and
    // propagated so `main()` can exit non-zero.
    let project_root = std::env::current_dir().context("failed to read current working dir")?;
    // Errors propagate to main() for centralized rendering — see
    // commands/dev.rs and main.rs for the shared rationale.
    let cfg = config::load_from_dir(&project_root)
        .await
        .context("failed to load project configuration")?;

    // Resolve `args.outdir` against the project root so the existence check
    // (and `ServeDir`) operate on an unambiguous path. CLI wins over config
    // unconditionally — see the precedence note in the module doc comment.
    let outdir = resolve_under_root(&project_root, &args.outdir);
    // CLI flag > config > built-in default (`4321`). See the module doc
    // comment for the precedence rule shared with `zfb dev`.
    let port = resolve_port(args.port, cfg.port);

    // Verify the output directory exists *before* binding the port so that
    // missing-build errors don't leave a half-started server behind.
    if !outdir.exists() {
        anyhow::bail!(
            "{} does not exist — run zfb build first",
            outdir.display()
        );
    }

    let serve_dir = ServeDir::new(&outdir);
    let app = Router::new().fallback_service(serve_dir);

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind preview server to {addr}"))?;

    output::ready(&format!("http://localhost:{port}"));

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

/// Resolve `path` against `root` if it is relative; absolute paths are
/// returned unchanged. Pure path arithmetic — no I/O, no `canonicalize`, so
/// it works equally for paths that don't yet exist.
fn resolve_under_root(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

/// Built-in default port for `zfb preview` when neither the CLI nor the
/// project config supplies one. Intentionally distinct from `zfb dev`'s
/// default so the two servers can run side-by-side without colliding.
const DEFAULT_PREVIEW_PORT: u16 = 4321;

/// Resolution helper: CLI override > config value > built-in default.
fn resolve_port(cli: Option<u16>, cfg: Option<u16>) -> u16 {
    cli.or(cfg).unwrap_or(DEFAULT_PREVIEW_PORT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_under_root_joins_relative_paths() {
        let root = Path::new("/tmp/project");
        assert_eq!(
            resolve_under_root(root, Path::new("dist")),
            PathBuf::from("/tmp/project/dist")
        );
        assert_eq!(
            resolve_under_root(root, Path::new("build/out")),
            PathBuf::from("/tmp/project/build/out")
        );
    }

    #[test]
    fn resolve_port_prefers_cli_over_config() {
        assert_eq!(resolve_port(Some(9000), Some(4000)), 9000);
    }

    #[test]
    fn resolve_port_falls_back_to_config_when_cli_absent() {
        assert_eq!(resolve_port(None, Some(4000)), 4000);
    }

    #[test]
    fn resolve_port_falls_back_to_builtin_when_neither_supplied() {
        assert_eq!(resolve_port(None, None), DEFAULT_PREVIEW_PORT);
        assert_eq!(DEFAULT_PREVIEW_PORT, 4321);
    }

    #[test]
    fn resolve_under_root_passes_absolute_paths_through() {
        let root = Path::new("/tmp/project");
        let abs = if cfg!(windows) {
            PathBuf::from("C:/elsewhere/dist")
        } else {
            PathBuf::from("/elsewhere/dist")
        };
        assert_eq!(resolve_under_root(root, &abs), abs);
    }

}
