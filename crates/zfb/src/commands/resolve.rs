//! Shared path and port resolution helpers for the `build`, `dev`, and
//! `preview` commands.
//!
//! All three commands share the same logic for resolving user-supplied paths
//! against the project root and for picking a port using the
//! CLI > config > built-in-default precedence rule. Centralising the helpers
//! here prevents the implementations from drifting independently and gives
//! one place to add tests.

use std::path::{Path, PathBuf};

/// Resolve `path` against `root` if it is relative; absolute paths are
/// returned unchanged. Pure path arithmetic — no I/O, so it works equally
/// for paths that do not yet exist.
pub fn resolve_under_root(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

/// Alias of [`resolve_under_root`] for the `build` command context where
/// the resolved value is specifically the output directory.
pub fn resolve_outdir(root: &Path, path: &Path) -> PathBuf {
    resolve_under_root(root, path)
}

/// CLI override > config value > built-in default precedence rule.
///
/// `default_port` is the caller's built-in constant (e.g. `DEFAULT_DEV_PORT`
/// in `dev.rs` or `DEFAULT_PREVIEW_PORT` in `preview.rs`). Passing the
/// default explicitly keeps this function pure and avoids coupling it to
/// either command's constant.
pub fn resolve_port(cli: Option<u16>, cfg: Option<u16>, default_port: u16) -> u16 {
    cli.or(cfg).unwrap_or(default_port)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- resolve_under_root / resolve_outdir --------------------------------

    #[test]
    fn resolve_under_root_joins_relative_paths_onto_root() {
        let root = Path::new("/tmp/proj");
        assert_eq!(
            resolve_under_root(root, Path::new("dist")),
            PathBuf::from("/tmp/proj/dist")
        );
        assert_eq!(
            resolve_under_root(root, Path::new("build/out")),
            PathBuf::from("/tmp/proj/build/out")
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_under_root_keeps_absolute_paths_as_is() {
        let root = Path::new("/tmp/proj");
        assert_eq!(
            resolve_under_root(root, Path::new("/var/www/dist")),
            PathBuf::from("/var/www/dist")
        );
    }

    #[test]
    fn resolve_under_root_handles_dot_relative() {
        let root = Path::new("/tmp/proj");
        let resolved = resolve_under_root(root, Path::new("./public"));
        assert!(
            resolved.starts_with(root),
            "expected {resolved:?} to start with {root:?}"
        );
    }

    #[test]
    fn resolve_outdir_joins_relative_paths_onto_root() {
        let root = Path::new("/proj");
        assert_eq!(
            resolve_outdir(root, Path::new("dist")),
            PathBuf::from("/proj/dist")
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_outdir_keeps_absolute_paths_as_is() {
        let root = Path::new("/proj");
        assert_eq!(
            resolve_outdir(root, Path::new("/tmp/zfb-out")),
            PathBuf::from("/tmp/zfb-out")
        );
    }

    // ---- resolve_port -------------------------------------------------------

    #[test]
    fn resolve_port_prefers_cli_over_config() {
        assert_eq!(resolve_port(Some(8080), Some(4000), 3000), 8080);
    }

    #[test]
    fn resolve_port_falls_back_to_config_when_cli_absent() {
        assert_eq!(resolve_port(None, Some(4000), 3000), 4000);
    }

    #[test]
    fn resolve_port_falls_back_to_builtin_when_neither_supplied() {
        // The default is chosen by the caller; test with the two
        // concrete built-in values used by dev (3000) and preview (4321).
        assert_eq!(resolve_port(None, None, 3000), 3000);
        assert_eq!(resolve_port(None, None, 4321), 4321);
    }
}
