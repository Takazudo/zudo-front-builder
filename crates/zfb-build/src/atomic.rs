//! Atomic file writes.
//!
//! `write-then-rename` is the canonical Unix recipe for "no reader ever
//! sees a half-written file". Both helpers below:
//!
//! 1. Create the parent directory if it doesn't exist.
//! 2. Write the bytes to a sibling temp file in the same directory.
//! 3. `fs::rename` the temp file over the destination. This is atomic on
//!    POSIX filesystems for files on the same filesystem.
//!
//! On Windows `std::fs::rename` is also atomic for replacing an existing
//! file (`MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` semantics).
//!
//! ## Why "same directory" matters
//!
//! `rename` is only atomic across the same filesystem. Putting the temp
//! file in the same directory as the destination guarantees this — using
//! e.g. `/tmp` would silently degrade to a copy + delete on multi-disk
//! setups, which is not atomic.
//!
//! ## Naming the temp file
//!
//! We use `<final>.tmp-<pid>-<seq>` so:
//!
//! - It clearly belongs to this build (process id) — easy to spot in
//!   `ls` output if a build crashes mid-write.
//! - A monotonic sequence number prevents collisions when the same
//!   destination is touched repeatedly inside one process.

use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Atomically write `bytes` to `dest`. See module-level docs.
pub fn atomic_write(dest: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
    }

    let temp_path = temp_sibling(dest);

    // Scope the file handle so it's flushed and dropped before rename.
    {
        let mut f = fs::File::create(&temp_path)
            .with_context(|| format!("failed to create temp file {}", temp_path.display()))?;
        f.write_all(bytes)
            .with_context(|| format!("failed to write temp file {}", temp_path.display()))?;
        f.sync_all()
            .with_context(|| format!("failed to fsync temp file {}", temp_path.display()))?;
    }

    // Rust's `std::fs::rename` is documented to replace an existing
    // destination on both POSIX and Windows (Windows: implemented via
    // `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`). We therefore
    // rely on it directly — no unlink-first fallback is required.
    // See https://doc.rust-lang.org/std/fs/fn.rename.html.
    if let Err(e) = fs::rename(&temp_path, dest) {
        // Best-effort cleanup so repeated rename failures don't accumulate
        // sibling temp files in the destination directory. Ignore the
        // remove error — the rename failure is the actionable signal.
        let _ = fs::remove_file(&temp_path);
        return Err(e).with_context(|| {
            format!(
                "failed to rename {} -> {}",
                temp_path.display(),
                dest.display()
            )
        });
    }

    Ok(())
}

/// Convenience: atomically write a `&str` to `dest`.
pub fn atomic_write_string(dest: &Path, s: &str) -> Result<()> {
    atomic_write(dest, s.as_bytes())
}

