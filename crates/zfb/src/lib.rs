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
// when the `direct-v8` cargo feature is on; without the feature, the
// dispatch sites in `commands::dev` / `commands::build` /
// `render_pipeline` fall back to feature-gated stubs that fail loudly
// if a project unexpectedly hits an SSR-requiring path.
#[cfg(feature = "direct-v8")]
pub(crate) mod dev_rewrite_prewarm;
#[cfg(feature = "direct-v8")]
pub(crate) mod lazy_render_adapter;
#[cfg(feature = "direct-v8")]
pub(crate) mod ssr_adapter;
#[cfg(feature = "direct-v8")]
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

/// Environment variables that selected a bundler backend before the esbuild
/// backend was removed (#4628). Native bundling is the only bundler, so a set
/// selector is a configuration this binary can no longer honor.
pub const REMOVED_BUNDLER_SELECTORS: [&str; 2] = ["ZFB_BUNDLER", "ZFB_ROLLDOWN_PROTOTYPE"];

/// Fail before any command runs when a removed bundler selector is set, to
/// any value, so a script that still asks for esbuild is not silently
/// built with another bundler.
pub fn reject_removed_bundler_selectors(
    env: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> anyhow::Result<()> {
    for name in REMOVED_BUNDLER_SELECTORS {
        if let Some(value) = env(name) {
            anyhow::bail!(
                "{name}={}: the esbuild bundler backend and its {name} selector were removed; \
                 zfb always bundles natively. Unset {name}.",
                value.to_string_lossy()
            );
        }
    }
    Ok(())
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
    use super::reject_removed_bundler_selectors;
    use std::ffi::OsString;

    #[test]
    fn accepts_an_environment_without_bundler_selectors() {
        assert!(reject_removed_bundler_selectors(|_| None).is_ok());
    }

    #[test]
    fn rejects_every_removed_selector_whatever_its_value() {
        for name in ["ZFB_BUNDLER", "ZFB_ROLLDOWN_PROTOTYPE"] {
            for value in ["esbuild", "rolldown", "0", "1", ""] {
                let error = reject_removed_bundler_selectors(|key| {
                    (key == name).then(|| OsString::from(value))
                })
                .unwrap_err();
                let message = format!("{error}");
                assert!(
                    message.starts_with(&format!("{name}={value}:")),
                    "{message}"
                );
                assert!(message.contains("esbuild bundler backend"), "{message}");
                assert!(message.contains("were removed"), "{message}");
                assert!(message.contains(&format!("Unset {name}")), "{message}");
            }
        }
    }
}
