//! `zfb preview` command — stub implementation.
//!
//! Contract for Wave 2 (cmd-preview agent):
//!   pub async fn run(args: &crate::cli::PreviewArgs) -> anyhow::Result<()>
//!
//! `args.port` is the bind port (default 4321), `args.outdir` is the directory
//! to serve from (default "dist").

use crate::cli::PreviewArgs;

pub async fn run(_args: &PreviewArgs) -> anyhow::Result<()> {
    anyhow::bail!("zfb preview: not yet implemented")
}