/// Validate that `dist_root.join(output_path)` lands inside `dist_root`.
///
/// Performs two layers of containment:
///
/// 1. **Lexical** — components of `output_path` are walked and any
///    `..` that would escape `dist_root`, any absolute root, and any
///    Windows prefix is rejected. This is sufficient for paths that
///    don't yet exist on disk (typical for build outputs).
/// 2. **Symlink-aware** — if the *parent* directory of the joined path
///    already exists, its canonical form must still start with the
///    canonical `dist_root`. This catches the case where a previous
///    build (or an attacker) planted a symlink inside dist that points
///    outside (e.g. `dist/foo -> /etc`); a lexical check alone would
///    accept the write and corrupt files outside dist.
///
/// The symlink check tolerates a missing parent directory (the file we
/// are about to write may live in a subdirectory `atomic_write` is
/// about to create), in which case only the lexical check applies. As
/// soon as the parent exists, the canonical comparison kicks in.
pub fn validate_output_path(dist_root: &Path, output_path: &Path) -> Result<PathBuf> {
    // Walk the components, building a normalized PathBuf relative to
    // dist_root. Reject absolute paths, Windows prefixes, and any `..`
    // that would walk above the root.
    let mut normalized: Vec<&std::ffi::OsStr> = Vec::new();
    for c in output_path.components() {
        match c {
            Component::Prefix(_) | Component::RootDir => {
                return Err(anyhow!(
                    "output path {} must be relative to dist_root {}",
                    output_path.display(),
                    dist_root.display()
                ));
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.pop().is_none() {
                    return Err(anyhow!(
                        "output path {} escapes dist_root {} via `..`",
                        output_path.display(),
                        dist_root.display()
                    ));
                }
            }
            Component::Normal(s) => normalized.push(s),
        }
    }

    let mut joined = dist_root.to_path_buf();
    for seg in normalized {
        joined.push(seg);
    }

    // Symlink-aware containment: walk up from the joined parent toward
    // dist_root, looking for the first component that already exists on
    // disk. Canonicalize THAT component (which resolves through any
    // symlink in its path) and require the canonical result to live
    // inside dist_root's canonical form.
    //
    // Why we walk: a partial symlink can bypass a naive
    // `parent.canonicalize()` check. If `dist/escape -> /tmp/outside`
    // exists but `dist/escape/newdir` does not, then
    // `dist/escape/newdir.canonicalize()` errors out. Falling through
    // would let `atomic_write`'s `create_dir_all` materialise the
    // directory under `/tmp/outside`. Walking up finds the existing
    // `dist/escape` first, canonicalizes it to `/tmp/outside`, and
    // rejects.
    //
    // If we walk all the way out of `dist_root` without finding any
    // existing component (the fresh-build case where `dist_root`
    // itself doesn't exist yet), we fall through to the lexical
    // result.
    if let (Some(parent), Ok(canon_root)) = (joined.parent(), dist_root.canonicalize()) {
        let mut current: Option<&Path> = Some(parent);
        while let Some(c) = current {
            // Lexical guard: never walk above dist_root. starts_with
            // works against either the lexical path or the canonical
            // form (which differ when dist_root is itself a symlink).
            if !c.starts_with(dist_root) && !c.starts_with(&canon_root) {
                return Err(anyhow!(
                    "output path {} escapes dist_root {} before reaching an existing parent",
                    output_path.display(),
                    canon_root.display(),
                ));
            }

            match c.symlink_metadata() {
                Ok(_) => {
                    // Component exists; canonicalize resolves any
                    // symlink in its path and gives us the real
                    // location.
                    let canonical = c.canonicalize().with_context(|| {
                        format!(
                            "failed to canonicalize {} while validating {}",
                            c.display(),
                            output_path.display()
                        )
                    })?;
                    if !canonical.starts_with(&canon_root) {
                        return Err(anyhow!(
                            "output path {} resolves to {} which is outside dist_root {} \
                             (likely a symlink pointing outside dist)",
                            output_path.display(),
                            canonical.display(),
                            canon_root.display(),
                        ));
                    }
                    break;
                }
                Err(_) => {
                    // Component doesn't exist yet — keep walking up.
                    current = c.parent();
                }
            }
        }
    }

    Ok(joined)
}

