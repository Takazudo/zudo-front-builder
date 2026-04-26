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
//! working directory. The config's `outDir` is treated as the *default* — a
//! CLI `--outdir` value always wins, since the user explicitly typed it. The
//! same precedence applies to `--port`. Output uses [`crate::output`]
//! helpers for consistent styling with the other zfb commands.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use anyhow::Context;
use axum::Router;
use tower_http::services::ServeDir;

use crate::cli::PreviewArgs;
use crate::config;
use crate::output;

/// Default value used by clap for `--outdir`. Kept in sync with
/// [`crate::cli::PreviewArgs`] so we can detect when the user did not pass an
/// explicit override and we should fall back to the config's `outDir`.
const CLAP_DEFAULT_OUTDIR: &str = "dist";

/// Default value used by clap for `--port`. Same rationale as
/// [`CLAP_DEFAULT_OUTDIR`] — when the user did not pass `--port`, we let the
/// config's `port` win.
const CLAP_DEFAULT_PORT: u16 = 4321;

pub async fn run(args: &PreviewArgs) -> anyhow::Result<()> {
    // Resolve the project root from the current working directory and load
    // the project config (if any). A missing config file is fine — it
    // returns `Config::default()`. Any *real* error (e.g. invalid JSON,
    // unsupported zfb.config.ts) is surfaced via the output helpers and
    // propagated so `main()` can exit non-zero.
    let project_root = std::env::current_dir().context("failed to read current working dir")?;
    let cfg = match config::load_from_dir(&project_root).await {
        Ok(cfg) => cfg,
        Err(err) => {
            output::error(&output::format_error(&err));
            return Err(err);
        }
    };

    // CLI `--outdir` wins over config `outDir`. We can only tell whether the
    // CLI value is "explicit" by comparing against clap's default literal, so
    // both the literal and the type stay in sync with `PreviewArgs::outdir`.
    let outdir = if args.outdir == PathBuf::from(CLAP_DEFAULT_OUTDIR) {
        cfg.out_dir.clone()
    } else {
        args.outdir.clone()
    };
    // Resolve a relative outdir against the project root so the
    // existence check (and ServeDir) operate on an unambiguous path.
    let outdir = resolve_under_root(&project_root, &outdir);

    // CLI `--port` wins over config `port`. Same fallback rule as outdir.
    let port = if args.port == CLAP_DEFAULT_PORT {
        cfg.port
    } else {
        args.port
    };

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

    output::ready(&format!("http://localhost:{}", port));

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
    fn resolve_under_root_passes_absolute_paths_through() {
        let root = Path::new("/tmp/project");
        let abs = if cfg!(windows) {
            PathBuf::from("C:/elsewhere/dist")
        } else {
            PathBuf::from("/elsewhere/dist")
        };
        assert_eq!(resolve_under_root(root, &abs), abs);
    }

    #[test]
    fn clap_defaults_match_cli_struct() {
        // Defensive: if `PreviewArgs` ever changes its clap defaults, the
        // CLI-vs-config precedence logic in `run()` would silently break.
        // This keeps the literals here honest.
        use clap::Parser;

        #[derive(Parser)]
        struct Wrap {
            #[command(flatten)]
            args: PreviewArgs,
        }

        let parsed = Wrap::try_parse_from(["zfb-preview-test"]).expect("defaults parse");
        assert_eq!(parsed.args.outdir, PathBuf::from(CLAP_DEFAULT_OUTDIR));
        assert_eq!(parsed.args.port, CLAP_DEFAULT_PORT);
    }
}
