//! Shared path and port resolution helpers for the `build`, `dev`, and
//! `preview` commands.
//!
//! All three commands share the same logic for resolving user-supplied paths
//! against the project root and for picking a port using the
//! CLI > config > built-in-default precedence rule. Centralising the helpers
//! here prevents the implementations from drifting independently and gives
//! one place to add tests.

use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

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

/// Guard against a dangerous `outdir` that would cause `wipe_outdir_contents`
/// to delete the project sources.
///
/// Hard-bails when the canonicalised `outdir` is equal to `project_root` or is
/// an ancestor of `project_root`. Both conditions mean wiping `outdir` would
/// delete the project sources. The check uses `canonicalize` so symlinks in
/// either path are resolved before comparison (mirrors the overlap check in
/// `dev.rs` and the traversal guard in `config.rs:ensure_path_in_root`).
///
/// No source-marker heuristics (package.json, src/, …) — a previous valid
/// build may have copied those from `public/` into `dist/` (#805).
///
/// Returns `Ok(())` when `outdir` does not yet exist (no-op wipe ahead) or
/// when it is a safe subdirectory of `project_root`.
pub fn validate_outdir_safety(project_root: &Path, outdir: &Path) -> Result<()> {
    // If outdir does not exist yet there is nothing to wipe and nothing unsafe.
    if !outdir.exists() {
        return Ok(());
    }

    let canonical_out = outdir
        .canonicalize()
        .with_context(|| format!("failed to canonicalize outdir {}", outdir.display()))?;

    let canonical_root = project_root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize project root {}", project_root.display()))?;

    if canonical_out == canonical_root || canonical_root.starts_with(&canonical_out) {
        bail!(
            "outdir ({}) resolves to {} which is the project root or an ancestor of it — \
             wiping it would delete project sources. Choose an outdir inside the project \
             (the default `dist/` is safe).",
            outdir.display(),
            canonical_out.display(),
        );
    }

    Ok(())
}

/// Wipe the contents of `outdir` without removing `outdir` itself.
///
/// Behaviour:
/// - No-op when `outdir` does not exist.
/// - Child symlinks are unlinked (not traversed) — a symlink inside `outdir`
///   pointing elsewhere is removed as a link; its target is untouched.
/// - Child directories are removed recursively via [`std::fs::remove_dir_all`].
/// - Child regular files are removed via [`std::fs::remove_file`].
///
/// `outdir` itself is never removed, so an outdir that is itself a symlink or
/// bind-mount survives intact with its contents replaced.
pub fn wipe_outdir_contents(outdir: &Path) -> Result<()> {
    let entries = match std::fs::read_dir(outdir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(e).with_context(|| {
                format!("failed to read outdir {} for wiping", outdir.display())
            })
        }
    };

    for entry in entries {
        let entry = entry.with_context(|| {
            format!("failed to iterate outdir entries in {}", outdir.display())
        })?;
        let path = entry.path();

        // Use symlink_metadata so we detect symlinks without following them.
        let meta = std::fs::symlink_metadata(&path).with_context(|| {
            format!("failed to stat {} during outdir wipe", path.display())
        })?;

        if meta.file_type().is_symlink() {
            // Remove the link itself; do NOT recurse through it.
            std::fs::remove_file(&path)
                .with_context(|| format!("failed to remove symlink {} from outdir", path.display()))?;
        } else if meta.is_dir() {
            std::fs::remove_dir_all(&path)
                .with_context(|| format!("failed to remove directory {} from outdir", path.display()))?;
        } else {
            std::fs::remove_file(&path)
                .with_context(|| format!("failed to remove file {} from outdir", path.display()))?;
        }
    }

    Ok(())
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

