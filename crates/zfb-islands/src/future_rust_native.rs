//! Placeholder Rust-native islands bundler.
//!
//! This module exists so the [`crate::ClientBundler`] swap-in story is
//! visible in code, not just in docs. [`NativeRustBundler`] implements the
//! trait — but every method returns a "not yet implemented" error.
//!
//! ## Roadmap
//!
//! esbuild ships as a Go-built CLI. A pure-Rust port (oxc_minifier on top
//! of oxc_resolver, rolldown, mako, …) is out of scope for the v0 build
//! pipeline (Epic 6 / Sub 2). When one becomes viable, we want the swap
//! to be a one-line change at the call site:
//!
//! ```ignore
//! // before:
//! let bundler = EsbuildSubprocessBundler::with_default_config();
//! // after:
//! let bundler = NativeRustBundler::default();
//! ```
//!
//! Until then, this stub guards the API surface against drift: if someone
//! widens [`crate::ClientBundler`] without updating both bundlers, the
//! build breaks here, not in production. (Same rationale as
//! `zfb_css::native_engine::NativeRustEngine`.)

use anyhow::{anyhow, Result};

use crate::bundler::{BundleConfig, BundleOutput, ClientBundler, Island};

/// A non-functional placeholder for a future pure-Rust islands bundler.
/// Construction succeeds; every method returns an error.
///
/// See the module-level docs for the swap-in story.
#[derive(Debug, Default, Clone)]
pub struct NativeRustBundler {
    _private: (),
}

impl NativeRustBundler {
    /// Construct a new instance. (Cheap: this bundler carries no state.)
    pub fn new() -> Self {
        Self::default()
    }
}

impl ClientBundler for NativeRustBundler {
    fn bundle(&self, _islands: &[Island], _config: &BundleConfig) -> Result<BundleOutput> {
        Err(anyhow!(
            "NativeRustBundler: not yet implemented. \
             Use EsbuildSubprocessBundler (Sub 2) for now; \
             see crate::future_rust_native for the roadmap."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_not_implemented_error() {
        let bundler = NativeRustBundler::new();
        let err = bundler
            .bundle(&[], &BundleConfig::default())
            .err()
            .expect("must return an error");
        let msg = format!("{err}");
        assert!(msg.contains("not yet implemented"), "msg = {msg}");
    }
}
