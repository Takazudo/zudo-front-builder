//! Runtime detection of the first-party source boundary (issue #1664).
//!
//! In a pnpm workspace, a sub-package host legitimately imports sibling
//! workspace source through tsconfig path aliases (`@/*` -> `../../*`).
//! The module-worker / island preprocessing contract must therefore treat
//! the **workspace** — not the single package directory — as the first-party
//! boundary. The workspace root is the nearest ancestor directory (including
//! `project_root` itself) containing `pnpm-workspace.yaml`; without one, the
//! boundary stays `project_root`.

use std::path::{Path, PathBuf};

use crate::normalize_path_lexical;

/// Widest directory whose files count as first-party sources for
/// `project_root`.
///
/// Returns the nearest ancestor of `project_root` (including `project_root`
/// itself) that contains `pnpm-workspace.yaml`, or `project_root` unchanged
/// when no such ancestor exists. The ascent stops at the nearest VCS root (a
/// directory containing `.git`) so a stray workspace marker above the
/// repository can never widen the boundary. The returned path preserves the
/// lexical spelling derived from `project_root` (no canonicalization), so
/// callers can keep comparing logical paths the way they already do with
/// `project_root`.
pub fn first_party_root_for(project_root: &Path) -> PathBuf {
    let root = normalize_path_lexical(project_root);
    let mut dir = root.as_path();
    loop {
        if dir.join("pnpm-workspace.yaml").is_file() {
            return dir.to_path_buf();
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
        std::fs::write(workspace.join("pnpm-workspace.yaml"), "packages: ['.']\n").unwrap();
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
        std::fs::write(repo.join("pnpm-workspace.yaml"), "packages: ['.']\n").unwrap();
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
        std::fs::write(outer.join("pnpm-workspace.yaml"), "packages: ['.']\n").unwrap();
        std::fs::write(inner.join("pnpm-workspace.yaml"), "packages: ['.']\n").unwrap();
        assert_eq!(
            first_party_root_for(&project),
            normalize_path_lexical(&inner)
        );
    }
}
