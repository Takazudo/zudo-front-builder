//! Windows-aware helpers shared by the `zfb` e2e test binaries through
//! `#[path = "support/e2e_platform.rs"] mod e2e_platform;` (#4508).
//!
//! Each binary uses only a subset, hence the module-wide `dead_code` allow.
#![allow(dead_code)]

use std::path::Path;

// pnpm ships as `pnpm.cmd` on Windows; `Command` only auto-appends `.exe`.
pub const PNPM_PROGRAM: &str = if cfg!(windows) { "pnpm.cmd" } else { "pnpm" };

pub fn npm_program() -> &'static str {
    // npm ships as a .cmd shim on Windows, which CreateProcess does not resolve from a bare name.
    if cfg!(windows) {
        "npm.cmd"
    } else {
        "npm"
    }
}

/// Lexically resolves `.` and `..` so a relative link target can be turned
/// into the absolute path a Windows junction requires.
#[cfg(windows)]
fn lexical_normalize(path: &Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Creates a directory link at `link` pointing at `target` (absolute, or
/// relative to `link`'s parent on unix and for copied relative links).
#[cfg(unix)]
pub fn link_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
pub fn link_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    use std::process::{Command, Stdio};

    // Symlink creation needs a privilege that is not guaranteed; a junction
    // needs none, so fall back to `mklink /J` with an absolute target.
    if std::os::windows::fs::symlink_dir(target, link).is_ok() {
        return Ok(());
    }
    let absolute = if target.is_absolute() {
        target.to_path_buf()
    } else {
        lexical_normalize(&link.parent().unwrap_or(Path::new("")).join(target))
    };
    let status = Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(&absolute)
        .stdout(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "mklink /J {} {} failed: {status}",
            link.display(),
            absolute.display()
        )))
    }
}
