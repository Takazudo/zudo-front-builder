//! CLI argument parsing for the `zfb` binary.
//!
//! The top-level [`Cli`] type holds the parsed clap state. Each subcommand
//! variant in [`Command`] wraps a dedicated `*Args` struct so that command
//! modules in `crate::commands` can take a single `&FooArgs` parameter rather
//! than a long parameter list. Wave 2 agents must keep these struct shapes
//! stable — adding fields is fine, but renaming or removing fields breaks the
//! contract documented in each `commands/<name>.rs` stub.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Top-level CLI for the `zfb` binary.
#[derive(Debug, Parser)]
#[command(name = "zfb", version, about = "zudo-front-builder")]
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
}

/// Arguments for `zfb new`.
#[derive(Debug, Args)]
pub struct NewArgs {
    /// Name of the new project (used as the destination directory).
    pub name: String,

    /// Template to scaffold from.
    #[arg(long, default_value = "default")]
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
/// `port` is `Option<u16>` for the same reason as `DevArgs::port` — see
/// the doc-comment there. `outdir` keeps a clap default because the
/// preview command does not consult config for it today (config's
/// `outDir` is already wired through the build command).
#[derive(Debug, Args)]
pub struct PreviewArgs {
    /// Port to bind the preview server to. Falls back to `port` from
    /// `zfb.config.json`, then to `4321`.
    #[arg(long)]
    pub port: Option<u16>,

    /// Directory to serve the previously built artifacts from.
    #[arg(long, default_value = "dist")]
    pub outdir: PathBuf,
}
