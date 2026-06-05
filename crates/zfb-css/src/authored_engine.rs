//! Tailwind-free CSS engine: pass authored global CSS through verbatim.
//!
//! When a project sets `tailwind: { enabled: false }` in its config it
//! wants to opt out of the Tailwind layers — the `@import "tailwindcss"`,
//! the `@source` utility scan, the preflight/reset, and the subprocess —
//! while keeping its authored global stylesheet and CSS Modules.
//!
//! [`AuthoredCssEngine`] is the engine half of that path. It implements
//! [`crate::CssEngine`] by returning a fixed CSS string (the project's
//! authored `styles/global.css`, or empty when none exists) without
//! spawning any subprocess or synthesising any Tailwind directives. The
//! rest of the pipeline — CSS Modules compilation, concatenation,
//! hashing, asset emission — is engine-agnostic and runs unchanged, so
//! the disabled path reuses [`crate::CssPipeline`] instead of
//! hand-rolling its own combine + hash logic.

use std::path::PathBuf;

use anyhow::Result;

use crate::engine::CssEngine;

/// A [`CssEngine`] that emits a pre-supplied authored CSS string and runs
/// no subprocess. Used for the `tailwind.enabled = false` path so the
/// authored global stylesheet still reaches the combined output while the
/// Tailwind import/scan/preflight are skipped entirely.
#[derive(Debug, Clone, Default)]
pub struct AuthoredCssEngine {
    css: String,
}

impl AuthoredCssEngine {
    /// Construct an engine that returns `css` verbatim from
    /// [`CssEngine::produce_utility_css`]. Pass the contents of the
    /// project's authored global stylesheet, or the empty string when the
    /// project has none.
    pub fn new(css: impl Into<String>) -> Self {
        Self { css: css.into() }
    }
}

impl CssEngine for AuthoredCssEngine {
    fn produce_utility_css(&self, _sources: &[PathBuf]) -> Result<String> {
        Ok(self.css.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_authored_css_verbatim() {
        let engine = AuthoredCssEngine::new("body { margin: 0; }");
        let out = engine.produce_utility_css(&[]).unwrap();
        assert_eq!(out, "body { margin: 0; }");
    }

    #[test]
    fn empty_when_no_authored_css() {
        let engine = AuthoredCssEngine::default();
        let out = engine.produce_utility_css(&[]).unwrap();
        assert!(out.is_empty());
    }
}
