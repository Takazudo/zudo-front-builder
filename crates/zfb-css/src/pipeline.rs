//! Hashing, asset emission, and the top-level [`CssPipeline`].
//!
//! Stage 3 of the CSS pipeline:
//!
//! 1. Engine output (Tailwind utilities) and CSS Modules output are
//!    concatenated, in that order, separated by `\n`.
//! 2. The combined bytes are hashed with SHA-256, and the first 8 hex
//!    characters become the asset filename suffix (`styles-{hash}.css`).
//! 3. The file is written under
//!    `{output_root}/assets/styles-{hash}.css`. Default `output_root` is
//!    `dist/`.
//!
//! [`link_href`] derives the URL for the renderer to inject as
//! `<link href="...">`. We do **not** inject HTML here — that's the
//! renderer's responsibility (Epic 3 / Epic 7 wiring).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::engine::CssEngine;
use crate::modules::{CssModulesConfig, CssModulesProcessor};

/// Configuration for [`CssPipeline::new`].
#[derive(Debug, Clone)]
pub struct CssPipelineConfig {
    /// Source files to scan for utility classes (passed to the engine).
    pub sources: Vec<PathBuf>,

    /// CSS Modules files to compile. Order is significant: it determines
    /// the order of concatenation in the global asset and therefore the
    /// hash.
    pub css_modules: Vec<PathBuf>,

    /// Where to write the global asset. The pipeline emits
    /// `{output_root}/assets/styles-{hash}.css` under this directory.
    /// Default: `dist/`.
    pub output_root: PathBuf,

    /// Public base URL prefix for the asset, e.g. `"/"` (default) or
    /// `"https://cdn.example.com/"`. Used by [`link_href`].
    pub base_url: String,

    /// CSS Modules processing config.
    pub modules_config: CssModulesConfig,
}

impl Default for CssPipelineConfig {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            css_modules: Vec::new(),
            output_root: PathBuf::from("dist"),
            base_url: "/".to_string(),
            modules_config: CssModulesConfig::default(),
        }
    }
}

/// Result of running [`CssPipeline::build`].
#[derive(Debug, Clone)]
pub struct CssPipelineOutput {
    /// 8-character hex hash (first 8 chars of SHA-256 of the combined CSS).
    pub hash: String,

    /// The full combined CSS string.
    pub css: String,

    /// Absolute or output-root-relative path the asset was written to.
    /// Convention: `{output_root}/assets/styles-{hash}.css`.
    pub asset_path: PathBuf,

    /// Per-file CSS Modules class-name maps. See
    /// [`crate::modules::CssModulesOutput::class_maps`].
    pub class_maps: HashMap<PathBuf, HashMap<String, String>>,
}

/// Top-level CSS pipeline.
///
/// Generic over the engine type so callers can swap [`crate::TailwindSubprocessEngine`]
/// for [`crate::NativeRustEngine`] (or a test double) without touching the
/// pipeline code.
pub struct CssPipeline<E: CssEngine> {
    engine: E,
    modules: CssModulesProcessor,
    config: CssPipelineConfig,
}

impl<E: CssEngine> CssPipeline<E> {
    /// Construct a new pipeline.
    pub fn new(engine: E, config: CssPipelineConfig) -> Self {
        let modules = CssModulesProcessor::new(config.modules_config.clone());
        Self {
            engine,
            modules,
            config,
        }
    }

    /// Run all stages: engine → CSS Modules → hash → write.
    pub fn build(&self) -> Result<CssPipelineOutput> {
        let tailwind = self
            .engine
            .produce_utility_css(&self.config.sources)
            .context("CSS engine stage failed")?;

        let modules = self
            .modules
            .process(&self.config.css_modules)
            .context("CSS Modules stage failed")?;

        let combined = combine(&tailwind, &modules.css);
        let hash = hash_8(&tailwind, &modules.css);
        let asset_path = self
            .config
            .output_root
            .join("assets")
            .join(format!("styles-{hash}.css"));

        if let Some(parent) = asset_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        std::fs::write(&asset_path, combined.as_bytes())
            .with_context(|| format!("failed to write {}", asset_path.display()))?;

        Ok(CssPipelineOutput {
            hash,
            css: combined,
            asset_path,
            class_maps: modules.class_maps,
        })
    }

