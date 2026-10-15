//! Directory links used by dependency staging, without elevated Windows privileges.

use std::{fs, io, path::Path};

/// Create a link at an absent destination. Never copies or replaces the target.
pub fn create(target: &Path, link: &Path) -> io::Result<()> {
    let canonical = fs::canonicalize(target)?;
    if !canonical.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "directory link target is not a directory",
        ));
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(std::path::absolute(target)?, link)
    }
    #[cfg(windows)]
    {
        windows::create_junction(&canonical, link)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = link;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "directory links are unsupported",
        ))
    }
}

/// Remove a link itself, including a dangling Windows directory junction.
pub fn remove(link: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(link)?;
    if !metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to unlink a non-link",
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt;
        if metadata.file_type().is_symlink_dir() {
            return fs::remove_dir(link);
        }
    }
    fs::remove_file(link)
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::{
        ffi::c_void,
        os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle},
        path::{Component, Prefix},
    };

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn DeviceIoControl(
            handle: *mut c_void,
            code: u32,
            input: *const c_void,
            input_len: u32,
            output: *mut c_void,
            output_len: u32,
            returned: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
    }

    // Mount-point layout and lengths: MS-FSCC REPARSE_DATA_BUFFER. The common
    // header is 8 bytes; both substitute and print names need a trailing NUL.
    #[repr(C)]
    struct MountPoint {
        tag: u32,
        data_len: u16,
        reserved: u16,
        substitute_offset: u16,
        substitute_len: u16,
        print_offset: u16,
        print_len: u16,
        path: [u16; 8184],
    }

    pub(super) fn create_junction(target: &Path, link: &Path) -> io::Result<()> {
        if !matches!(target.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::VerbatimDisk(_)))
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "junction target must be a local disk directory",
            ));
        }
        let mut name: Vec<u16> = target.as_os_str().encode_wide().collect();
        // canonicalize returns \\?\C:\...; the mount point requires \??\C:\...
        name[1] = b'?' as u16;
        if name.len() + 2 > 8184 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "junction target is too long",
            ));
        }
        let total_len = 16 + (name.len() + 2) * 2;
        let mut data = MountPoint {
            tag: 0xA0000003, // IO_REPARSE_TAG_MOUNT_POINT
            data_len: (total_len - 8) as u16,
            reserved: 0,
            substitute_offset: 0,
            substitute_len: (name.len() * 2) as u16,
            print_offset: ((name.len() + 1) * 2) as u16,
            print_len: 0,
            path: [0; 8184],
        };
        data.path[..name.len()].copy_from_slice(&name);
        fs::create_dir(link)?;
        let result = (|| {
            let directory = fs::OpenOptions::new()
                .write(true)
                // Open the reparse point itself, allowing a directory handle.
                .custom_flags(0x00200000 | 0x02000000)
                .open(link)?;
            let mut returned = 0;
            // SAFETY: the live directory handle and initialized, aligned buffer
            // remain valid for this synchronous call; no output buffer is used.
            let success = unsafe {
                DeviceIoControl(
                    directory.as_raw_handle(),
                    0x000900A4,
                    (&data as *const MountPoint).cast(),
                    total_len as u32,
                    std::ptr::null_mut(),
                    0,
                    &mut returned,
                    std::ptr::null_mut(),
                )
            };
            if success == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        })();
        if result.is_err() {
            let _ = fs::remove_dir(link);
        }
        result
    }
}

#[cfg(all(test, any(unix, windows)))]
mod tests {
    use super::*;

    #[test]
    fn link_exposes_canonical_target_and_unlink_preserves_it() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target with spaces & unicode 日本語");
        let link = temp.path().join("linked directory");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("sentinel"), "keep").unwrap();
        create(&target, &link).unwrap();
        assert_eq!(
            fs::canonicalize(&link).unwrap(),
            fs::canonicalize(&target).unwrap()
        );
        assert_eq!(fs::read(link.join("sentinel")).unwrap(), b"keep");
        assert!(create(&target, &link).is_err());
        remove(&link).unwrap();
        assert_eq!(fs::read(target.join("sentinel")).unwrap(), b"keep");
        assert!(remove(&target).is_err());
        create(&target, &link).unwrap();
        fs::remove_dir_all(&target).unwrap();
        remove(&link).unwrap();
        assert!(fs::symlink_metadata(link).is_err());
    }

    #[test]
    fn relative_target_is_anchored_before_linking_from_another_directory() {
        let cwd = std::env::current_dir().unwrap();
        let temp = tempfile::tempdir_in(&cwd).unwrap();
        let target = temp.path().join("install");
        let shadow = temp.path().join("shadow");
        fs::create_dir(&target).unwrap();
        fs::create_dir(&shadow).unwrap();
        fs::write(target.join("sentinel"), "keep").unwrap();
        let relative = target.strip_prefix(&cwd).unwrap();
        assert!(relative.is_relative());
        let link = shadow.join("node_modules");
        create(relative, &link).unwrap();
        assert_eq!(fs::read(link.join("sentinel")).unwrap(), b"keep");
        remove(&link).unwrap();
    }

    #[test]
    fn failed_creation_does_not_leave_an_empty_dependency_root() {
        let temp = tempfile::tempdir().unwrap();
        let link = temp.path().join("link");
        assert!(create(&temp.path().join("missing"), &link).is_err());
        assert!(fs::symlink_metadata(&link).is_err());
        let file = temp.path().join("file");
        fs::write(&file, "keep").unwrap();
        assert!(create(&file, &link).is_err());
        assert!(fs::symlink_metadata(link).is_err());
    }
}
