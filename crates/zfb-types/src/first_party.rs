//! Runtime detection of the first-party source boundary (issue #1664).
//!
//! In a pnpm workspace, a sub-package host legitimately imports sibling
//! workspace source through tsconfig path aliases (`@/*` -> `../../*`).
//! The module-worker / island preprocessing contract must therefore treat
//! the **workspace** — not the single package directory — as the first-party
//! boundary. The workspace root is the nearest ancestor directory (including
//! `project_root` itself) whose `pnpm-workspace.yaml` actually **claims**
//! the project as a member; without one, the boundary stays `project_root`.

use std::path::{Component, Path, PathBuf};

use crate::normalize_path_lexical;

/// Widest directory whose files count as first-party sources for
/// `project_root`.
///
/// Returns the nearest ancestor of `project_root` (including `project_root`
/// itself) that contains `pnpm-workspace.yaml` **and whose `packages:` globs
/// claim `project_root` as a workspace member**, or `project_root` unchanged
/// otherwise. Membership matters: a fixture or scratch project that merely
/// sits inside some repository which happens to be a pnpm workspace (e.g. a
/// test-fixture site inside this very repo) must not inherit that
/// workspace's boundary. The ascent stops at the nearest workspace marker or
/// VCS root (a directory containing `.git`), so a stray marker above the
/// repository can never widen the boundary. The returned path preserves the
/// lexical spelling derived from `project_root` (no canonicalization), so
/// callers can keep comparing logical paths the way they already do with
/// `project_root`.
pub fn first_party_root_for(project_root: &Path) -> PathBuf {
    let root = normalize_path_lexical(project_root);
    let mut dir = root.as_path();
    loop {
        let marker = dir.join("pnpm-workspace.yaml");
        if marker.is_file() {
            // The nearest marker defines the workspace boundary. If it does
            // not claim the project, the project is not in a workspace —
            // do NOT keep ascending to an outer marker.
            let claims = std::fs::read_to_string(&marker)
                .ok()
                .map(|yaml| {
                    let globs = workspace_package_globs(&yaml);
                    workspace_claims_member(&globs, &root, dir)
                })
                .unwrap_or(false);
            if claims {
                return dir.to_path_buf();
            }
            return root;
        }
        // `.git` may be a dir (checkout) or a file (worktree/submodule);
        // either marks the repository boundary. The VCS root itself was
        // already checked for the workspace marker above.
        if dir.join(".git").exists() {
            return root;
        }
        match dir.parent() {
            Some(parent) if parent != dir => dir = parent,
            _ => return root,
        }
    }
}

/// Whether the pnpm workspace which claims `project_root` also explicitly
/// claims its root package with `.`.
///
/// This is deliberately stronger than [`first_party_root_for`]: a nested host
/// may be a workspace member through `sub-packages/*` while the workspace root
/// itself is not a package. Callers must use this predicate before treating a
/// path directly under the workspace root as root-package first-party source.
pub fn workspace_explicitly_claims_root_package(project_root: &Path) -> bool {
    let project_root = normalize_path_lexical(project_root);
    let workspace_root = first_party_root_for(&project_root);
    if workspace_root == project_root {
        return false;
    }
    let marker = workspace_root.join("pnpm-workspace.yaml");
    std::fs::read_to_string(marker)
        .ok()
        .map(|yaml| {
            let mut explicitly_claimed = false;
            for glob in workspace_package_globs(&yaml) {
                let (negated, pattern) = match glob.strip_prefix('!') {
                    Some(rest) => (true, rest),
                    None => (false, glob.as_str()),
                };
                let pattern = pattern.strip_prefix("./").unwrap_or(pattern);
                let pattern = pattern.strip_suffix('/').unwrap_or(pattern);
                if negated {
                    let segments = if pattern == "." || pattern.is_empty() {
                        Vec::new()
                    } else {
                        pattern.split('/').collect::<Vec<_>>()
                    };
                    if glob_segments_match(&segments, &[]) {
                        explicitly_claimed = false;
                    }
                } else if pattern == "." || pattern.is_empty() {
                    explicitly_claimed = true;
                }
            }
            explicitly_claimed
        })
        .unwrap_or(false)
}

