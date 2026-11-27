//! `addVirtualModule(..., { watchDirectories })` matching (issue #4894).
//!
//! A loader that enumerates a directory itself declares the directory as
//! `{ path, include?, exclude? }`. This module owns the ONE matching decision
//! both consumers use: the orchestrator's policy gate
//! ([`crate::policy::RawImportInvalidation::is_plugin_watch_target`], which
//! decides whether a batch runs the pre-tick refresh and invalidates pages)
//! and the refresh ownership lookup
//! ([`crate::plugin_refresh::PluginWatchOwnership::affected`], which decides
//! which loaders are re-invoked). Both hold clones of the same
//! [`PluginWatchDirectories`], so they cannot disagree.
//!
//! ## Matching rules (v1 contract)
//!
//! A changed path is matched against each declaration whose root contains it
//! (lexical and canonical spellings of both sides are compared):
//!
//! - the root itself always matches (root delete, recreate or rename);
//! - an existing directory, or a removed path last seen as a directory, is a
//!   structure change: it matches unless the directory's own root-relative
//!   path matches an `exclude` glob. `include` is not applied, because a
//!   directory name rarely matches a file glob;
//! - anything else is a file: it matches when `include` is absent or matches
//!   its root-relative path, and no `exclude` glob matches it.
//!
//! `exclude` only filters its own declaration; another declaration's match
//! still stands. Globs are root-relative, `/`-separated and compiled with
//! `literal_separator(true)` (`*` is one segment, `**` recurses), the same
//! option `bundle.exclude` and `import.meta.glob` use.
//!
//! Two kinds of path never match, so a root that contains them (a whole
//! project root with no `include`, say) cannot refresh itself in a loop:
//!
//! - anything below a directory zfb itself writes into this session (the
//!   dev output, asset and scratch roots the policy registers with
//!   [`PluginWatchDirectories::set_ignored_roots`]);
//! - anything below a `.git` directory.
//!
//! `node_modules` is not ignored: a plugin may declare a root inside it.
//!
//! Directories are remembered once seen (boot walk, a directory event, or
//! the re-arm reconcile walk) so a later removal is still recognised as a
//! structure change after the directory is gone. A directory event
//! re-learns that directory's subtree, so a recreated directory's new
//! children are known too. The set only grows: a stale entry can only
//! cause an extra refresh, never a missed one.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use tracing::warn;
use walkdir::WalkDir;

use crate::plugin_registries::{VirtualLoaderId, VirtualModuleRegistry};

/// Validate and compile one declaration's `include` or `exclude` list. The
/// single compiler both setup validation and the live matcher use, so a
/// pattern accepted at setup always compiles the same way in the matcher.
pub fn compile_watch_directory_globs(patterns: &[String]) -> Result<GlobSet, String> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        validate_pattern(pattern)?;
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map_err(|error| format!("invalid glob {pattern:?}: {error}"))?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|error| format!("failed to compile globs: {error}"))
}

fn validate_pattern(pattern: &str) -> Result<(), String> {
    if pattern.is_empty() {
        return Err("glob patterns must be non-empty strings".to_string());
    }
    if pattern.starts_with('/') || Path::new(pattern).is_absolute() {
        return Err(format!(
            "glob {pattern:?} is absolute; patterns are relative to the directory root"
        ));
    }
    if pattern.contains('\\') {
        return Err(format!(
            "glob {pattern:?} contains a backslash; separate segments with `/`"
        ));
    }
    let bytes = pattern.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(format!(
            "glob {pattern:?} starts with a drive letter; patterns are relative to the directory root"
        ));
    }
    if pattern.split('/').any(|segment| segment == "..") {
        return Err(format!(
            "glob {pattern:?} contains a `..` segment; patterns cannot leave the directory root"
        ));
    }
    if pattern.split('/').any(|segment| segment == ".") {
        return Err(format!(
            "glob {pattern:?} contains a `.` segment, which never matches; remove it"
        ));
    }
    if pattern.split('/').any(str::is_empty) {
        return Err(format!(
            "glob {pattern:?} has an empty segment (`//` or a trailing `/`), which never matches"
        ));
    }
    Ok(())
}

