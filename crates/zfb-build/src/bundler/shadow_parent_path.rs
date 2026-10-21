//! Ordinary drive spelling for canonical Windows SSR shadow parents (#4347).
//!
//! Keep paths that require verbatim semantics unchanged. In particular UNC is
//! not a drive path: its ordinary spelling still starts with `//` after POSIX
//! conversion and needs a separate bundler contract. This is not a general
//! Windows path normalizer. Component checks follow the existing #4340 boundary
//! helper; no Rolldown implementation or minification policy is imported here.
use std::path::PathBuf;

pub(super) fn ordinary_shadow_parent(canonical: PathBuf) -> PathBuf {
    #[cfg(windows)]
    if let Some(ordinary) = canonical.to_str().and_then(ordinary_disk_spelling) {
        return PathBuf::from(ordinary);
    }
    canonical
}

#[cfg(windows)]
fn ordinary_disk_spelling(path: &str) -> Option<&str> {
    let ordinary = path.strip_prefix(r"\\?\")?;
    let bytes = ordinary.as_bytes();
    if bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || bytes[2] != b'\\'
        || ordinary.encode_utf16().count() >= 260
    {
        return None;
    }
    let tail = ordinary[3..].strip_suffix('\\').unwrap_or(&ordinary[3..]);
    if !tail.is_empty() && !tail.split('\\').all(ordinary_component) {
        return None;
    }
    Some(ordinary)
}

#[cfg(windows)]
fn ordinary_component(component: &str) -> bool {
    if component.is_empty()
        || component.ends_with(['.', ' '])
        || component
            .chars()
            .any(|c| c < ' ' || matches!(c, '<' | '>' | ':' | '"' | '/' | '|' | '?' | '*'))
    {
        return false;
    }
    let stem = component.split('.').next().unwrap_or_default().trim_end();
    !["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"]
        .iter()
        .any(|name| stem.eq_ignore_ascii_case(name))
        && !["COM", "LPT"].iter().any(|name| {
            stem.get(..3)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(name))
                && matches!(
                    &stem[3..],
                    "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn drive_root_and_unicode_with_spaces_are_ordinary() {
        for (input, expected) in [
            (r"\\?\C:\", r"C:\"),
            (r"\\?\d:\work\", r"d:\work\"),
            (r"\\?\C:\作業 🦀\with spaces", r"C:\作業 🦀\with spaces"),
            (r"\\?\C:\console\COM10\nul-ish", r"C:\console\COM10\nul-ish"),
        ] {
            assert_eq!(
                ordinary_shadow_parent(input.into()),
                PathBuf::from(expected)
            );
        }
    }

    #[test]
    fn ordinary_unc_device_and_other_namespaces_are_preserved() {
        for path in [
            r"C:\work",
            r"\\server\share\work",
            r"\\?\UNC\server\share\work",
            r"\\.\pipe\zfb",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\work",
            r"\\?\Volume{0123}\work",
            r"\\?\C:",
            r"\\?\C:work",
            "/tmp/work",
        ] {
            assert_eq!(ordinary_shadow_parent(path.into()), PathBuf::from(path));
        }
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_only_components_are_preserved() {
        for tail in [
            ".",
            "..",
            "work.",
            "work ",
            "a/b",
            "a?b",
            "a:b",
            "a\0b",
            "a\\\\b",
            "NUL",
            "con.txt",
            "COM1 .txt",
            "COM²",
            "LPT9",
            "CONIN$",
            "conout$.log",
        ] {
            let path = format!(r"\\?\C:\{tail}");
            assert_eq!(
                ordinary_shadow_parent(path.clone().into()),
                PathBuf::from(path)
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn length_boundary_counts_utf16_units() {
        let fits = format!(r"\\?\C:\{}", "a".repeat(256));
        assert_eq!(ordinary_shadow_parent(fits.into()).as_os_str().len(), 259);
        for tail in ["a".repeat(257), "🦀".repeat(129)] {
            let path = format!(r"\\?\C:\{tail}");
            assert_eq!(
                ordinary_shadow_parent(path.clone().into()),
                PathBuf::from(path)
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn unpaired_utf16_is_preserved_without_loss() {
        use std::os::windows::ffi::OsStringExt;
        let mut units: Vec<u16> = r"\\?\C:\".encode_utf16().collect();
        units.push(0xD800);
        let path = PathBuf::from(std::ffi::OsString::from_wide(&units));
        assert_eq!(ordinary_shadow_parent(path.clone()), path);
    }

    #[cfg(windows)]
    #[test]
    fn canonical_drive_directory_keeps_identity_and_child_access() {
        use std::{
            fs,
            path::{Component, Prefix},
            time::{SystemTime, UNIX_EPOCH},
        };
        // Like the bundler regression, this requires a short local-drive TEMP.
        let path = std::env::temp_dir().join(format!(
            "zfb-4347-作業 with spaces-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        let canonical = fs::canonicalize(&path).unwrap();
        let ordinary = ordinary_shadow_parent(canonical.clone());
        assert!(
            matches!(ordinary.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_)))
        );
        assert_eq!(fs::canonicalize(&ordinary).unwrap(), canonical);
        fs::write(ordinary.join("entry.mjs"), "export default 42;").unwrap();
        assert_eq!(
            fs::read_to_string(canonical.join("entry.mjs")).unwrap(),
            "export default 42;"
        );
        fs::remove_file(ordinary.join("entry.mjs")).unwrap();
        fs::remove_dir(ordinary).unwrap();
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_verbatim_looking_names_are_unchanged() {
        let path = PathBuf::from(r"\\?\C:\work");
        assert_eq!(ordinary_shadow_parent(path.clone()), path);
    }
}