/// Whether the same pnpm workspace that claims `project_root` also claims the
/// concrete package directory `candidate`.
pub fn workspace_claims_package(project_root: &Path, candidate: &Path) -> bool {
    let project_root = normalize_path_lexical(project_root);
    let workspace_root = first_party_root_for(&project_root);
    let candidate = normalize_path_lexical(candidate);
    if workspace_root == project_root || !candidate.starts_with(&workspace_root) {
        return false;
    }
    std::fs::read_to_string(workspace_root.join("pnpm-workspace.yaml"))
        .ok()
        .map(|yaml| {
            workspace_claims_member(&workspace_package_globs(&yaml), &candidate, &workspace_root)
        })
        .unwrap_or(false)
}

/// Whether the `pnpm-workspace.yaml` **at `workspace_root`** claims
/// `candidate` as a package.
///
/// Unlike [`workspace_claims_package`], this takes the workspace root
/// directly instead of deriving it from a project root, so it also answers
/// the question for a workspace whose own root IS the project being built
/// (issue #1730's topology, where `first_party_root_for` returns the project
/// root and `workspace_claims_package` therefore bails out). Added for the
/// stage-escape audit-eligibility predicate — see
/// [`crate::audit_eligibility`].
///
/// Both paths are compared after lexical normalisation only; pass canonical
/// paths if the caller needs symlinks resolved. A missing or unreadable
/// marker fails closed (`false`).
pub fn workspace_root_claims_path(workspace_root: &Path, candidate: &Path) -> bool {
    let workspace_root = normalize_path_lexical(workspace_root);
    let candidate = normalize_path_lexical(candidate);
    if !candidate.starts_with(&workspace_root) {
        return false;
    }
    std::fs::read_to_string(workspace_root.join("pnpm-workspace.yaml"))
        .ok()
        .map(|yaml| {
            workspace_claims_member(&workspace_package_globs(&yaml), &candidate, &workspace_root)
        })
        .unwrap_or(false)
}

/// Extract the `packages:` glob list from `pnpm-workspace.yaml` without a
/// YAML dependency. Handles the two shapes pnpm documents: a block sequence
/// (`packages:\n  - 'sub-packages/*'`) and an inline flow sequence
/// (`packages: ['.', 'apps/*']`). Quotes and trailing `#` comments are
/// stripped. Unknown/exotic shapes yield an empty list, which fails closed
/// (no widening).
fn workspace_package_globs(yaml: &str) -> Vec<String> {
    fn strip_unquoted_comment(raw: &str) -> &str {
        let mut quote = None;
        let mut escaped = false;
        for (index, ch) in raw.char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            match quote {
                Some('"') if ch == '\\' => escaped = true,
                Some(active) if ch == active => quote = None,
                Some(_) => {}
                None if ch == '\'' || ch == '"' => quote = Some(ch),
                None if ch == '#' => return &raw[..index],
                None => {}
            }
        }
        raw
    }

    fn clean(raw: &str) -> Option<String> {
        let mut value = strip_unquoted_comment(raw).trim();
        if (value.starts_with('\'') && value.len() >= 2 && value.ends_with('\''))
            || (value.starts_with('"') && value.len() >= 2 && value.ends_with('"'))
        {
            value = &value[1..value.len() - 1];
        }
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    }

    let mut globs = Vec::new();
    let mut in_packages_block = false;
    for line in yaml.lines() {
        let trimmed = line.trim_end();
        if let Some(rest) = trimmed.strip_prefix("packages:") {
            let rest = strip_unquoted_comment(rest).trim();
            if let Some(inline) = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                globs.extend(inline.split(',').filter_map(clean));
                return globs;
            }
            in_packages_block = rest.is_empty() || rest.starts_with('#');
            continue;
        }
        if in_packages_block {
            let stripped = trimmed.trim_start();
            if let Some(item) = stripped
                .strip_prefix("- ")
                .or_else(|| (stripped == "-").then_some(""))
            {
                if let Some(value) = clean(item) {
                    globs.push(value);
                }
                continue;
            }
            if stripped.is_empty() || stripped.starts_with('#') {
                continue;
            }
            // Next top-level (or any non-item) key ends the block.
            break;
        }
    }
    globs
}

