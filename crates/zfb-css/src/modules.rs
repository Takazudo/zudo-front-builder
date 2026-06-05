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

    /// Project root used to derive the *project-relative* filename that
    /// lightningcss hashes into the scoped class prefix (`[hash]`).
    ///
    /// The default `[hash]_[local]` pattern derives `[hash]` from the
    /// filename we hand to lightningcss. Passing the absolute module path
    /// there makes byte-identical sources produce different scoped class
    /// names — and a different `styles-<hash>.css` filename — across
    /// machines/checkout paths (see issue #825). Passing the path
    /// relative to this root (with `/` separators) keeps `[hash]` stable
    /// across checkouts while staying unique across same-basename modules
    /// in different directories.
    ///
    /// When `None`, or when a module path is not under this root, we fall
    /// back to the absolute path — the non-reproducible-but-correct
    /// behaviour from before #825.
    ///
    /// Note: this only affects the *string fed to lightningcss for
    /// hashing*. The returned [`CssModulesOutput::class_maps`] stays keyed
    /// by the original absolute path so downstream bundler lookups are
    /// unaffected.
    pub project_root: Option<PathBuf>,
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

    fn process_one(&self, path: &Path, source: &str) -> Result<(String, HashMap<String, String>)> {
        let pattern = self
            .config
            .pattern
            .clone()
            .unwrap_or_else(|| "[hash]_[local]".to_string());

        let pattern_parsed = Pattern::parse(&pattern)
            .map_err(|e| anyhow::anyhow!("invalid CSS Modules pattern {pattern:?}: {e}"))?;

        let parser_opts = ParserOptions {
            filename: hash_filename(path, self.config.project_root.as_deref()),
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

        let stylesheet = StyleSheet::parse(source, parser_opts)
            .map_err(|e| anyhow::anyhow!("lightningcss failed to parse {}: {e}", path.display()))?;

        let printed = stylesheet
            .to_css(PrinterOptions {
                minify: !self.config.dev,
                ..PrinterOptions::default()
            })
            .map_err(|e| anyhow::anyhow!("lightningcss failed to print {}: {e}", path.display()))?;

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

/// Derive the filename string fed to a *user-visible hash* for a CSS
/// Modules file.
///
/// When `project_root` is `Some` and `path` is under it, returns the
/// project-relative path with `/` separators (so the hash is stable
/// across machines/checkout paths — see issue #825, and matches across
/// OSes by normalising Windows `\` to `/`). Otherwise falls back to the
/// absolute (lossy) path: stable within a build, just not across
/// relocations.
///
/// Used for the lightningcss `[hash]` prefix in [`CssModulesProcessor`]
/// and for the class-map JSON filename hash in `pipeline.rs`, so both
/// user-visible hashes derive from the same normalised string.
pub(crate) fn hash_filename(path: &Path, project_root: Option<&Path>) -> String {
    let rel = project_root.and_then(|root| path.strip_prefix(root).ok());
    let chosen = rel.unwrap_or(path);
    let lossy = chosen.to_string_lossy();
    // Normalise Windows separators so the hash matches across OSes.
    if lossy.contains('\\') {
        lossy.replace('\\', "/")
    } else {
        lossy.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = ".section { color: red; }";

    fn processor_with_root(root: &str) -> CssModulesProcessor {
        CssModulesProcessor::new(CssModulesConfig {
            project_root: Some(PathBuf::from(root)),
            ..CssModulesConfig::default()
        })
    }

    /// Synthetic absolute paths only — no tempdirs. A disk-backed
    /// tempdir on macOS resolves `/var` → `/private/var`, which would
    /// make `strip_prefix(project_root)` flaky; the production inputs
    /// are lexical (see `scanner::resolve_specifier`), so synthetic
    /// paths model them faithfully.
    fn scoped_name(processor: &CssModulesProcessor, path: &str) -> String {
        let (_, names) = processor
            .process_source(Path::new(path), SOURCE)
            .expect("process_source");
        names
            .get("section")
            .cloned()
            .expect("scoped name for `section`")
    }

    /// #825: byte-identical sources at the *same project-relative path*
    /// under two different absolute roots must produce identical scoped
    /// class names, so builds are reproducible across machines/checkout
    /// paths.
    #[test]
    fn scoped_names_are_stable_across_project_roots() {
        let a = scoped_name(
            &processor_with_root("/home/runner/work/proj"),
            "/home/runner/work/proj/src/card.module.css",
        );
        let b = scoped_name(
            &processor_with_root("/Users/dev/repos/proj"),
            "/Users/dev/repos/proj/src/card.module.css",
        );
        assert_eq!(
            a, b,
            "same relative path under different roots must hash identically"
        );
    }

    /// Uniqueness is preserved: two same-basename modules in different
    /// subdirectories of the *same* root must still get different scoped
    /// names (the relative path, not just the basename, feeds the hash).
    #[test]
    fn scoped_names_differ_for_same_basename_in_different_dirs() {
        let processor = processor_with_root("/proj");
        let a = scoped_name(&processor, "/proj/src/a/card.module.css");
        let b = scoped_name(&processor, "/proj/src/b/card.module.css");
        assert_ne!(
            a, b,
            "same basename in different dirs must keep distinct scoped names"
        );
    }

    /// Without a `project_root`, the absolute path feeds the hash (the
    /// pre-#825 behaviour): two different absolute paths yield different
    /// names — correct within a build, just not reproducible across
    /// relocations.
    #[test]
    fn scoped_names_fall_back_to_absolute_path_without_root() {
        let processor = CssModulesProcessor::with_default_config();
        let a = scoped_name(&processor, "/root-a/src/card.module.css");
        let b = scoped_name(&processor, "/root-b/src/card.module.css");
        assert_ne!(
            a, b,
            "absolute-path fallback keeps per-build uniqueness when no root is set"
        );
    }

    #[test]
    fn hash_filename_relativises_under_root() {
        assert_eq!(
            hash_filename(
                Path::new("/proj/src/card.module.css"),
                Some(Path::new("/proj"))
            ),
            "src/card.module.css"
        );
    }

    #[test]
    fn hash_filename_falls_back_to_absolute_outside_root() {
        // Path not under the root → lossy absolute fallback.
        assert_eq!(
            hash_filename(
                Path::new("/elsewhere/card.module.css"),
                Some(Path::new("/proj"))
            ),
            "/elsewhere/card.module.css"
        );
    }

    #[test]
    fn hash_filename_normalises_backslashes() {
        // Even on a non-Windows host, a backslash in the relative tail
        // must normalise so the hash matches a Unix checkout.
        assert_eq!(
            hash_filename(Path::new(r"sub\card.module.css"), None),
            "sub/card.module.css"
        );
    }
}