#[derive(Debug)]
struct Declaration {
    specifier: String,
    loader_id: VirtualLoaderId,
    root: PathBuf,
    /// Lexical and (when resolvable at setup) canonical spellings of `root`.
    root_aliases: Vec<PathBuf>,
    include: Option<GlobSet>,
    exclude: GlobSet,
}

#[derive(Debug, Default)]
struct Inner {
    declarations: Vec<Declaration>,
    /// `(declaration index, root-relative path)` of every directory seen.
    known_dirs: RwLock<BTreeSet<(usize, String)>>,
    /// Roots zfb writes into itself; nothing below them matches.
    ignored_roots: RwLock<Vec<PathBuf>>,
}

const GIT_DIR: &str = ".git";

/// Compiled `watchDirectories` declarations of every registered loader.
/// Cheap to clone; clones share the remembered-directory state.
#[derive(Debug, Clone, Default)]
pub struct PluginWatchDirectories {
    inner: Arc<Inner>,
}

impl PluginWatchDirectories {
    /// Compile every registered `watchDirectories` declaration and record
    /// the directories that already exist below each root. Declarations were
    /// validated at setup; one that no longer compiles is skipped with a
    /// warning instead of failing the dev session.
    pub fn from_registry(registry: &VirtualModuleRegistry) -> Self {
        let mut entries: Vec<_> = registry.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        let mut declarations = Vec::new();
        for (specifier, entry) in entries {
            for directory in &entry.watch_directories {
                let include = match directory.include.as_deref() {
                    None => None,
                    Some(patterns) => match compile_watch_directory_globs(patterns) {
                        Ok(set) => Some(set),
                        Err(error) => {
                            warn!(specifier = %specifier, %error, "skipping watchDirectories entry");
                            continue;
                        }
                    },
                };
                let exclude = match compile_watch_directory_globs(&directory.exclude) {
                    Ok(set) => set,
                    Err(error) => {
                        warn!(specifier = %specifier, %error, "skipping watchDirectories entry");
                        continue;
                    }
                };
                let root = zfb_types::normalize_path_lexical(&directory.path);
                let mut root_aliases = vec![root.clone()];
                if let Ok(canonical) = root.canonicalize() {
                    let canonical = zfb_types::normalize_path_lexical(&canonical);
                    if canonical != root {
                        root_aliases.push(canonical);
                    }
                }
                declarations.push(Declaration {
                    specifier: specifier.clone(),
                    loader_id: entry.loader_id.clone(),
                    root,
                    root_aliases,
                    include,
                    exclude,
                });
            }
        }
        let this = Self {
            inner: Arc::new(Inner {
                declarations,
                known_dirs: RwLock::new(BTreeSet::new()),
                ignored_roots: RwLock::new(Vec::new()),
            }),
        };
        for index in 0..this.inner.declarations.len() {
            let root = this.inner.declarations[index].root.clone();
            this.learn_subtree(index, &root);
        }
        this
    }

    /// Replace the roots zfb writes into itself (all spellings). Clones
    /// share the setting.
    pub fn set_ignored_roots(&self, roots: impl IntoIterator<Item = PathBuf>) {
        if let Ok(mut stored) = self.inner.ignored_roots.write() {
            *stored = roots.into_iter().collect();
        }
    }

    fn is_ignored(&self, path: &Path) -> bool {
        let Ok(roots) = self.inner.ignored_roots.read() else {
            return false;
        };
        !roots.is_empty()
            && crate::policy::RawImportInvalidation::path_aliases(path)
                .iter()
                .any(|alias| roots.iter().any(|root| alias.starts_with(root)))
    }

    pub fn is_empty(&self) -> bool {
        self.inner.declarations.is_empty()
    }

    /// Every declared root, for the recursive watch and the root's parent
    /// watch that observes the root's own deletion and recreation.
    pub fn roots(&self) -> BTreeSet<PathBuf> {
        self.inner
            .declarations
            .iter()
            .map(|declaration| declaration.root.clone())
            .collect()
    }