/// True when `project_root` is claimed as a member by the workspace rooted at
/// `workspace_root`, per pnpm `packages:` glob semantics: `*` matches exactly
/// one path segment, `**` matches any number (including zero), a leading `!`
/// negates, and `.` claims the workspace root itself. Later patterns win on
/// negation, matching pnpm's include-then-exclude behavior.
fn workspace_claims_member(globs: &[String], project_root: &Path, workspace_root: &Path) -> bool {
    let Ok(relative) = project_root.strip_prefix(workspace_root) else {
        return false;
    };
    let segments: Vec<String> = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();

    let mut claimed = false;
    for glob in globs {
        let (negated, pattern) = match glob.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, glob.as_str()),
        };
        let pattern = pattern.strip_prefix("./").unwrap_or(pattern);
        let pattern = pattern.strip_suffix('/').unwrap_or(pattern);
        let pattern_segments: Vec<&str> = if pattern == "." || pattern.is_empty() {
            Vec::new()
        } else {
            pattern.split('/').collect()
        };
        if glob_segments_match(&pattern_segments, &segments) {
            claimed = !negated;
        }
    }
    claimed
}

fn glob_segments_match(pattern: &[&str], path: &[String]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => {
            (0..=path.len()).any(|skip| glob_segments_match(rest, &path[skip..]))
        }
        Some((first, rest)) => {
            let Some((segment, tail)) = path.split_first() else {
                return false;
            };
            glob_segment_matches(first, segment) && glob_segments_match(rest, tail)
        }
    }
}

