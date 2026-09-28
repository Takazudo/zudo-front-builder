//! Parsed command skeleton for zudo-wind inspection commands.

use anyhow::{bail, Result};

use crate::cli::WindArgs;

/// Dispatch the zudo-wind command family.
pub async fn run(_args: &WindArgs) -> Result<()> {
    bail!("zfb wind is not implemented yet")
}
