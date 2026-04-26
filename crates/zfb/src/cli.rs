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
#[derive(Debug, Args)]
pub struct DevArgs {
    /// Port to bind the dev server to.
    #[arg(long, default_value_t = 3000)]
    pub port: u16,

    /// Host interface to bind the dev server to.
    #[arg(long, default_value = "localhost")]
    pub host: String,
}

/// Arguments for `zfb build`.
#[derive(Debug, Args)]
pub struct BuildArgs {
    /// Output directory for the production build.
    #[arg(long, default_value = "dist")]
    pub outdir: PathBuf,
}

/// Arguments for `zfb preview`.
#[derive(Debug, Args)]
pub struct PreviewArgs {
    /// Port to bind the preview server to.
    #[arg(long, default_value_t = 4321)]
    pub port: u16,

    /// Directory to serve the previously built artifacts from.
    #[arg(long, default_value = "dist")]
    pub outdir: PathBuf,
}
