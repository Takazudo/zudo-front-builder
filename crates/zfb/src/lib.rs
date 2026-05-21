//! zfb — zudo-front-builder library crate.
//!
//! This crate exposes the CLI parser, command dispatchers, configuration
//! loader, and structured output helpers used by the `zfb` binary.

pub mod cli;
pub mod commands;
pub mod config;
pub mod diagnostics;
pub(crate) mod output;
pub mod render_pipeline;
pub(crate) mod ssr_adapter;
pub(crate) mod v8_host_adapter;

// Re-export the public dynamic-route planning types so adapter authors can
// build on them without pulling in the `render_pipeline` module directly.
pub use render_pipeline::{DeferredDynamicRoute, PendingDynamicRoute};

/// Render and print an [`anyhow::Error`] to stderr, prefixed with a red `✗`.
///
/// Used by the `zfb` binary entry point to report top-level command failures.
/// Wraps [`output::format_error`] + [`output::error`] so the binary does not
/// need to import the internal `output` module directly.
pub fn report_error(err: &anyhow::Error) {
    output::error(output::format_error(err).trim_end());
}