    /// Borrow the underlying config.
    pub fn config(&self) -> &CssPipelineConfig {
        &self.config
    }
}

/// Combine engine output + CSS Modules output exactly as the hashing stage
/// will see it. Exposed as a free function so tests and the hashing helper
/// agree on the canonical form.
pub(crate) fn combine(tailwind: &str, modules: &str) -> String {
    let mut out = String::with_capacity(tailwind.len() + modules.len() + 1);
    out.push_str(tailwind);
    out.push('\n');
    out.push_str(modules);
    out
}

/// Compute the 8-char hex hash for the given (tailwind, modules) pair.
///
/// The hash is the first 8 characters of `sha256(tailwind + "\n" + modules)`
/// in lowercase hex. Using a fixed separator means appending a class to one
/// half is distinguishable from prepending it to the other.
pub fn hash_8(tailwind: &str, modules: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(tailwind.as_bytes());
    hasher.update(b"\n");
    hasher.update(modules.as_bytes());
    let digest = hasher.finalize();
    let full = hex::encode(digest);
    full[..8].to_string()
}

/// Build the public URL for the global CSS asset.
///
/// `base_url` is concatenated with the asset path's filename portion under
/// an `assets/` segment. `base_url` is normalised to end with `/`.
///
/// Examples:
/// ```
/// use std::path::PathBuf;
/// use zfb_css::link_href;
///
/// let p = PathBuf::from("dist/assets/styles-abc12345.css");
/// assert_eq!(link_href("/", &p), "/assets/styles-abc12345.css");
/// assert_eq!(
///     link_href("https://cdn.example.com", &p),
///     "https://cdn.example.com/assets/styles-abc12345.css",
/// );
/// ```
pub fn link_href(base_url: &str, asset_path: &Path) -> String {
    let filename = asset_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let trimmed = base_url.trim_end_matches('/');
    format!("{trimmed}/assets/{filename}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_for_identical_inputs() {
        let a = hash_8(".a{color:red}", ".b{color:blue}");
        let b = hash_8(".a{color:red}", ".b{color:blue}");
        assert_eq!(a, b);
        assert_eq!(a.len(), 8);
    }

    #[test]
    fn hash_changes_when_a_class_changes() {
        let before = hash_8(".a{color:red}", ".b{color:blue}");
        let after = hash_8(".a{color:green}", ".b{color:blue}");
        assert_ne!(
            before, after,
            "changing a class in the tailwind half must change the hash"
        );

        let after2 = hash_8(".a{color:red}", ".b{color:teal}");
        assert_ne!(
            before, after2,
            "changing a class in the modules half must change the hash"
        );
    }

    #[test]
    fn hash_uses_separator_to_avoid_boundary_collisions() {
        // Without a separator, ("ab", "cd") and ("a", "bcd") would hash the
        // same. With our "\n" separator they must differ.
        let a = hash_8("ab", "cd");
        let b = hash_8("a", "bcd");
        assert_ne!(a, b);
    }

    #[test]
    fn link_href_normalises_trailing_slash() {
        let p = PathBuf::from("dist/assets/styles-deadbeef.css");
        assert_eq!(link_href("/", &p), "/assets/styles-deadbeef.css");
        assert_eq!(link_href("", &p), "/assets/styles-deadbeef.css");
        assert_eq!(
            link_href("https://cdn.example.com/", &p),
            "https://cdn.example.com/assets/styles-deadbeef.css"
        );
        assert_eq!(
            link_href("https://cdn.example.com", &p),
            "https://cdn.example.com/assets/styles-deadbeef.css"
        );
    }
}