    /// Whether any declaration matches `path`.
    pub fn is_match(&self, path: &Path) -> bool {
        (0..self.inner.declarations.len()).any(|index| self.declaration_matches(index, path))
    }

    /// The `(specifier, loader_id)` owners whose declarations match `path`,
    /// one entry per specifier.
    pub fn owners_of(&self, path: &Path) -> Vec<(String, VirtualLoaderId)> {
        let mut seen = BTreeSet::new();
        let mut owners = Vec::new();
        for (index, declaration) in self.inner.declarations.iter().enumerate() {
            if seen.contains(&declaration.specifier) || !self.declaration_matches(index, path) {
                continue;
            }
            seen.insert(declaration.specifier.clone());
            owners.push((declaration.specifier.clone(), declaration.loader_id.clone()));
        }
        owners
    }

    /// Matching files and directories below roots accepted by `in_scope`
    /// whose mtime is at or after `read_since` — the watch-arm reconcile for
    /// a root whose recursive watch was (re)armed after its loader last read
    /// it. Also remembers every directory it walks.
    pub fn modified_since(
        &self,
        read_since: SystemTime,
        in_scope: impl Fn(&Path) -> bool,
    ) -> Vec<PathBuf> {
        let mut out = BTreeSet::new();
        for (index, declaration) in self.inner.declarations.iter().enumerate() {
            if !in_scope(&declaration.root) {
                continue;
            }
            for entry in WalkDir::new(&declaration.root)
                .follow_links(false)
                .min_depth(1)
                .into_iter()
                .filter_entry(|entry| !self.walk_skips(entry))
                .filter_map(Result::ok)
            {
                let path = entry.path();
                let Some(rel) = relative_key(&declaration.root, path) else {
                    continue;
                };
                if entry.file_type().is_dir() {
                    self.remember(index, rel.clone());
                }
                let modified = entry
                    .metadata()
                    .ok()
                    .and_then(|metadata| metadata.modified().ok());
                if modified.is_some_and(|mtime| mtime >= read_since)
                    && self.declaration_matches(index, path)
                {
                    out.insert(path.to_path_buf());
                }
            }
        }
        out.into_iter().collect()
    }

    fn walk_skips(&self, entry: &walkdir::DirEntry) -> bool {
        entry.file_type().is_dir()
            && (entry.file_name() == GIT_DIR || self.is_ignored(entry.path()))
    }

    fn declaration_matches(&self, index: usize, path: &Path) -> bool {
        let declaration = &self.inner.declarations[index];
        let Some((root, rel)) = declaration_relative(declaration, path) else {
            return false;
        };
        if self.is_ignored(path) {
            return false;
        }
        if rel.is_empty() {
            if root.is_dir() {
                self.learn_subtree(index, &root);
            }
            return true;
        }
        if rel.split('/').any(|segment| segment == GIT_DIR) {
            return false;
        }
        let candidate = root.join(&rel);
        match std::fs::symlink_metadata(&candidate).or_else(|_| std::fs::symlink_metadata(path)) {
            Ok(metadata) if metadata.is_dir() => {
                // Learned even when already known: a recreated directory's
                // new children must be recognised when they are removed.
                self.remember(index, rel.clone());
                self.learn_subtree(index, &candidate);
                !declaration.exclude.is_match(&rel)
            }
            Ok(_) => file_matches(declaration, &rel),
            Err(_) if self.is_known_dir(index, &rel) => !declaration.exclude.is_match(&rel),
            Err(_) => file_matches(declaration, &rel),
        }
    }

    fn remember(&self, index: usize, rel: String) -> bool {
        self.inner
            .known_dirs
            .write()
            .map(|mut dirs| dirs.insert((index, rel)))
            .unwrap_or(false)
    }

    fn is_known_dir(&self, index: usize, rel: &str) -> bool {
        self.inner
            .known_dirs
            .read()
            .map(|dirs| dirs.contains(&(index, rel.to_string())))
            .unwrap_or(true)
    }

