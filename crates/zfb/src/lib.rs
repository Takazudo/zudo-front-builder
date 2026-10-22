//! zfb — zudo-front-builder library crate.
//!
//! This crate exposes the CLI parser, command dispatchers, configuration
//! loader, and structured output helpers used by the `zfb` binary.

pub mod bounded_join;
pub mod cli;
pub mod commands;
pub mod config;
pub mod diagnostics;
pub(crate) mod output;
pub mod process_supervisor;
pub mod render_pipeline;
// V8-bearing adapters (issue #371, sub-task 4.1a). Compiled in only
// when the `embed_v8` cargo feature is on; without the feature, the
// dispatch sites in `commands::dev` / `commands::build` /
// `render_pipeline` fall back to feature-gated stubs that fail loudly
// if a project unexpectedly hits an SSR-requiring path.
#[cfg(any(feature = "embed_v8", feature = "direct-v8"))]
pub(crate) mod dev_rewrite_prewarm;
#[cfg(any(feature = "embed_v8", feature = "direct-v8"))]
pub(crate) mod lazy_render_adapter;
#[cfg(any(feature = "embed_v8", feature = "direct-v8"))]
pub(crate) mod ssr_adapter;
#[cfg(any(feature = "embed_v8", feature = "direct-v8"))]
pub(crate) mod v8_host_adapter;

// Re-export the public dynamic-route planning types so adapter authors can
// build on them without pulling in the `render_pipeline` module directly.
pub use render_pipeline::{DeferredDynamicRoute, PendingDynamicRoute};

/// An intentional CLI exit that should not be rendered as an error.
#[derive(Debug)]
pub struct SilentExit(pub i32);

impl std::fmt::Display for SilentExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "exit with status {}", self.0)
    }
}

impl std::error::Error for SilentExit {}

/// Reject a `ZFB_BUNDLER` value this binary cannot honor before any command runs.
/// The bundler crates read the variable themselves and treat anything other
/// than `esbuild` as Rolldown, so a typo would otherwise pass silently.
pub fn check_bundler_selection(value: Option<&std::ffi::OsStr>) -> anyhow::Result<()> {
    let Some(value) = value else { return Ok(()) };
    match value.to_str() {
        Some("esbuild") => Ok(()),
        Some("rolldown") if cfg!(feature = "rolldown-prototype") => Ok(()),
        Some("rolldown") => anyhow::bail!(
            "ZFB_BUNDLER=rolldown: this zfb binary was built without the Rolldown bundler; unset ZFB_BUNDLER or set it to esbuild"
        ),
        _ => anyhow::bail!(
            "ZFB_BUNDLER={}: expected `rolldown` or `esbuild`",
            value.to_string_lossy()
        ),
    }
}

/// Render and print an [`anyhow::Error`] to stderr, prefixed with a red `✗`.
///
/// Used by the `zfb` binary entry point to report top-level command failures.
/// Wraps [`output::format_error`] + [`output::error`] so the binary does not
/// need to import the internal `output` module directly.
pub fn report_error(err: &anyhow::Error) {
    output::error(output::format_error(err).trim_end());
}

#[cfg(test)]
mod bundler_selection_tests {
    use super::check_bundler_selection;
    use std::ffi::OsStr;

    #[test]
    fn accepts_unset_and_esbuild_and_rejects_unknown_values() {
        assert!(check_bundler_selection(None).is_ok());
        assert!(check_bundler_selection(Some(OsStr::new("esbuild"))).is_ok());
        let error = check_bundler_selection(Some(OsStr::new("esbuld"))).unwrap_err();
        assert!(format!("{error}").contains("expected `rolldown` or `esbuild`"));
    }

    #[test]
    fn rolldown_needs_the_compiled_backend() {
        let result = check_bundler_selection(Some(OsStr::new("rolldown")));
        assert_eq!(result.is_ok(), cfg!(feature = "rolldown-prototype"));
    }
}
