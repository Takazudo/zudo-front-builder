//! CSS Modules processing via [`lightningcss`].
//!
//! Each `*.module.css` file is parsed with `lightningcss`'s built-in CSS
//! Modules support, which:
//!
//! - rewrites local class names to scoped, file-stable identifiers,
//! - returns the original-name → scoped-name map (we expose this to callers
//!   so JSX/TSX consumers can rewrite `className` references),
//! - emits a per-file source map in dev mode (deferred for now: gated by
//!   the `dev` flag in [`CssModulesConfig`] but only the merging is wired
//!   up; full source-map plumbing is part of Epic 4 follow-up).
//!
//! All processed module CSS is concatenated into one string ready for the
//! global stylesheet.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lightningcss::css_modules::{Config as LcssConfig, Pattern};
use lightningcss::stylesheet::{ParserOptions, PrinterOptions, StyleSheet};

/// Configuration for [`CssModulesProcessor`].
#[derive(Debug, Clone, Default)]
pub struct CssModulesConfig {
    /// When true, emit per-file source maps. When false (production), skip
    /// source-map generation entirely.
    ///
    /// Note: the v0 pipeline does **not** emit a *merged* source map for
    /// the global asset — that's deferred to v1.1 per the Epic 4 reviewers.
    /// Even in dev mode, we currently retain only the per-file maps; the
    /// hook-up to the renderer will land in a follow-up.
    pub dev: bool,

    /// Class-name pattern. Defaults to lightningcss's `[hash]_[local]`
    /// equivalent (its own default).
    pub pattern: Option<String>,
}

/// Output of [`CssModulesProcessor::process`].
#[derive(Debug, Clone, Default)]
pub struct CssModulesOutput {
    /// All processed module CSS, concatenated. Order matches the input
    /// `Vec<PathBuf>` so callers can produce stable hashes.
    pub css: String,

    /// Per-file class-name map: `module file → (original class → scoped class)`.
    ///
    /// Consumers (JSX/TSX module-graph rewriters) use this map to rewrite
    /// `import styles from "./foo.module.css"` references at build time.
    pub class_maps: HashMap<PathBuf, HashMap<String, String>>,
}

/// Compiles `*.module.css` files into scoped CSS plus a class-name map.
#[derive(Debug, Clone)]
pub struct CssModulesProcessor {
    config: CssModulesConfig,
}

impl CssModulesProcessor {
    /// Construct with the given config.
    pub fn new(config: CssModulesConfig) -> Self {
        Self { config }
    }

    /// Construct with the default config (production-style: no source maps).
    pub fn with_default_config() -> Self {
        Self::new(CssModulesConfig::default())
    }

    /// Borrow the underlying config.
    pub fn config(&self) -> &CssModulesConfig {
        &self.config
    }

    /// Process the given CSS Modules files.
    ///
    /// Each path is expected to point at a real file on disk. Files are
    /// processed in input order; the returned `css` string is the
    /// concatenation of each file's compiled output, separated by blank
    /// lines for readability.
    pub fn process(&self, files: &[PathBuf]) -> Result<CssModulesOutput> {
        let mut combined = String::new();
        let mut class_maps: HashMap<PathBuf, HashMap<String, String>> = HashMap::new();

        for path in files {
            let source = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read CSS Modules file {}", path.display()))?;

            let (css, names) = self.process_one(path, &source)?;
            if !combined.is_empty() {
                combined.push_str("\n\n");
            }
            combined.push_str(&css);
            class_maps.insert(path.clone(), names);
        }

        Ok(CssModulesOutput {
            css: combined,
            class_maps,
        })
    }

    /// Process a single in-memory CSS Modules source. Useful for unit tests
    /// and for callers that already have the source string in memory.
    pub fn process_source(
        &self,
        path: &Path,
        source: &str,
    ) -> Result<(String, HashMap<String, String>)> {
        self.process_one(path, source)
    }

    fn process_one(
        &self,
        path: &Path,
        source: &str,
    ) -> Result<(String, HashMap<String, String>)> {
        let pattern = self
            .config
            .pattern
            .clone()
            .unwrap_or_else(|| "[hash]_[local]".to_string());

        let pattern_parsed = Pattern::parse(&pattern)
            .map_err(|e| anyhow::anyhow!("invalid CSS Modules pattern {pattern:?}: {e}"))?;

        let parser_opts = ParserOptions {
            filename: path.to_string_lossy().into_owned(),
            css_modules: Some(LcssConfig {
                pattern: pattern_parsed,
                dashed_idents: false,
                animation: true,
                custom_idents: true,
                grid: true,
                container: true,
                pure: false,
            }),
            ..ParserOptions::default()
        };

        let stylesheet = StyleSheet::parse(source, parser_opts).map_err(|e| {
            anyhow::anyhow!(
                "lightningcss failed to parse {}: {e}",
                path.display()
            )
        })?;

        let printed = stylesheet
            .to_css(PrinterOptions {
                minify: !self.config.dev,
                ..PrinterOptions::default()
            })
            .map_err(|e| {
                anyhow::anyhow!(
                    "lightningcss failed to print {}: {e}",
                    path.display()
                )
            })?;

        // Translate the lightningcss `exports` map (Option<CssModuleExports>)
        // into a plain HashMap<String, String> of original→scoped names.
        let mut names: HashMap<String, String> = HashMap::new();
        if let Some(exports) = printed.exports {
            for (orig, export) in exports.into_iter() {
                names.insert(orig, export.name);
            }
        }

        Ok((printed.code, names))
    }
}