    fn learn_subtree(&self, index: usize, dir: &Path) {
        let root = &self.inner.declarations[index].root;
        for entry in WalkDir::new(dir)
            .follow_links(false)
            .min_depth(1)
            .into_iter()
            .filter_entry(|entry| !self.walk_skips(entry))
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_dir())
        {
            let key = relative_key(root, entry.path())
                .or_else(|| {
                    declaration_relative(&self.inner.declarations[index], entry.path())
                        .map(|(_, rel)| rel)
                })
                .filter(|rel| !rel.is_empty());
            if let Some(rel) = key {
                self.remember(index, rel);
            }
        }
    }
}

fn file_matches(declaration: &Declaration, rel: &str) -> bool {
    declaration
        .include
        .as_ref()
        .is_none_or(|include| include.is_match(rel))
        && !declaration.exclude.is_match(rel)
}

/// The root alias containing `path` (in any of its spellings) and `path`'s
/// `/`-joined position below it.
fn declaration_relative(declaration: &Declaration, path: &Path) -> Option<(PathBuf, String)> {
    let candidates = crate::policy::RawImportInvalidation::path_aliases(path);
    for root in &declaration.root_aliases {
        for candidate in &candidates {
            if let Some(rel) = relative_key(root, candidate) {
                return Some((root.clone(), rel));
            }
        }
    }
    None
}

