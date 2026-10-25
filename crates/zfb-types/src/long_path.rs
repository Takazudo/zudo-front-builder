//! Windows long-path spelling for the project root (#4499/#4516).
//!
//! An 8.3 short spelling (`C:\Users\RUNNER~1\…`) of the project root reached
//! the bundler through symlinked `node_modules` while the islands scanner
//! spelled the same installed files canonically (long), so one module got two
//! ids and the zudo-react reactive core was bundled twice. Expanding the short
//! components once at command entry gives every downstream consumer a single
//! spelling.

use std::path::{Path, PathBuf};

/// `path` with Windows 8.3 short components expanded (`RUNNER~1` -> `runneradmin`).
/// Identity on every other OS. Junctions, symlinks and subst drives are kept as
/// spelled (unlike `fs::canonicalize`, which resolves them and returns `\\?\`),
/// so the result stays comparable with what oxc_resolver/Rolldown and esbuild
/// derive from the same spelling. On a Win32 failure (path gone, ERROR_*) the
/// input is returned unchanged; never an error.
pub fn long_path_spelling(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        windows::call_path_api(windows::GetLongPathNameW, path)
            .unwrap_or_else(|| path.to_path_buf())
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}

/// `std::env::current_dir()` passed through `long_path_spelling`.
/// The one entry point the `zfb` commands use for `project_root`.
pub fn current_dir_long() -> std::io::Result<PathBuf> {
    std::env::current_dir().map(|cwd| long_path_spelling(&cwd))
}

#[cfg(windows)]
mod windows {
    use std::{
        ffi::OsString,
        os::windows::ffi::{OsStrExt, OsStringExt},
        path::{Path, PathBuf},
    };

    pub(super) type PathApi = unsafe extern "system" fn(*const u16, *mut u16, u32) -> u32;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub(super) fn GetLongPathNameW(short: *const u16, long: *mut u16, cch: u32) -> u32;
        #[cfg(test)]
        pub(super) fn GetShortPathNameW(long: *const u16, short: *mut u16, cch: u32) -> u32;
    }

    /// Run a `Get{Long,Short}PathNameW`-shaped API on `path`. Both return the
    /// copied length without the NUL on success, the required length including
    /// the NUL when the buffer is too small, and 0 on failure.
    pub(super) fn call_path_api(api: PathApi, path: &Path) -> Option<PathBuf> {
        let input: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut buffer = vec![0u16; 260];
        for _ in 0..2 {
            // SAFETY: `input` is NUL-terminated and `buffer` holds exactly the
            // `cch` units passed; both outlive this synchronous call.
            let len =
                unsafe { api(input.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
            if len == 0 {
                return None;
            }
            if len < buffer.len() {
                return Some(PathBuf::from(OsString::from_wide(&buffer[..len])));
            }
            buffer = vec![0u16; len];
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    use std::fs;

    /// `fs::canonicalize` without the `\\?\` verbatim prefix: the long spelling.
    #[cfg(windows)]
    fn canonical_long(path: &Path) -> PathBuf {
        let canonical = fs::canonicalize(path).unwrap();
        let text = canonical.to_string_lossy();
        PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&*text))
    }

    /// The 8.3 alias of `long`; panics when the volume generates no 8.3 names,
    /// so a Windows run never passes without exercising an expansion.
    #[cfg(windows)]
    fn short_spelling(long: &Path) -> PathBuf {
        let short = windows::call_path_api(windows::GetShortPathNameW, long)
            .unwrap_or_else(|| panic!("GetShortPathNameW failed for {}", long.display()));
        if short == long {
            panic!(
                "no 8.3 short name for {}; 8.3 name generation is disabled on this volume, \
                 so the long-path expansion cannot be tested here",
                long.display()
            );
        }
        short
    }

    #[cfg(windows)]
    #[test]
    fn short_components_expand_to_the_long_spelling() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("long-component-name");
        fs::create_dir(&dir).unwrap();
        let long = canonical_long(&dir);
        let short = short_spelling(&long);
        assert_eq!(long_path_spelling(&short), long);
    }

    #[cfg(windows)]
    #[test]
    fn long_spelling_is_a_fixed_point() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("long-component-name");
        fs::create_dir(&dir).unwrap();
        let long = canonical_long(&dir);
        assert_eq!(long_path_spelling(&long), long);
    }

    #[cfg(windows)]
    #[test]
    fn junction_target_is_not_resolved() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("long-real-directory");
        fs::create_dir(&real).unwrap();
        let link = canonical_long(temp.path()).join("long-linked-directory");
        crate::directory_link::create(&real, &link).unwrap();
        let expanded = long_path_spelling(&short_spelling(&link));
        assert_eq!(expanded, link);
        assert!(expanded.ends_with("long-linked-directory"));
        assert_ne!(expanded, canonical_long(&real));
    }

    #[cfg(windows)]
    #[test]
    fn missing_path_is_returned_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let missing = canonical_long(temp.path()).join("no-such-dir~1");
        assert_eq!(long_path_spelling(&missing), missing);
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_is_identity() {
        for path in [
            "/tmp/RUNNER~1/positive-consumer",
            "relative/no-such-dir~1",
            "/var/folders/x",
        ] {
            assert_eq!(long_path_spelling(Path::new(path)), PathBuf::from(path));
        }
    }

    #[test]
    fn current_dir_long_is_absolute_and_exists() {
        let cwd = current_dir_long().unwrap();
        assert!(cwd.is_absolute(), "{}", cwd.display());
        assert!(cwd.is_dir(), "{}", cwd.display());
    }
}
