//! `zfb new` command — stub implementation.
//!
//! Contract for Wave 2 (scaffold-new agent):
//!   pub async fn run(args: &crate::cli::NewArgs) -> anyhow::Result<()>
//!
//! `args.name` is the destination directory, `args.template` is the template
//! identifier (defaulting to `"default"`).

use crate::cli::NewArgs;

pub async fn run(_args: &NewArgs) -> anyhow::Result<()> {
    anyhow::bail!("zfb new: not yet implemented")
}