/// CLI override > config value > built-in default precedence for the bind
/// host. Mirrors [`resolve_port`]; `default_host` is the caller's built-in
/// constant (e.g. `DEFAULT_DEV_HOST` in `dev.rs` or `DEFAULT_PREVIEW_HOST` in
/// `preview.rs`). Shared by `dev` and `preview` so the two stay symmetric.
pub fn resolve_host(cli: Option<&str>, cfg: Option<&str>, default_host: &str) -> String {
    cli.or(cfg).unwrap_or(default_host).to_owned()
}

/// Resolve a `host:port` pair into a bindable [`SocketAddr`]. Accepts the same
/// host forms both `dev` and `preview` support (`localhost`, `127.0.0.1`,
/// `0.0.0.0`, IPv6, …).
///
/// When `host` is `"localhost"` (case-insensitive), the first IPv4 address
/// among the resolved candidates is preferred so that the dev/preview server
/// banner (`http://localhost:PORT/`) matches the bound address family. If no
/// IPv4 address is available (rare IPv6-only environments), the first resolved
/// address is used as a fallback. All other host forms — explicit IP literals,
/// IPv6 bracket addresses, named hosts — return the first resolved address
/// unchanged.
pub fn resolve_addr(host: &str, port: u16) -> Result<SocketAddr> {
    let pair = format!("{host}:{port}");
    let addrs: Vec<SocketAddr> = pair
        .to_socket_addrs()
        .with_context(|| format!("could not resolve bind address {pair}"))?
        .collect();
    let chosen = if host.eq_ignore_ascii_case("localhost") {
        // Prefer IPv4 so the printed URL (http://localhost:PORT/) and the
        // actual bound address are on the same family; fall back to first if
        // the resolver returns only IPv6 (e.g. some Docker / minimal containers).
        addrs.iter().find(|a| a.is_ipv4()).or_else(|| addrs.first()).copied()
    } else {
        addrs.first().copied()
    };
    chosen.ok_or_else(|| anyhow::anyhow!("no socket addresses resolved for {pair}"))
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

    // ---- resolve_host -------------------------------------------------------

    #[test]
    fn resolve_host_prefers_cli_over_config() {
        assert_eq!(
            resolve_host(Some("0.0.0.0"), Some("127.0.0.1"), "localhost"),
            "0.0.0.0"
        );
    }

    #[test]
    fn resolve_host_falls_back_to_config_when_cli_absent() {
        assert_eq!(
            resolve_host(None, Some("127.0.0.1"), "localhost"),
            "127.0.0.1"
        );
    }

    #[test]
    fn resolve_host_falls_back_to_builtin_when_neither_supplied() {
        assert_eq!(resolve_host(None, None, "localhost"), "localhost");
    }

    // ---- resolve_addr -------------------------------------------------------

    #[test]
    fn resolve_addr_binds_loopback_and_unspecified() {
        let loopback = resolve_addr("127.0.0.1", 4321).unwrap();
        assert_eq!(loopback.port(), 4321);
        assert!(loopback.ip().is_loopback());

        let any = resolve_addr("0.0.0.0", 4321).unwrap();
        assert!(any.ip().is_unspecified(), "0.0.0.0 must bind all interfaces");
    }

    /// Verify that `resolve_addr("localhost", …)` returns an IPv4 loopback
    /// address when one is available — the banner URL `http://localhost:PORT/`
    /// must match the bound address family (fixes #725).
    ///
    /// On extremely rare IPv6-only environments (some Docker / minimal
    /// containers) the resolver may return only `[::1]`; in that case the
    /// fallback-to-first behaviour still applies and the test would fail.
    /// Mark the test `#[ignore]` manually if running in such an environment.
    #[test]
    fn resolve_addr_localhost_prefers_ipv4() {
        let addr = resolve_addr("localhost", 4321).expect("localhost must resolve");
        assert!(addr.is_ipv4(), "expected IPv4 for localhost, got {addr:?}");
        assert!(
            addr.ip().is_loopback(),
            "expected loopback IP for localhost, got {addr:?}"
        );
        assert_eq!(addr.port(), 4321);
    }

    /// Verify the case-insensitive match — `LOCALHOST` should also prefer IPv4.
    #[test]
    fn resolve_addr_localhost_uppercase_prefers_ipv4() {
        let addr = resolve_addr("LOCALHOST", 4321).expect("LOCALHOST must resolve");
        assert!(addr.is_ipv4(), "expected IPv4 for LOCALHOST, got {addr:?}");
        assert!(addr.ip().is_loopback());
    }

    // ---- validate_outdir_safety ---------------------------------------------

    #[cfg(unix)]
    #[test]
    fn validate_outdir_safety_accepts_nonexistent_outdir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // outdir does not exist yet — safe, no-op wipe ahead.
        assert!(validate_outdir_safety(root, &root.join("dist")).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn validate_outdir_safety_accepts_safe_subdirectory() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let outdir = root.join("dist");
        std::fs::create_dir_all(&outdir).unwrap();
        assert!(validate_outdir_safety(root, &outdir).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn validate_outdir_safety_rejects_outdir_equal_to_project_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let err = validate_outdir_safety(root, root).unwrap_err();
        assert!(
            err.to_string().contains("project root or an ancestor"),
            "unexpected error: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn validate_outdir_safety_rejects_outdir_as_ancestor_of_project_root() {
        let tmp = tempfile::tempdir().unwrap();
        // Nest: root = tmp/proj, outdir = tmp (ancestor of root).
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let ancestor = tmp.path();
        let err = validate_outdir_safety(&root, ancestor).unwrap_err();
        assert!(
            err.to_string().contains("project root or an ancestor"),
            "unexpected error: {err}"
        );
    }

    // ---- wipe_outdir_contents -----------------------------------------------

    #[cfg(unix)]
    #[test]
    fn wipe_outdir_contents_is_noop_when_outdir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let outdir = tmp.path().join("dist");
        // Must not error even though outdir does not exist.
        assert!(wipe_outdir_contents(&outdir).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn wipe_outdir_contents_removes_files_and_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let outdir = tmp.path().join("dist");
        std::fs::create_dir_all(outdir.join("assets")).unwrap();
        std::fs::write(outdir.join("index.html"), b"hello").unwrap();
        std::fs::write(outdir.join("assets/main.js"), b"var x=1").unwrap();

        wipe_outdir_contents(&outdir).unwrap();

        assert!(outdir.exists(), "outdir itself must survive the wipe");
        assert!(!outdir.join("index.html").exists());
        assert!(!outdir.join("assets").exists());
    }

    #[cfg(unix)]
    #[test]
    fn wipe_outdir_contents_unlinks_child_symlink_not_target() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let outdir = tmp.path().join("dist");
        std::fs::create_dir_all(&outdir).unwrap();

        // Create a target file outside outdir.
        let target = tmp.path().join("outside.txt");
        std::fs::write(&target, b"keep me").unwrap();

        // Create a symlink inside outdir pointing at the outside target.
        let link = outdir.join("link.txt");
        symlink(&target, &link).unwrap();

        wipe_outdir_contents(&outdir).unwrap();

        assert!(!link.exists(), "symlink must be removed from outdir");
        assert!(target.exists(), "symlink target outside outdir must be untouched");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep me");
    }

    #[cfg(unix)]
    #[test]
    fn wipe_outdir_contents_outdir_itself_is_a_symlink_survives() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let real_dir = tmp.path().join("real_dist");
        std::fs::create_dir_all(&real_dir).unwrap();
        std::fs::write(real_dir.join("stale.js"), b"old").unwrap();

        // dist/ is a symlink pointing at real_dist/.
        let dist_link = tmp.path().join("dist");
        symlink(&real_dir, &dist_link).unwrap();

        wipe_outdir_contents(&dist_link).unwrap();

        // The symlink itself must still exist (we only wipe children).
        assert!(
            std::fs::symlink_metadata(&dist_link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "dist symlink must survive the wipe"
        );
        // The stale file inside the real directory must be gone.
        assert!(!real_dir.join("stale.js").exists());
    }
}