/// Build the sibling temp path. Pub for tests; not part of the stable API.
fn temp_sibling(dest: &Path) -> std::path::PathBuf {
    let pid = std::process::id();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);

    let mut name = dest
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("zfb"));
    name.push(format!(".tmp-{pid}-{seq}"));

    let mut out = dest.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    if out.as_os_str().is_empty() {
        out = std::path::PathBuf::from(".");
    }
    out.push(name);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn writes_file_atomically() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("nested/output.txt");
        atomic_write_string(&dest, "hello").unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "hello");
    }

    #[test]
    fn overwrites_existing_file() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("a.txt");
        atomic_write_string(&dest, "first").unwrap();
        atomic_write_string(&dest, "second").unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "second");
    }

    #[test]
    fn temp_sibling_is_in_same_dir() {
        let dest = Path::new("/tmp/some/dir/file.html");
        let t = temp_sibling(dest);
        assert_eq!(t.parent(), Some(Path::new("/tmp/some/dir")));
        let name = t.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("file.html.tmp-"), "got {name}");
    }

    #[test]
    fn validate_output_path_accepts_normal() {
        let root = Path::new("/dist");
        assert_eq!(
            validate_output_path(root, Path::new("blog/index.html")).unwrap(),
            PathBuf::from("/dist/blog/index.html"),
        );
    }

    #[test]
    fn validate_output_path_rejects_absolute() {
        let root = Path::new("/dist");
        assert!(validate_output_path(root, Path::new("/etc/passwd")).is_err());
    }

    #[test]
    fn validate_output_path_rejects_traversal() {
        let root = Path::new("/dist");
        assert!(
            validate_output_path(root, Path::new("../etc/passwd")).is_err()
        );
        assert!(
            validate_output_path(root, Path::new("blog/../../etc/passwd")).is_err()
        );
    }

    #[test]
    fn validate_output_path_allows_inner_dotdot() {
        // `blog/../index.html` resolves to `index.html`, which is still
        // inside dist_root, so it's allowed.
        let root = Path::new("/dist");
        assert_eq!(
            validate_output_path(root, Path::new("blog/../index.html")).unwrap(),
            PathBuf::from("/dist/index.html"),
        );
    }

    #[test]
    fn no_partial_temp_files_remain_after_success() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("a.txt");
        atomic_write_string(&dest, "hi").unwrap();

        let entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        // Only the destination file should exist; no leftover .tmp-* sibling.
        assert_eq!(entries, vec!["a.txt".to_string()]);
    }

    /// Regression guard for Round 2: a second write to the same path must
    /// succeed and replace the first content. `std::fs::rename` does the
    /// right thing on POSIX and Windows, but it costs nothing to verify
    /// here so a future rewrite cannot regress it.
    #[test]
    fn second_write_replaces_existing() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("a.txt");
        atomic_write(&dest, b"first").unwrap();
        atomic_write(&dest, b"second").unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"second");
        let entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["a.txt".to_string()]);
    }

    /// Symlink-aware containment: a symlink inside `dist` pointing at a
    /// location outside `dist` must be rejected, even though the
    /// lexical check accepts the relative path `escape/foo.txt`.
    #[cfg(unix)]
    #[test]
    fn validate_output_path_rejects_symlink_outside_dist() {
        use std::os::unix::fs::symlink;

        let dist_dir = tempdir().unwrap();
        let outside_dir = tempdir().unwrap();
        let dist_root = dist_dir.path().canonicalize().unwrap();
        let outside_root = outside_dir.path().canonicalize().unwrap();

        // Plant `dist/escape -> /tmp/.../outside`
        symlink(&outside_root, dist_root.join("escape")).unwrap();

        let err = validate_output_path(&dist_root, Path::new("escape/foo.txt"))
            .expect_err("symlink-traversed write must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("outside dist_root"),
            "error should mention outside dist_root: {msg}"
        );
    }

    /// Round 3 regression: a symlink inside `dist` pointing outside
    /// must still be rejected when the deeper path under it does not
    /// yet exist. With the naive `parent.canonicalize()` check, an
    /// output path of `escape/newdir/file.html` would skip the
    /// containment check (because `dist/escape/newdir` doesn't exist
    /// → canonicalize errors → fall through), and `atomic_write`
    /// would then `create_dir_all` outside dist_root.
    #[cfg(unix)]
    #[test]
    fn validate_output_path_rejects_symlink_outside_dist_partial_parent() {
        use std::os::unix::fs::symlink;

        let dist_dir = tempdir().unwrap();
        let outside_dir = tempdir().unwrap();
        let dist_root = dist_dir.path().canonicalize().unwrap();
        let outside_root = outside_dir.path().canonicalize().unwrap();

        // Plant `dist/escape -> /tmp/.../outside` but do NOT create
        // `dist/escape/newdir` — that's the directory `atomic_write`
        // would otherwise materialise outside dist_root.
        symlink(&outside_root, dist_root.join("escape")).unwrap();

        let err = validate_output_path(
            &dist_root,
            Path::new("escape/newdir/file.html"),
        )
        .expect_err("symlink with partial parent must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("outside dist_root"),
            "error should mention outside dist_root: {msg}"
        );
    }

    /// Symlink-aware containment must not break the common case: a
    /// non-symlinked subdirectory inside dist is still accepted.
    #[test]
    fn validate_output_path_accepts_real_subdir() {
        let dist_dir = tempdir().unwrap();
        let dist_root = dist_dir.path().canonicalize().unwrap();
        fs::create_dir_all(dist_root.join("blog")).unwrap();
        validate_output_path(&dist_root, Path::new("blog/index.html"))
            .expect("real subdir should validate");
    }

    /// If the parent directory does not yet exist (the typical fresh-build
    /// case), validation falls through to the lexical result.
    #[test]
    fn validate_output_path_tolerates_missing_parent() {
        let dist_dir = tempdir().unwrap();
        let dist_root = dist_dir.path().canonicalize().unwrap();
        // No `nested/` directory created.
        validate_output_path(&dist_root, Path::new("nested/deep/index.html"))
            .expect("missing parent should not error");
    }
}
