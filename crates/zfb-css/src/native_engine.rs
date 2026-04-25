//! Placeholder Rust-native CSS engine.
//!
//! This module exists so the [`crate::CssEngine`] swap-in story is visible
//! in code, not just in docs. [`NativeRustEngine`] implements the trait —
//! but every method returns a "not yet implemented" error.
//!
//! ## Roadmap
//!
//! Tailwind v4 ships a Go-built CLI ("oxide" engine). A pure-Rust port is
//! out of scope for the v0 build pipeline (Epic 4) — but when one becomes
//! viable, we want the swap to be a one-line change at the `CssPipeline`
//! constructor:
//!
//! ```ignore
//! // before:
//! let engine = TailwindSubprocessEngine::with_default_config();
//! // after:
//! let engine = NativeRustEngine::default();
//! ```
//!
//! Until then, this stub guards the API surface against drift: if someone
//! widens [`crate::CssEngine`] without updating both engines, the build
//! breaks here, not in production.

use std::path::PathBuf;

use anyhow::{anyhow, Result};

use crate::engine::CssEngine;

/// A non-functional placeholder for a future pure-Rust Tailwind-equivalent
/// engine. Construction succeeds; every method returns an error.
///
/// See the module-level docs for the swap-in story.
#[derive(Debug, Default, Clone)]
pub struct NativeRustEngine {
    _private: (),
}

impl NativeRustEngine {
    /// Construct a new instance. (Cheap: this engine carries no state.)
    pub fn new() -> Self {
        Self::default()
    }
}

impl CssEngine for NativeRustEngine {
    fn produce_utility_css(&self, _sources: &[PathBuf]) -> Result<String> {
        Err(anyhow!(
            "NativeRustEngine: not yet implemented. \
             Use TailwindSubprocessEngine for now; see crate::native_engine for the roadmap."
        ))
    }
}
