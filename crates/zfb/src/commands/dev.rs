//! `zfb dev` command — stub implementation.
//!
//! Contract for Wave 2 (cmd-dev agent):
//!   pub async fn run(args: &crate::cli::DevArgs) -> anyhow::Result<()>
//!
//! `args.port` is the bind port (default 3000), `args.host` is the bind
//! interface (default "localhost").

use crate::cli::DevArgs;

pub async fn run(_args: &DevArgs) -> anyhow::Result<()> {
    anyhow::bail!("zfb dev: not yet implemented")
}
