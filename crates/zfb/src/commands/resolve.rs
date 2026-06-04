//! Shared path and port resolution helpers for the `build`, `dev`, and
//! `preview` commands.
//!
//! All three commands share the same logic for resolving user-supplied paths
//! against the project root and for picking a port using the
//! CLI > config > built-in-default precedence rule. Centralising the helpers
//! here prevents the implementations from drifting independently and gives
//! one place to add tests.

use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

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
}
