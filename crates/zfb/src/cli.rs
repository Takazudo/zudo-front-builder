//! CLI argument parsing for the `zfb` binary.
//!
//! The top-level [`Cli`] type holds the parsed clap state. Each subcommand
//! variant in [`Command`] wraps a dedicated `*Args` struct so that command
//! modules in `crate::commands` can take a single `&FooArgs` parameter rather
//! than a long parameter list. Keep these struct shapes stable — adding
//! fields is fine, but renaming or removing fields is a contract break for
//! each `commands/<name>.rs` handler.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Top-level CLI for the `zfb` binary.
#[derive(Debug, Parser)]
// ZFB_RELEASE_VERSION is set by the release CI (= packages/zfb/package.json version).
// Local builds without the env var fall back to CARGO_PKG_VERSION (0.0.0 placeholder).
#[command(name = "zfb", version = option_env!("ZFB_RELEASE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")), about = "zudo-front-builder")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// Subcommand dispatch. Each variant carries its own argument struct so that
/// command implementations stay decoupled from clap.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Scaffold a new project from a template.
    New(NewArgs),
    /// Run the local development server.
    Dev(DevArgs),
    /// Build the project for production.
    Build(BuildArgs),
    /// Preview a previously built project.
    Preview(PreviewArgs),
    /// Typecheck the project and validate content collections against
    /// their schemas. Equivalent in spirit to Astro's `astro check`.
    Check(CheckArgs),
}

/// Arguments for `zfb new`.
#[derive(Debug, Args)]
pub struct NewArgs {
    /// Name of the new project (used as the destination directory).
    pub name: String,

    /// Template to scaffold from. v0 ships a single template:
    /// `basic-blog`, sourced from `crates/zfb/templates/basic-blog/` and
    /// baked into the binary at compile time.
    #[arg(long, default_value = "basic-blog")]
    pub template: String,
}

/// Arguments for `zfb dev`.
///
/// `host` and `port` are intentionally `Option<_>` (no clap `default_value`)
/// so the command body can layer "CLI > config > built-in default" cleanly.
/// Adding a clap default would erase the distinction between "user passed
/// --port 8080" and "user accepted the default 3000".
///
/// The `after_help` block documents the lazy dev-render environment
/// switches (issue #1027): the dev server defaults to lazy rendering
/// (changed routes are marked stale and re-rendered on first request);
/// `ZFB_DEV_EAGER=1` is the user-facing escape hatch back to fully
/// eager per-tick rendering, and `ZFB_LAZY_DEV_RENDER=0|1` is the
/// precise override that wins over `ZFB_DEV_EAGER` when both are set.
/// `ZFB_DEV_BOOT_LAZY=1` (issue #1057) additionally defers the BOOT
/// render: a valid prebuilt `dist/` is served immediately and each route
/// re-renders on its first request. `ZFB_DEV_DEFER_BUNDLE=0` (issue #1188)
/// opts out of the #1182 bundle deferral under boot-lazy, so the renderer is
/// built before bind (no SSR-only 404 window) at the cost of a slower
/// first-accept.
#[derive(Debug, Args)]
#[command(after_help = "Environment variables:
  ZFB_DEV_EAGER=1          Disable lazy dev rendering: re-render every affected
                           route eagerly on each file change (escape hatch
                           restoring the pre-lazy behaviour).
  ZFB_LAZY_DEV_RENDER=0|1  Precise override of the lazy dev-render switch
                           (1|true forces lazy, 0|false forces eager). Takes
                           precedence over ZFB_DEV_EAGER when both are set.
  ZFB_DEV_BOOT_LAZY=1      Opt-in fast boot: when a valid prebuilt `dist/` is
                           present, serve it immediately and defer per-route
                           rendering to the first request, instead of rendering
                           every route at boot. Requires lazy rendering (no-op
                           when ZFB_DEV_EAGER is set). Off by default.
  ZFB_DEV_DEFER_BUNDLE=0   Opt out of the #1182 boot-lazy bundle deferral: build
                           the renderer before bind (no SSR-only 404 window)
                           at the cost of a slower first-accept. On by default
                           when boot-lazy is active.")]
pub struct DevArgs {
    /// Port to bind the dev server to. Falls back to `port` from
    /// `zfb.config.json`, then to `3000`.
    #[arg(long)]
    pub port: Option<u16>,

    /// Host interface to bind the dev server to. Falls back to `host`
    /// from `zfb.config.json`, then to `localhost`. Bare `--host` (no value)
    /// is a Vite-style shortcut for `0.0.0.0` (expose to the LAN); an absent
    /// flag stays `None` so the "CLI > config > built-in default" layering
    /// above is preserved.
    #[arg(long, num_args = 0..=1, default_missing_value = "0.0.0.0")]
    pub host: Option<String>,
}

/// Arguments for `zfb build`.
#[derive(Debug, Args)]
pub struct BuildArgs {
    /// Output directory for the production build.
    #[arg(long, default_value = "dist")]
    pub outdir: PathBuf,
}

/// Arguments for `zfb preview`.
///
/// `port` and `host` are `Option<_>` for the same reason as the `DevArgs`
/// fields — see the doc-comment there — so the command body can layer
/// "CLI > config > built-in default" cleanly. `outdir` keeps a clap default
/// because the preview command does not consult config for it today (config's
/// `outDir` is already wired through the build command).
#[derive(Debug, Args)]
pub struct PreviewArgs {
    /// Port to bind the preview server to. Falls back to `port` from
    /// `zfb.config.json`, then to `4321`.
    #[arg(long)]
    pub port: Option<u16>,

