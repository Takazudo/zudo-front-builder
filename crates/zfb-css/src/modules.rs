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

    /// Optional workspace-wide root used as a *second* relativization base
    /// for module paths that fall outside `project_root` — e.g. a
    /// `.module.css` file in a workspace sibling package staged into the
    /// SSR shadow (issue #1691/#1694).
    ///
    /// When `Some`, [`hash_filename`]'s absolute-path fallback is tried
    /// last: a path outside `project_root` but under `first_party_root`
    /// hashes off its `first_party_root`-relative path instead of the
    /// absolute path, keeping sibling scoped class names (and the
    /// class-map JSON filename) reproducible across checkout locations
    /// too. This never changes behaviour for paths already under
    /// `project_root`, and `None` reproduces the pre-#1694 fallback
    /// exactly — see [`hash_filename`]'s doc comment for the full
    /// resolution order.
    pub first_party_root: Option<PathBuf>,
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

impl CssModulesConfig {
    /// Production config that hashes scoped class names off the
    /// *project-relative* module path (issue #825).
    ///
    /// This is the single source of truth for the reproducible-hash config
    /// used by the build's CSS pipeline: the emitted `styles-<hash>.css` and
    /// the build-time JSX class-map rewrite MUST construct their
    /// `CssModulesProcessor` with the same `project_root` so the scoped names
    /// (and therefore the asset hash) agree. Using this constructor at every
    /// such site makes the invariant structural instead of comment-enforced.
    pub fn for_project_root(root: &Path) -> Self {
        Self {
            project_root: Some(root.to_path_buf()),
            ..Self::default()
        }
    }

    /// Workspace-aware variant of [`Self::for_project_root`] (issue #1694).
    ///
    /// Use this instead of `for_project_root` when the build has a
    /// workspace: a module path outside `project_root` but under
    /// `first_party_root` (a sibling package staged into the SSR shadow's
    /// work mirror) hashes off its `first_party_root`-relative path rather
    /// than falling back to the absolute path, so sibling `.module.css`
    /// scoped class names stay reproducible across checkout locations too.
    /// Behaviour for paths under `project_root` is unchanged.
    ///
    /// Callers without a workspace should keep using `for_project_root` —
    /// passing the same value for both roots also works (it degenerates to
    /// `for_project_root`'s behaviour) but is needlessly indirect.
    pub fn for_project_and_first_party_roots(project_root: &Path, first_party_root: &Path) -> Self {
        Self {
            project_root: Some(project_root.to_path_buf()),
            first_party_root: Some(first_party_root.to_path_buf()),
            ..Self::default()
        }
    }
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
            filename: hash_filename_with_workspace(
                path,
                self.config.project_root.as_deref(),
                self.config.first_party_root.as_deref(),
            ),
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
///
/// Delegates to [`hash_filename_with_workspace`] with `first_party_root:
/// None`, so its behaviour (and every existing call site) is unchanged.
pub(crate) fn hash_filename(path: &Path, project_root: Option<&Path>) -> String {
    hash_filename_with_workspace(path, project_root, None)
}

/// Workspace-aware variant of [`hash_filename`] (issue #1694).
///
/// Resolution order: `project_root`-relative first (identical to
/// `hash_filename`); when `path` is not under `project_root`,
/// `first_party_root`-relative next (a workspace sibling package staged
/// into the SSR shadow's work mirror — issue #1691); otherwise the
/// absolute-path fallback. Passing `first_party_root: None` reproduces
/// `hash_filename` exactly, byte for byte.
///
/// The `first_party_root`-relative branch is tagged with a
/// `workspace-sibling:` prefix. Without it, a project module and a sibling
/// module that happen to share the same root-relative suffix (e.g. project
/// `lib/card.module.css` vs. a sibling package's `lib/card.module.css`)
/// would hash to the identical string *within the same build*, giving two
/// distinct files the same scoped `[hash]` prefix — caught in review for
/// #1694.
pub(crate) fn hash_filename_with_workspace(
    path: &Path,
    project_root: Option<&Path>,
    first_party_root: Option<&Path>,
) -> String {
    if let Some(root) = project_root {
        if let Ok(rel) = path.strip_prefix(root) {
            return normalize_hash_path(rel);
        }
    }
    if let Some(root) = first_party_root {
        if let Ok(rel) = path.strip_prefix(root) {
            return format!("workspace-sibling:{}", normalize_hash_path(rel));
        }
    }
    normalize_hash_path(path)
}

// Normalise `\` → `/` UNCONDITIONALLY (not via the OS-path-gated
// `zfb_types::path_to_posix_string`, which only replaces on Windows to
// avoid corrupting literal `\` in Unix filenames). The module hash must be
// identical regardless of the host OS that produced the path — a Windows
// checkout's `sub\a.css` must hash the same as a Unix checkout's
// `sub/a.css` (#825).
fn normalize_hash_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
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

