//! Deterministic, gitignore-aware expansion of declared source roots.
use crate::{ExclusionMatcher, PositiveRoot, SourceId, SourcePlan};
use ignore::WalkBuilder;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SKIP: &[&str] = &[
    "node_modules",
    "dist",
    "build",
    "target",
    ".git",
    ".cache",
    ".turbo",
    ".next",
    ".vercel",
    ".zfb-build",
    ".zfb",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpandedFile {
    pub id: SourceId,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WalkDiagnostic {
    pub root_label: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FileSet {
    pub files: Vec<ExpandedFile>,
    pub diagnostics: Vec<WalkDiagnostic>,
}

/// Skipped directory names a declared package root still walks.
pub const PACKAGE_ROOT_TRAVERSES: &[&str] = &["node_modules", "dist"];

/// All source discovery paths apply this file rule before extraction.
pub fn is_candidate_source(path: &Path, accepts_extension: impl FnOnce(&str) -> bool) -> bool {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(".d.ts"))
    {
        return false;
    }
    path.extension()
        .and_then(|part| part.to_str())
        .is_some_and(accepts_extension)
}

fn root_priority(root: &PositiveRoot) -> u8 {
    if root.label.starts_with("package-root/") {
        0
    } else if root.label.starts_with("root/") {
        1
    } else {
        2
    }
}

fn compare_roots(a: &PositiveRoot, b: &PositiveRoot) -> std::cmp::Ordering {
    root_priority(a)
        .cmp(&root_priority(b))
        .then_with(|| {
            b.resolved_path()
                .components()
                .count()
                .cmp(&a.resolved_path().components().count())
        })
        .then(a.label.cmp(&b.label))
        .then(a.path.cmp(&b.path))
}

fn excluded(path: &Path, exclusions: &BTreeSet<PathBuf>) -> bool {
    exclusions.iter().any(|excluded| path.starts_with(excluded))
}

fn compile_author_exclusions(plan: &SourcePlan, out: &mut FileSet) -> Arc<Vec<ExclusionMatcher>> {
    let mut compiled = Vec::new();
    for exclusion in &plan.author_exclusions {
        match exclusion.compile() {
            Ok(matcher) => compiled.push(matcher),
            Err(message) => out.diagnostics.push(WalkDiagnostic {
                root_label: exclusion.origin.clone(),
                message: format!(
                    "invalid source exclusion {:?}: {message}",
                    exclusion.pattern
                ),
            }),
        }
    }
    Arc::new(compiled)
}

fn author_excluded(path: &Path, exclusions: &[ExclusionMatcher]) -> bool {
    if exclusions.is_empty() {
        return false;
    }
    let canonical = fs::canonicalize(path).ok();
    exclusions
        .iter()
        .any(|exclusion| exclusion.matches_identity(path, canonical.as_deref()))
}

fn visit_root(
    root: &PositiveRoot,
    plan: &SourcePlan,
    out: &mut FileSet,
    seen: &mut BTreeSet<PathBuf>,
    exclusions: &BTreeSet<PathBuf>,
    author_exclusions: &Arc<Vec<ExclusionMatcher>>,
    changed: Option<&Path>,
) {
    let declared = root.resolved_path();
    let Ok(resolved) = fs::canonicalize(&declared) else {
        if root.required {
            out.diagnostics.push(WalkDiagnostic {
                root_label: root.label.clone(),
                message: format!(
                    "required source missing or unreadable: {}",
                    declared.display()
                ),
            });
        }
        return;
    };
    if excluded(&resolved, exclusions) || author_excluded(&resolved, author_exclusions) {
        return;
    }
    let is_file = resolved.is_file();
    let changed = if let Some(changed) = changed {
        let Ok(changed) = fs::canonicalize(changed) else {
            return;
        };
        if is_file {
            if changed != resolved {
                return;
            }
            Some(changed)
        } else if changed.starts_with(&resolved) {
            Some(changed)
        } else if resolved.starts_with(&changed) {
            Some(resolved.clone())
        } else {
            return;
        }
    } else {
        None
    };
    // Start at the declared root so ancestor .gitignore rules have exactly
    // the same meaning as a full expansion; prune unrelated branches before
    // descending, rather than walking the entire project.
    let mut builder = WalkBuilder::new(&resolved);
    builder.require_git(false).follow_links(false).hidden(true);
    if root.package_root {
        // Projects and workspace packages commonly gitignore `node_modules`
        // and `dist`, which are exactly what a package root must reach.
        builder
            .parents(false)
            .ignore(false)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false);
    }
    let root_path = resolved.clone();
    let filter_exclusions = exclusions.clone();
    let filter_author_exclusions = Arc::clone(author_exclusions);
    let package_root = root.package_root;
    builder.filter_entry(move |entry| {
        if entry.depth() == 0 {
            return true;
        }
        let path = entry.path();
        let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
        (changed
            .as_ref()
            .is_none_or(|target| path.starts_with(target) || target.starts_with(path)))
            && !(excluded(path, &filter_exclusions)
                || is_dir && author_excluded(path, &filter_author_exclusions)
                || is_dir
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| {
                            SKIP.contains(&name)
                                && !(package_root && PACKAGE_ROOT_TRAVERSES.contains(&name))
                        }))
    });
    for entry in builder.build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                out.diagnostics.push(WalkDiagnostic {
                    root_label: root.label.clone(),
                    message: error.to_string(),
                });
                continue;
            }
        };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        if excluded(path, exclusions) || author_excluded(path, author_exclusions) {
            continue;
        }
        if !is_candidate_source(path, |ext| plan.extensions.contains(ext)) {
            continue;
        }
        let relative = if is_file {
            Path::new(path.file_name().unwrap_or_default())
        } else {
            path.strip_prefix(&root_path)
                .expect("walk entry belongs to root")
        };
        let id = match SourceId::new(&root.label, relative) {
            Ok(id) => id,
            Err(message) => {
                out.diagnostics.push(WalkDiagnostic {
                    root_label: root.label.clone(),
                    message,
                });
                continue;
            }
        };
        let Ok(canonical) = fs::canonicalize(path) else {
            continue;
        };
        if seen.insert(canonical.clone()) {
            out.files.push(ExpandedFile {
                id,
                path: canonical,
            });
        }
    }
}

