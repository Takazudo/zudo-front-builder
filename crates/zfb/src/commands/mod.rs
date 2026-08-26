//! Command module declarations.
//!
//! Each submodule exposes an `async fn run(args: &crate::cli::FooArgs) ->
//! anyhow::Result<()>` entrypoint that `main.rs` dispatches to.

pub mod build;
pub mod bundler_input;
pub mod check;
pub(crate) mod css_support;
pub mod dev;
pub(crate) mod dev_companion_ledger;
pub(crate) mod html_minify;
pub mod island_marker_check;
pub mod link_base_rewrite;
pub(crate) mod mermaid_preserve;
pub mod new;
pub mod package_routes;
pub mod plugins;
pub mod preview;
pub(crate) mod render_artifact;
pub mod resolve;
pub(crate) mod watcher_liveness_probe;
