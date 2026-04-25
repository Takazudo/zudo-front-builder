//! Placeholder Rust-native client bundler.
//!
//! This module exists so the [`crate::ClientBundler`] swap-in story is
//! visible in code, not just in docs. [`NativeRustBundler`] implements the
//! trait — but every method returns a "not yet implemented" error.
//!
//! ## Roadmap
//!
//! esbuild ships a Go-built CLI. A pure-Rust replacement (swc-bundler,
//! turbopack-style, or our own) is out of scope for the v0 build pipeline
//! (Epic 6) — but when one becomes viable, we want the swap to be a
//! one-line change at the call site:
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
//! build breaks here, not in production.
//!
//! Module name mirrors `zfb_css::native_engine` — Sub 1 named the same
//! analog `future_rust_native` here, so that name is canonical for
//! `zfb_islands`.

use anyhow::{anyhow, Result};

use crate::bundler::{BundleConfig, BundleOutput, ClientBundler, Island};

/// A non-functional placeholder for a future pure-Rust esbuild-equivalent
/// bundler. Construction succeeds; every method returns an error.
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
             Use EsbuildSubprocessBundler for now; see crate::future_rust_native for the roadmap."
        ))
    }
}
