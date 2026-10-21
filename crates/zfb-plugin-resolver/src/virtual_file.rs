//! Content-addressed materialization of one plugin virtual module (#4367).
//!
//! esbuild hashes each input's path into the name of the chunk that holds it,
//! so a randomly named temp file renamed that chunk on every build of
//! identical input. The file name is therefore derived from the specifier and
//! source, which makes concurrent builds of one project share a file.
//!
//! Sharing uses the #1976 lock protocol: every holder keeps a shared lock,
//! and only a process that wins an exclusive lock on a file that is still
//! the one at the path may delete it.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

const MAX_ACQUIRE_ATTEMPTS: usize = 32;

/// A materialized virtual module, held under a shared lock until dropped.
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
            let file = match OpenOptions::new().read(true).open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    match publish(dir, prefix, suffix, &path, source)? {
                        Some(file) => file,
                        None => continue,
                    }
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
    match candidate.try_lock() {
        Ok(()) => file_still_at_path(candidate, path),
        Err(TryLockError::WouldBlock | TryLockError::Error(_)) => false,
    }
}

fn file_still_at_path(file: &File, path: &Path) -> bool {
    let Some(held) = file
        .try_clone()
        .ok()
        .and_then(|clone| same_file::Handle::from_file(clone).ok())
    else {
        return false;
    };
    same_file::Handle::from_path(path).is_ok_and(|current| current == held)
}

/// Write `source` beside `path` and move it into place without replacing an
/// existing file. `None` means another process published it first.
fn publish(
    dir: &Path,
    prefix: &str,
    suffix: &str,
    path: &Path,
    source: &str,
) -> Result<Option<File>> {
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
        Ok(file) => Ok(Some(file)),
        Err(error) if error.error.kind() == ErrorKind::AlreadyExists => Ok(None),
        Err(error) => Err(error.error)
            .with_context(|| format!("failed to publish virtual-module file `{}`", path.display())),
    }
}

fn file_name(prefix: &str, suffix: &str, specifier: &str, source: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update((specifier.len() as u64).to_le_bytes());
    hasher.update(specifier.as_bytes());
    hasher.update(source.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{prefix}{hex}{suffix}")
}