    /// Host interface to bind the preview server to. Falls back to `host`
    /// from `zfb.config.json`, then to `localhost`. Pass `0.0.0.0` to expose
    /// the built site to other devices on the LAN — or just bare `--host`
    /// (no value), a Vite-style shortcut for the same `0.0.0.0`. An absent
    /// flag stays `None` so the "CLI > config > built-in default" layering
    /// is preserved.
    #[arg(long, num_args = 0..=1, default_missing_value = "0.0.0.0")]
    pub host: Option<String>,

    /// Directory to serve the previously built artifacts from. In adapter
    /// mode this is only an existence pre-check — `wrangler dev` serves the
    /// directories named in the project's wrangler config, and a non-default
    /// value here triggers a warning to that effect.
    #[arg(long, default_value = "dist")]
    pub outdir: PathBuf,
}

/// Arguments for `zfb check`.
///
/// Two failure modes:
///
/// 1. TypeScript errors — `tsc --noEmit` is invoked as a subprocess on
///    the project. Anything tsc would flag in normal CI.
/// 2. Content collection schema violations — every entry's frontmatter
///    is validated against the JSON Schema declared in
///    `zfb.config.json`'s `collections[].schema` field (when present).
///
/// Either failure mode produces a non-zero exit. `--skip-tsc` is
/// useful when the project hasn't installed TypeScript yet (or for
/// schema-only CI lanes).
#[derive(Debug, Args)]
pub struct CheckArgs {
    /// Skip the `tsc --noEmit` subprocess. Schema validation still runs.
    /// Useful when the project has no TypeScript dependency installed
    /// yet but still wants schema enforcement in CI.
    #[arg(long)]
    pub skip_tsc: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev_host(argv: &[&str]) -> Option<String> {
        match Cli::try_parse_from(argv).expect("parse").command {
            Command::Dev(args) => args.host,
            other => panic!("expected dev subcommand, got {other:?}"),
        }
    }

    fn preview_host(argv: &[&str]) -> Option<String> {
        match Cli::try_parse_from(argv).expect("parse").command {
            Command::Preview(args) => args.host,
            other => panic!("expected preview subcommand, got {other:?}"),
        }
    }

    // Absent `--host` must stay `None` so the command body's
    // "CLI > config > built-in default" layering still kicks in.
    #[test]
    fn dev_host_absent_is_none() {
        assert_eq!(dev_host(&["zfb", "dev"]), None);
    }

    // Bare `--host` is the Vite-style LAN shortcut → `0.0.0.0`.
    #[test]
    fn dev_host_bare_defaults_to_all_interfaces() {
        assert_eq!(dev_host(&["zfb", "dev", "--host"]), Some("0.0.0.0".into()));
    }

    #[test]
    fn dev_host_explicit_value_is_preserved() {
        assert_eq!(
            dev_host(&["zfb", "dev", "--host", "1.2.3.4"]),
            Some("1.2.3.4".into())
        );
    }

    #[test]
    fn preview_host_absent_is_none() {
        assert_eq!(preview_host(&["zfb", "preview"]), None);
    }

    #[test]
    fn preview_host_bare_defaults_to_all_interfaces() {
        assert_eq!(
            preview_host(&["zfb", "preview", "--host"]),
            Some("0.0.0.0".into())
        );
    }

    #[test]
    fn preview_host_explicit_value_is_preserved() {
        assert_eq!(
            preview_host(&["zfb", "preview", "--host", "10.0.0.1"]),
            Some("10.0.0.1".into())
        );
    }

    /// Issue #1027 — `zfb dev --help` documents the lazy-render env
    /// switches: the `ZFB_DEV_EAGER=1` escape hatch and the
    /// `ZFB_LAZY_DEV_RENDER` precise override (with its precedence).
    #[test]
    fn dev_help_documents_lazy_render_env_switches() {
        use clap::CommandFactory;
        let mut cmd = Cli::command();
        let dev = cmd
            .find_subcommand_mut("dev")
            .expect("dev subcommand exists");
        let help = dev.render_long_help().to_string();
        assert!(
            help.contains("ZFB_DEV_EAGER=1"),
            "dev help must document the ZFB_DEV_EAGER escape hatch:\n{help}"
        );
        assert!(
            help.contains("ZFB_LAZY_DEV_RENDER"),
            "dev help must document the ZFB_LAZY_DEV_RENDER override:\n{help}"
        );
        assert!(
            help.contains("precedence over ZFB_DEV_EAGER"),
            "dev help must state the precedence rule:\n{help}"
        );
    }

    /// Issue #1057 — `zfb dev --help` documents the opt-in boot-lazy switch.
    #[test]
    fn dev_help_documents_boot_lazy_env_switch() {
        use clap::CommandFactory;
        let mut cmd = Cli::command();
        let dev = cmd
            .find_subcommand_mut("dev")
            .expect("dev subcommand exists");
        let help = dev.render_long_help().to_string();
        assert!(
            help.contains("ZFB_DEV_BOOT_LAZY=1"),
            "dev help must document the ZFB_DEV_BOOT_LAZY opt-in switch:\n{help}"
        );
    }

    /// Issue #1188 — `zfb dev --help` documents the bundle-deferral opt-out.
    #[test]
    fn dev_help_documents_defer_bundle_optout() {
        use clap::CommandFactory;
        let mut cmd = Cli::command();
        let dev = cmd
            .find_subcommand_mut("dev")
            .expect("dev subcommand exists");
        let help = dev.render_long_help().to_string();
        assert!(
            help.contains("ZFB_DEV_DEFER_BUNDLE=0"),
            "dev help must document the ZFB_DEV_DEFER_BUNDLE opt-out:\n{help}"
        );
    }
}
