//! Command module declarations.
//!
//! Each submodule exposes an `async fn run(args: &crate::cli::FooArgs) ->
//! anyhow::Result<()>` entrypoint that `main.rs` dispatches to.

pub mod build;
pub mod check;
pub mod dev;
pub mod new;
pub mod preview;
