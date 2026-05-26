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
#[derive(Debug, Args)]
pub struct DevArgs {
    /// Port to bind the dev server to. Falls back to `port` from
    /// `zfb.config.json`, then to `3000`.
    #[arg(long)]
    pub port: Option<u16>,

    /// Host interface to bind the dev server to. Falls back to `host`
    /// from `zfb.config.json`, then to `localhost`.
    #[arg(long)]
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
    /// the built site to other devices on the LAN.
    #[arg(long)]
    pub host: Option<String>,

    /// Directory to serve the previously built artifacts from.
    #[arg(long, default_value = "dist")]
    pub outdir: PathBuf,
}

/// Arguments for `zfb check`.
///
/// Two failure modes:
///
/// 1. TypeScript errors — `tsc --noEmit` is invoked as a subprocess on
///    the project. Anything tsc would flag in normal CI flagsHERE.
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
