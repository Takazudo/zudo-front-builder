//! zfb — zudo-front-builder library crate.
//!
//! This crate exposes the CLI parser, command dispatchers, configuration
//! loader, and structured output helpers used by the `zfb` binary.

pub mod cli;
pub mod commands;
pub mod config;
pub mod diagnostics;
mod node_resolve;
pub(crate) mod output;
pub mod render_pipeline;
// V8-bearing adapters (issue #371, sub-task 4.1a). Compiled in only
// when the `embed_v8` cargo feature is on; without the feature, the
// dispatch sites in `commands::dev` / `commands::build` /
// `render_pipeline` fall back to feature-gated stubs that fail loudly
// if a project unexpectedly hits an SSR-requiring path.
#[cfg(feature = "embed_v8")]
pub(crate) mod ssr_adapter;
#[cfg(feature = "embed_v8")]
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