fn relative_key(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let mut segments = Vec::new();
    for component in rel.components() {
        match component {
            Component::Normal(segment) => segments.push(segment.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(segments.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_registries::{VirtualModuleEntry, WatchDirectory};

    fn directory(path: &Path, include: Option<&[&str]>, exclude: &[&str]) -> WatchDirectory {
        WatchDirectory {
            path: path.to_path_buf(),
            include: include.map(|patterns| patterns.iter().map(|p| p.to_string()).collect()),
            exclude: exclude.iter().map(|p| p.to_string()).collect(),
        }
    }

    fn matcher(entries: Vec<(&str, Vec<WatchDirectory>)>) -> PluginWatchDirectories {
        let mut registry = VirtualModuleRegistry::new();
        for (specifier, watch_directories) in entries {
            registry.insert(VirtualModuleEntry {
                specifier: specifier.to_string(),
                plugin: "test-plugin".to_string(),
                loader_id: VirtualLoaderId(format!("loader:{specifier}")),
                watch_files: Vec::new(),
                watch_directories,
            });
        }
        PluginWatchDirectories::from_registry(&registry)
    }

    fn write(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "x").unwrap();
    }

    fn specifiers(owners: Vec<(String, VirtualLoaderId)>) -> Vec<String> {
        owners.into_iter().map(|(specifier, _)| specifier).collect()
    }

    #[test]
    fn glob_validation_rejects_absolute_parent_and_empty_patterns() {
        for bad in ["/abs/*.md", "../up/*.md", "a/../b.md", ".."] {
            assert!(
                compile_watch_directory_globs(&[bad.to_string()]).is_err(),
                "{bad:?} must be rejected"
            );
        }
        assert!(compile_watch_directory_globs(&[String::new()]).is_err());
        assert!(compile_watch_directory_globs(&["a/[".to_string()]).is_err());
        assert!(
            compile_watch_directory_globs(&["**/*.md".to_string(), "a..b/*".to_string()]).is_ok()
        );
    }

    #[test]
    fn glob_validation_rejects_patterns_that_can_never_match() {
        for (bad, reason) in [
            ("C:/posts/*.md", "drive letter"),
            ("c:*.md", "drive letter"),
            ("posts\\*.md", "backslash"),
            ("./a.md", "`.` segment"),
            ("a/./b.md", "`.` segment"),
            ("a//b.md", "empty segment"),
            ("drafts/", "empty segment"),
        ] {
            let error = compile_watch_directory_globs(&[bad.to_string()])
                .expect_err(&format!("{bad:?} must be rejected"));
            assert!(error.contains(reason), "{bad:?}: {error}");
        }
        assert!(compile_watch_directory_globs(&[".hidden/*.md".to_string()]).is_ok());
    }

    #[test]
    fn zfb_written_roots_and_git_dirs_never_match() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        std::fs::create_dir_all(root.join(".git/objects")).unwrap();
        std::fs::create_dir_all(root.join("dist/assets")).unwrap();
        write(&root.join("src/a.md"));
        write(&root.join("node_modules/pkg/data.json"));
        let m = matcher(vec![("virtual:all", vec![directory(&root, None, &[])])]);
        let clone = m.clone();
        clone.set_ignored_roots([root.join("dist"), root.join(".zfb-build")]);
        for ignored in [
            root.join("dist/index.html"),
            root.join("dist/assets"),
            root.join("dist"),
            root.join(".zfb-build/dev/a.js"),
            root.join(".git/index"),
            root.join(".git/objects"),
            root.join("sub/.git/HEAD"),
        ] {
            assert!(!m.is_match(&ignored), "{ignored:?}");
        }
        assert!(m.is_match(&root.join("src/a.md")));
        assert!(m.is_match(&root.join("node_modules/pkg/data.json")));
        assert!(m.is_match(&root.join("distribution/a.md")));
        assert!(m.is_match(&root), "the root itself still matches");

        // The reconcile walk skips them too.
        let since = SystemTime::UNIX_EPOCH;
        let found = m.modified_since(since, |_| true);
        assert!(
            found
                .iter()
                .all(|p| !p.starts_with(root.join("dist")) && !p.starts_with(root.join(".git"))),
            "{found:?}"
        );
        assert!(found.contains(&root.join("src/a.md")));
    }

    #[test]
    fn recreated_known_directory_relearns_its_new_subtree() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("posts");
        write(&root.join("sub/a.md"));
        let m = matcher(vec![(
            "virtual:posts",
            vec![directory(&root, Some(&["**/*.md"]), &[])],
        )]);
        std::fs::remove_dir_all(root.join("sub")).unwrap();
        assert!(m.is_match(&root.join("sub")), "removed known dir");
        write(&root.join("sub/fresh/b.md"));
        assert!(m.is_match(&root.join("sub")), "recreated known dir");
        std::fs::remove_dir_all(root.join("sub")).unwrap();
        assert!(
            m.is_match(&root.join("sub/fresh")),
            "the recreated dir's new child is remembered after removal"
        );

        // Same through a recreated root.
        std::fs::remove_dir_all(&root).unwrap();
        write(&root.join("again/c.md"));
        assert!(m.is_match(&root));
        std::fs::remove_dir_all(root.join("again")).unwrap();
        assert!(m.is_match(&root.join("again")));
    }

    #[test]
    fn omitted_include_matches_every_descendant_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("posts");
        std::fs::create_dir_all(&root).unwrap();
        let m = matcher(vec![("virtual:posts", vec![directory(&root, None, &[])])]);
        assert!(m.is_match(&root.join("a.md")));
        assert!(m.is_match(&root.join("deep/nested/b.json")));
        assert!(!m.is_match(&tmp.path().join("posts-other/a.md")));
        assert!(!m.is_match(&tmp.path().join("a.md")));
    }

    #[test]
    fn include_uses_literal_separator_semantics() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("posts");
        std::fs::create_dir_all(&root).unwrap();
        let m = matcher(vec![(
            "virtual:posts",
            vec![directory(&root, Some(&["*.md", "data/**/*.json"]), &[])],
        )]);
        assert!(m.is_match(&root.join("a.md")));
        assert!(!m.is_match(&root.join("sub/a.md")), "`*` is one segment");
        assert!(m.is_match(&root.join("data/x.json")));
        assert!(m.is_match(&root.join("data/a/b/x.json")), "`**` recurses");
        assert!(!m.is_match(&root.join("notes.txt")));
    }

    #[test]
    fn exclude_wins_only_within_its_own_declaration() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("posts");
        std::fs::create_dir_all(&root).unwrap();
        let m = matcher(vec![
            (
                "virtual:published",
                vec![directory(&root, Some(&["**/*.md"]), &["drafts/**"])],
            ),
            (
                "virtual:all",
                vec![directory(&root, Some(&["**/*.md"]), &[])],
            ),
        ]);
        assert_eq!(
            specifiers(m.owners_of(&root.join("drafts/a.md"))),
            vec!["virtual:all".to_string()],
            "the exclude cancels only its own declaration"
        );
        assert_eq!(
            specifiers(m.owners_of(&root.join("a.md"))),
            vec!["virtual:all".to_string(), "virtual:published".to_string()]
        );
    }

    #[test]
    fn duplicate_declarations_and_one_loader_with_many_roots_dedupe() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let m = matcher(vec![(
            "virtual:x",
            vec![
                directory(&a, None, &[]),
                directory(&a, Some(&["*.md"]), &[]),
                directory(&b, None, &[]),
            ],
        )]);
        assert_eq!(specifiers(m.owners_of(&a.join("x.md"))), vec!["virtual:x"]);
        assert_eq!(specifiers(m.owners_of(&b.join("y.txt"))), vec!["virtual:x"]);
        assert_eq!(m.roots(), BTreeSet::from([a, b]));
    }

    #[test]
    fn directory_structure_events_match_even_when_no_file_glob_does() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("posts");
        let existing = root.join("existing");
        std::fs::create_dir_all(&existing).unwrap();
        let m = matcher(vec![(
            "virtual:posts",
            vec![directory(&root, Some(&["**/*.md"]), &["drafts/**"])],
        )]);

        assert!(m.is_match(&root), "the root itself always matches");
        let created = root.join("new-dir");
        std::fs::create_dir_all(created.join("child")).unwrap();
        assert!(m.is_match(&created), "a created directory matches");

        // Removed after being seen at boot or at creation: still a structure
        // change, and stays so for repeated queries in the same tick.
        std::fs::remove_dir_all(&existing).unwrap();
        assert!(m.is_match(&existing));
        assert!(m.is_match(&existing));
        std::fs::remove_dir_all(&created).unwrap();
        assert!(m.is_match(&created));
        assert!(
            m.is_match(&created.join("child")),
            "a subdirectory learned with its parent is recognised after removal"
        );

        // A removed path never seen as a directory is a file.
        assert!(!m.is_match(&root.join("gone.txt")));
        assert!(m.is_match(&root.join("gone.md")));

        // Editing a non-matching file never matches.
        write(&root.join("notes.txt"));
        assert!(!m.is_match(&root.join("notes.txt")));

        // Directories below an exclude are skipped.
        std::fs::create_dir_all(root.join("drafts/sub")).unwrap();
        assert!(!m.is_match(&root.join("drafts/sub")));

        // Root removal and recreation.
        std::fs::remove_dir_all(&root).unwrap();
        assert!(m.is_match(&root));
        std::fs::create_dir_all(&root).unwrap();
        assert!(m.is_match(&root));
    }

    #[cfg(unix)]
    #[test]
    fn canonical_and_lexical_spellings_match_the_same_declaration() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let m = matcher(vec![(
            "virtual:x",
            vec![directory(&link, Some(&["*.md"]), &[])],
        )]);
        assert!(m.is_match(&real.join("a.md")));
        assert!(m.is_match(&link.join("a.md")));
        assert!(!m.is_match(&real.join("a.txt")));
    }

    #[test]
    fn modified_since_reports_matching_entries_and_respects_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("posts");
        std::fs::create_dir_all(&root).unwrap();
        let m = matcher(vec![(
            "virtual:posts",
            vec![directory(&root, Some(&["**/*.md"]), &[])],
        )]);
        let before = SystemTime::now() - std::time::Duration::from_secs(60);
        write(&root.join("sub/a.md"));
        write(&root.join("sub/b.txt"));
        let found = m.modified_since(before, |_| true);
        assert!(found.contains(&root.join("sub/a.md")));
        assert!(
            found.contains(&root.join("sub")),
            "a new directory is a structure change"
        );
        assert!(!found.contains(&root.join("sub/b.txt")));
        assert!(m.modified_since(before, |_| false).is_empty());
        let future = SystemTime::now() + std::time::Duration::from_secs(3600);
        assert!(m.modified_since(future, |_| true).is_empty());
    }
}