    fn processor_with_workspace_roots(
        project_root: &str,
        first_party_root: &str,
    ) -> CssModulesProcessor {
        CssModulesProcessor::new(CssModulesConfig::for_project_and_first_party_roots(
            Path::new(project_root),
            Path::new(first_party_root),
        ))
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

    /// #1694: a sibling module (outside `project_root`, inside
    /// `first_party_root`) hashes off its `first_party_root`-relative
    /// path, so byte-identical siblings at the same workspace-relative
    /// path are checkout-location independent — same guarantee #825 gave
    /// project files, extended to workspace siblings.
    #[test]
    fn scoped_names_for_sibling_modules_are_stable_across_workspace_roots() {
        let a = scoped_name(
            &processor_with_workspace_roots("/home/runner/work/ws/proj", "/home/runner/work/ws"),
            "/home/runner/work/ws/lib/shared/card.module.css",
        );
        let b = scoped_name(
            &processor_with_workspace_roots("/Users/dev/repos/ws/proj", "/Users/dev/repos/ws"),
            "/Users/dev/repos/ws/lib/shared/card.module.css",
        );
        assert_eq!(
            a, b,
            "same workspace-relative sibling path under different workspace roots must hash identically"
        );
    }

    #[test]
    fn hash_filename_with_workspace_relativises_under_first_party_root_outside_project_root() {
        assert_eq!(
            hash_filename_with_workspace(
                Path::new("/ws/lib/shared/card.module.css"),
                Some(Path::new("/ws/proj")),
                Some(Path::new("/ws")),
            ),
            "workspace-sibling:lib/shared/card.module.css"
        );
    }

    /// A project module and a workspace-sibling module that happen to share
    /// the same root-relative suffix must NOT collide onto the same hash
    /// input within a single build — they are two distinct files and must
    /// keep distinct scoped names (caught in review for #1694).
    #[test]
    fn hash_filename_with_workspace_does_not_collide_project_and_sibling_same_suffix() {
        let project_side = hash_filename_with_workspace(
            Path::new("/ws/apps/site/lib/card.module.css"),
            Some(Path::new("/ws/apps/site")),
            Some(Path::new("/ws")),
        );
        let sibling_side = hash_filename_with_workspace(
            Path::new("/ws/lib/card.module.css"),
            Some(Path::new("/ws/apps/site")),
            Some(Path::new("/ws")),
        );
        assert_eq!(project_side, "lib/card.module.css");
        assert_eq!(sibling_side, "workspace-sibling:lib/card.module.css");
        assert_ne!(
            project_side, sibling_side,
            "project-relative and workspace-sibling-relative paths sharing the same suffix must not collide"
        );
    }

    #[test]
    fn hash_filename_with_workspace_prefers_project_root_when_path_is_under_both() {
        // project_root is nested under first_party_root here (the common
        // shape); a path under project_root must still relativize against
        // project_root, not the wider first_party_root.
        assert_eq!(
            hash_filename_with_workspace(
                Path::new("/ws/proj/src/card.module.css"),
                Some(Path::new("/ws/proj")),
                Some(Path::new("/ws")),
            ),
            "src/card.module.css"
        );
    }

    #[test]
    fn hash_filename_with_workspace_falls_back_to_absolute_outside_both_roots() {
        assert_eq!(
            hash_filename_with_workspace(
                Path::new("/elsewhere/card.module.css"),
                Some(Path::new("/ws/proj")),
                Some(Path::new("/ws")),
            ),
            "/elsewhere/card.module.css"
        );
    }

    /// `first_party_root: None` must reproduce `hash_filename` byte for
    /// byte — the delegation in `hash_filename` is exercised directly, not
    /// just asserted by reading the source.
    #[test]
    fn hash_filename_with_workspace_none_matches_hash_filename() {
        let path = Path::new("/elsewhere/card.module.css");
        let root = Some(Path::new("/proj"));
        assert_eq!(
            hash_filename_with_workspace(path, root, None),
            hash_filename(path, root)
        );
    }
}