/// Expand declared roots; explicit roots remain eligible below an ignored parent.
pub fn expand_file_set(plan: &SourcePlan) -> FileSet {
    let exclusions: BTreeSet<_> = plan
        .exclusions
        .iter()
        .map(|path| fs::canonicalize(path).unwrap_or_else(|_| path.clone()))
        .collect();
    let mut out = FileSet::default();
    let author_exclusions = compile_author_exclusions(plan, &mut out);
    let mut seen = BTreeSet::new();
    let mut roots: Vec<_> = plan
        .roots
        .iter()
        .chain(plan.package_sources.values())
        .collect();
    roots.sort_by(|a, b| compare_roots(a, b));
    for root in roots {
        let root_exclusions = exclusions
            .iter()
            .cloned()
            .chain(root.exclusions.iter().map(|path| {
                let declared = root.declaring_dir.join(path);
                fs::canonicalize(&declared).unwrap_or(declared)
            }))
            .collect();
        visit_root(
            root,
            plan,
            &mut out,
            &mut seen,
            &root_exclusions,
            &author_exclusions,
            None,
        );
    }
    out.files.sort_by(|a, b| a.id.cmp(&b.id));
    out.diagnostics.sort_by(|a, b| {
        a.root_label
            .cmp(&b.root_label)
            .then(a.message.cmp(&b.message))
    });
    out
}

/// Expand a changed file or directory using the same root, ignore, and
/// exclusion rules as a complete walk. A created directory may be the only
/// watcher event for all of its children.
pub fn expand_changed_path(plan: &SourcePlan, changed: &Path) -> FileSet {
    let exclusions: BTreeSet<_> = plan
        .exclusions
        .iter()
        .map(|path| fs::canonicalize(path).unwrap_or_else(|_| path.clone()))
        .collect();
    let mut out = FileSet::default();
    let author_exclusions = compile_author_exclusions(plan, &mut out);
    let mut seen = BTreeSet::new();
    let mut roots: Vec<_> = plan
        .roots
        .iter()
        .chain(plan.package_sources.values())
        .collect();
    roots.sort_by(|a, b| compare_roots(a, b));
    for root in roots {
        let declared = root.resolved_path();
        if !declared.exists() && !changed.starts_with(&declared) && !declared.starts_with(changed) {
            continue;
        }
        let root_exclusions = exclusions
            .iter()
            .cloned()
            .chain(root.exclusions.iter().map(|path| {
                let declared = root.declaring_dir.join(path);
                fs::canonicalize(&declared).unwrap_or(declared)
            }))
            .collect();
        visit_root(
            root,
            plan,
            &mut out,
            &mut seen,
            &root_exclusions,
            &author_exclusions,
            Some(changed),
        );
    }
    out.files.sort_by(|a, b| a.id.cmp(&b.id));
    out
}
