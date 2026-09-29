use std::path::PathBuf;

use crate::CssEngineOutput;
use anyhow::Result;

/// Produces a structured CSS result for the shared pipeline.
pub trait CssEngine {
    fn produce_utility_css(&self, sources: &[PathBuf]) -> Result<CssEngineOutput>;
}

/// Conventional project source roots shared by build and standalone CSS.
pub const DEFAULT_CONTENT_ROOTS: &[&str] = &["pages", "components", "layouts", "content", "src"];
