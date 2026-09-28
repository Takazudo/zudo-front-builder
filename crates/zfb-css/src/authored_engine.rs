//! Authored-only CSS engine for `wind: false` projects.
//!
//! Passes authored global CSS and its companion assets through the shared
//! pipeline, which also handles CSS Modules and stylesheet emission.

use std::path::PathBuf;

use anyhow::Result;

use crate::engine::CssEngine;
use crate::{AuthoredCssBundle, CssEngineId, CssEngineOutput, CssInputDependency, PackageUrlAsset};

/// A [`CssEngine`] that emits a pre-supplied authored CSS string and runs
/// no subprocess. Used for the `wind: false` path.
#[derive(Debug, Clone, Default)]
pub struct AuthoredCssEngine {
    css: String,
    companions: Vec<PackageUrlAsset>,
    input_dependencies: Vec<CssInputDependency>,
}

impl AuthoredCssEngine {
    /// Construct an engine that returns `css` verbatim from
    /// [`CssEngine::produce_utility_css`]. Pass the contents of the
    /// project's authored global stylesheet, or the empty string when the
    /// project has none.
    pub fn new(css: impl Into<String>) -> Self {
        Self {
            css: css.into(),
            companions: Vec::new(),
            input_dependencies: Vec::new(),
        }
    }

    /// Construct an engine from the asset-aware authored bundle.
    pub fn with_bundle(bundle: AuthoredCssBundle) -> Self {
        Self {
            css: bundle.css,
            companions: bundle.companions,
            input_dependencies: bundle.input_dependencies,
        }
    }
}

impl CssEngine for AuthoredCssEngine {
    fn produce_utility_css(&self, _sources: &[PathBuf]) -> Result<CssEngineOutput> {
        let mut output = CssEngineOutput::new(self.css.clone(), CssEngineId::new("authored", None));
        output.companions = self.companions.clone();
        output.input_dependencies = self.input_dependencies.clone();
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_authored_css_verbatim() {
        let engine = AuthoredCssEngine::new("body { margin: 0; }");
        let out = engine.produce_utility_css(&[]).unwrap();
        assert_eq!(out.css, "body { margin: 0; }");
    }

    #[test]
    fn empty_when_no_authored_css() {
        let engine = AuthoredCssEngine::default();
        let out = engine.produce_utility_css(&[]).unwrap();
        assert!(out.css.is_empty());
    }
}