fn glob_segment_matches(pattern: &str, segment: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == segment,
        Some((prefix, suffix)) if !suffix.contains('*') => {
            segment.len() >= prefix.len() + suffix.len()
                && segment.starts_with(prefix)
                && segment.ends_with(suffix)
        }
        // Multiple `*` in one segment: fall back to a conservative
        // prefix/suffix check around the outermost stars.
        Some((prefix, _)) => {
            let suffix = pattern.rsplit_once('*').map(|(_, s)| s).unwrap_or("");
            segment.len() >= prefix.len() + suffix.len()
                && segment.starts_with(prefix)
                && segment.ends_with(suffix)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_project_root_without_workspace_marker() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("apps/site");
        std::fs::create_dir_all(&project).unwrap();
        assert_eq!(
            first_party_root_for(&project),
            normalize_path_lexical(&project)
        );
    }

    #[test]
    fn returns_nearest_ancestor_with_pnpm_workspace_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let project = workspace.join("sub-packages/host");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            workspace.join("pnpm-workspace.yaml"),
            "packages:\n  - '.'\n  - 'sub-packages/*'\n",
        )
        .unwrap();
        assert_eq!(
            first_party_root_for(&project),
            normalize_path_lexical(&workspace)
        );
    }

    #[test]
    fn root_package_membership_requires_an_explicit_dot_claim() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let project = workspace.join("sub-packages/host");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            workspace.join("pnpm-workspace.yaml"),
            "packages: ['sub-packages/*']\n",
        )
        .unwrap();
        assert_eq!(
            first_party_root_for(&project),
            normalize_path_lexical(&workspace)
        );
        assert!(!workspace_explicitly_claims_root_package(&project));

        std::fs::write(
            workspace.join("pnpm-workspace.yaml"),
            "packages:\n  - '.' # root package\n  - 'sub-packages/*' # hosts\n",
        )
        .unwrap();
        assert!(workspace_explicitly_claims_root_package(&project));

        std::fs::write(
            workspace.join("pnpm-workspace.yaml"),
            "packages: ['.', 'sub-packages/*'] # inline comment\n",
        )
        .unwrap();
        assert!(workspace_explicitly_claims_root_package(&project));

        std::fs::write(
            workspace.join("pnpm-workspace.yaml"),
            "packages: ['**', 'sub-packages/*']\n",
        )
        .unwrap();
        assert!(!workspace_explicitly_claims_root_package(&project));

        std::fs::write(
            workspace.join("pnpm-workspace.yaml"),
            "packages: ['.', 'sub-packages/*']\n",
        )
        .unwrap();
        assert!(workspace_explicitly_claims_root_package(&project));

        std::fs::write(
            workspace.join("pnpm-workspace.yaml"),
            "packages: ['.', 'sub-packages/*', '! .']\n",
        )
        .unwrap();
        // Whitespace makes this a different literal, so use the actual pnpm
        // negation spelling for the fail-closed, later-pattern-wins case.
        std::fs::write(
            workspace.join("pnpm-workspace.yaml"),
            "packages: ['.', 'sub-packages/*', '!.']\n",
        )
        .unwrap();
        assert!(!workspace_explicitly_claims_root_package(&project));
    }

    #[test]
    fn non_member_nested_project_is_not_widened() {
        // The CI regression shape: a fixture site nested inside a repository
        // that IS a pnpm workspace, but whose packages globs do not claim
        // the fixture. It must keep its own boundary.
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("repo");
        let fixture = workspace.join("tests/built-site-smoke/fixture-site");
        std::fs::create_dir_all(&fixture).unwrap();
        std::fs::write(
            workspace.join("pnpm-workspace.yaml"),
            "packages:\n  - 'packages/*'\n  - 'crates/zfb-islands/npm'\n",
        )
        .unwrap();
        assert_eq!(
            first_party_root_for(&fixture),
            normalize_path_lexical(&fixture)
        );
    }

    #[test]
    fn membership_supports_double_star_and_negation() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("ws");
        let claimed = workspace.join("apps/site/nested");
        let excluded = workspace.join("apps/legacy");
        std::fs::create_dir_all(&claimed).unwrap();
        std::fs::create_dir_all(&excluded).unwrap();
        std::fs::write(
            workspace.join("pnpm-workspace.yaml"),
            "packages:\n  - 'apps/**'\n  - '!apps/legacy'\n",
        )
        .unwrap();
        assert_eq!(
            first_party_root_for(&claimed),
            normalize_path_lexical(&workspace)
        );
        assert_eq!(
            first_party_root_for(&excluded),
            normalize_path_lexical(&excluded)
        );
    }

    #[test]
    fn membership_parses_comments_and_quotes() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("ws");
        let project = workspace.join("sub-packages/host");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            workspace.join("pnpm-workspace.yaml"),
            "# workspace config\npackages:\n  - '.'\n  # sub packages\n  - \"sub-packages/*\"\n\nshamefullyHoist: true\n",
        )
        .unwrap();
        assert_eq!(
            first_party_root_for(&project),
            normalize_path_lexical(&workspace)
        );
    }

    #[test]
    fn project_root_that_is_the_workspace_root_maps_to_itself() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("pnpm-workspace.yaml"), "packages: ['.']\n").unwrap();
        assert_eq!(
            first_party_root_for(&workspace),
            normalize_path_lexical(&workspace)
        );
    }

    #[test]
    fn vcs_root_bounds_the_ascent() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let project = repo.join("apps/site");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        // Workspace marker ABOVE the repository boundary must not widen.
        std::fs::write(dir.path().join("pnpm-workspace.yaml"), "packages: ['.']\n").unwrap();
        assert_eq!(
            first_party_root_for(&project),
            normalize_path_lexical(&project)
        );
    }

    #[test]
    fn workspace_marker_at_the_vcs_root_still_wins() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let project = repo.join("sub-packages/host");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(
            repo.join("pnpm-workspace.yaml"),
            "packages: ['.', 'sub-packages/*']\n",
        )
        .unwrap();
        assert_eq!(
            first_party_root_for(&project),
            normalize_path_lexical(&repo)
        );
    }

    #[test]
    fn nested_workspace_marker_prefers_the_nearest_one() {
        let dir = tempfile::tempdir().unwrap();
        let outer = dir.path().join("outer");
        let inner = outer.join("inner");
        let project = inner.join("pkg");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            outer.join("pnpm-workspace.yaml"),
            "packages: ['.', 'inner/pkg']\n",
        )
        .unwrap();
        std::fs::write(inner.join("pnpm-workspace.yaml"), "packages: ['pkg']\n").unwrap();
        assert_eq!(
            first_party_root_for(&project),
            normalize_path_lexical(&inner)
        );
    }
}
