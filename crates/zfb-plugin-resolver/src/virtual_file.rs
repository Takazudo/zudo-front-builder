//! Content-addressed materialization of one plugin virtual module (#4367).
//!
//! esbuild hashes each input's path into the name of the chunk that holds it,
//! so a randomly named temp file renamed that chunk on every build of
//! identical input. The file name is therefore derived from the specifier and
//! source, which makes concurrent builds of one project share a file.
//!
//! Every holder keeps the file pinned at its path until it drops, and the
//! last holder removes it:
//!
//! - Unix: holders keep the #1976 shared lock. Only a process that wins an
//!   exclusive lock on the file still at the path may delete it, and a holder
//!   re-checks after locking that a releasing holder did not remove its file.
//! - Windows: holders open the file without delete sharing, so the OS refuses
//!   every removal while any holder is alive.

use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

const MAX_ACQUIRE_ATTEMPTS: usize = 64;

/// A materialized virtual module, pinned at its path until dropped.
/// The last holder to drop it removes it from disk.
pub struct VirtualModuleFile {
    path: PathBuf,
    file: Option<File>,
}

impl VirtualModuleFile {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn as_file(&self) -> &File {
        self.file.as_ref().expect("held until drop")
    }

    pub(crate) fn acquire(
        dir: &Path,
        prefix: &str,
        suffix: &str,
        specifier: &str,
        source: &str,
    ) -> Result<Self> {
        let path = dir.join(file_name(prefix, suffix, specifier, source));
        for _ in 0..MAX_ACQUIRE_ATTEMPTS {
            let file = match open_holder(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    publish(dir, prefix, suffix, &path, source)?;
                    continue;
                }
                Err(error) if is_transient_sharing_error(&error) => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("failed to open virtual-module file `{}`", path.display())
                    })
                }
            };
            // A failed lock leaves only the sweeper's age gate protecting
            // the file, as before this file was content-addressed.
            let _ = file.lock_shared();
            // A releasing holder may have removed the file between our open
            // and our lock; a newer one may already stand at the path.
            if file_still_at_path(&file, &path) {
                return Ok(Self {
                    path,
                    file: Some(file),
                });
            }
        }
        bail!(
            "virtual-module file `{}` kept being replaced while it was acquired",
            path.display()
        )
    }
}

impl Drop for VirtualModuleFile {
    fn drop(&mut self) {
        drop(self.file.take());
        // Windows refuses this while another holder's handle is open.
        let Ok(candidate) = OpenOptions::new().read(true).write(true).open(&self.path) else {
            return;
        };
        if exclusive_lock_on_file_at_path(&candidate, &self.path) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Is `candidate` provably unowned and still the file at `path`?
///
/// Callers that delete `path` on a `true` result cannot remove a newer file:
/// no file can replace `path` while the exclusive lock on it is held, since
/// publishing never overwrites and every remover takes the same lock first.
pub fn exclusive_lock_on_file_at_path(candidate: &File, path: &Path) -> bool {
    candidate.try_lock().is_ok() && file_still_at_path(candidate, path)
}

#[cfg(unix)]
fn file_still_at_path(file: &File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (file.metadata(), std::fs::metadata(path)) {
        (Ok(held), Ok(current)) => held.dev() == current.dev() && held.ino() == current.ino(),
        _ => false,
    }
}

/// A Windows holder's handle was opened through `path` without delete
/// sharing, so the file it names cannot have been removed or replaced.
#[cfg(not(unix))]
fn file_still_at_path(_file: &File, path: &Path) -> bool {
    path.is_file()
}

#[cfg(windows)]
fn open_holder(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_SHARE_READ only: esbuild can read the file, but no other handle
    // may delete or write it while this one is open.
    const FILE_SHARE_READ: u32 = 0x1;
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
}

#[cfg(not(windows))]
fn open_holder(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().read(true).open(path)
}

/// A Windows sharing violation, or access to a file pending deletion: another
/// process is publishing, sweeping or removing the file right now.
fn is_transient_sharing_error(error: &std::io::Error) -> bool {
    cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32))
}

/// Write `source` beside `path` and move it into place without replacing an
/// existing file. Losing that race to another publisher is not an error.
fn publish(dir: &Path, prefix: &str, suffix: &str, path: &Path, source: &str) -> Result<()> {
    let staging = tempfile::Builder::new()
        .prefix(prefix)
        .suffix(suffix)
        .tempfile_in(dir)
        .with_context(|| {
            format!(
                "failed to allocate a virtual-module staging file inside `{}`",
                dir.display()
            )
        })?;
    std::fs::write(staging.path(), source.as_bytes()).with_context(|| {
        format!(
            "failed to write virtual-module source to `{}`",
            staging.path().display()
        )
    })?;
    match staging.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(error)
            if error.error.kind() == ErrorKind::AlreadyExists
                || is_transient_sharing_error(&error.error) =>
        {
            Ok(())
        }
        Err(error) => Err(error.error)
            .with_context(|| format!("failed to publish virtual-module file `{}`", path.display())),
    }
}

fn file_name(prefix: &str, suffix: &str, specifier: &str, source: &str) -> String {
    let mut key = Vec::with_capacity(8 + specifier.len() + source.len());
    key.extend_from_slice(&(specifier.len() as u64).to_le_bytes());
    key.extend_from_slice(specifier.as_bytes());
    key.extend_from_slice(source.as_bytes());
    let high = zfb_types::module_worker_content_hash(&key);
    key.push(0);
    let low = zfb_types::module_worker_content_hash(&key);
    format!("{prefix}{high}{low}{suffix}")
}
