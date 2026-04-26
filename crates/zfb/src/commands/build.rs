//! `zfb build` command — stub implementation.
//!
//! Contract for Wave 2 (cmd-build agent):
//!   pub async fn run(args: &crate::cli::BuildArgs) -> anyhow::Result<()>
//!
//! `args.outdir` is the production output directory (default "dist").

use crate::cli::BuildArgs;

pub async fn run(_args: &BuildArgs) -> anyhow::Result<()> {
    anyhow::bail!("zfb build: not yet implemented")
}
