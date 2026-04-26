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
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};

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
}
