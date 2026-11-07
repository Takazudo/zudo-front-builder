//! Windows verbatim (`\\?\`) paths at the Rolldown boundary.
//!
//! ZFB canonicalizes its shadow tempdir parent, and on Windows
//! `fs::canonicalize` returns a verbatim path. Esbuild accepts that spelling;
//! Rolldown slash-normalizes it to `//?/C:/…`, where `?` reads as a query
//! separator, and fails with `UNRESOLVED_ENTRY` (#4336, run 39376525150).
//!
//! A verbatim path disables Win32 normalization, so removing the prefix can
//! change which file it names. Only the two spellings that have an equivalent
//! ordinary form are rewritten, and only when every component means the same
//! thing without the prefix (the rules `dunce` applies), including MAX_PATH
//! for the rewritten spelling itself. Anything else fails closed with a
//! diagnostic instead of reaching Rolldown as a broken id. Relative arguments
//! joined onto the rewritten cwd keep their ordinary Win32 meaning.
use anyhow::{bail, Result};
use std::borrow::Cow;

const VERBATIM: &str = r"\\?\";
// MAX_PATH counts the terminating NUL, so an ordinary path holds 259 units.
const MAX_PATH: usize = 260;

/// Return the ordinary spelling of a verbatim disk or UNC path. Paths without
/// the verbatim prefix (drive, relative, ordinary UNC, `\\.\` device) pass
/// through unchanged.
pub(super) fn for_rolldown(path: &str) -> Result<Cow<'_, str>> {
    let Some(rest) = path.strip_prefix(VERBATIM) else {
        return Ok(Cow::Borrowed(path));
    };
    let (ordinary, components) = if let Some(unc) = strip_prefix_ignore_case(rest, r"UNC\") {
        let mut parts = unc.splitn(3, '\\');
        let server = parts.next().unwrap_or_default();
        let share = parts.next().unwrap_or_default();
        if server.is_empty() || share.is_empty() {
            return unsupported(path, "it has no UNC server and share");
        }
        (format!(r"\\{unc}"), unc)
    } else if is_drive_root(rest) {
        (rest.to_string(), &rest[3..])
    } else {
        return unsupported(
            path,
            "it is not a drive or UNC path (for example a volume GUID or device path)",
        );
    };
    // A trailing separator names the same directory; any other empty
    // component is a doubled separator that only verbatim paths keep.
    let components = components.strip_suffix('\\').unwrap_or(components);
    if !components.is_empty() {
        for component in components.split('\\') {
            if let Some(reason) = unsafe_component(component) {
                return unsupported(path, &format!("component {component:?} {reason}"));
            }
        }
    }
    if ordinary.encode_utf16().count() >= MAX_PATH {
        return unsupported(path, "it is longer than MAX_PATH without the prefix");
    }
    Ok(Cow::Owned(ordinary))
}

fn strip_prefix_ignore_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .get(..prefix.len())
        .filter(|head| head.eq_ignore_ascii_case(prefix))
        .map(|_| &value[prefix.len()..])
}

// `\\?\C:` without a separator is the volume itself, not its root directory.
fn is_drive_root(rest: &str) -> bool {
    let bytes = rest.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
}

fn unsafe_component(component: &str) -> Option<&'static str> {
    if component.is_empty() {
        return Some("is empty");
    }
    if component == "." || component == ".." {
        return Some("would be normalized away");
    }
    if component.ends_with(['.', ' ']) {
        return Some("ends with a dot or space, which Win32 trims");
    }
    if component
        .chars()
        .any(|c| c < ' ' || matches!(c, '<' | '>' | ':' | '"' | '/' | '|' | '?' | '*'))
    {
        return Some("contains a character reserved outside verbatim paths");
    }
    let stem = component.split('.').next().unwrap_or_default().trim_end();
    let reserved = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"]
        .iter()
        .any(|name| stem.eq_ignore_ascii_case(name))
        || ["COM", "LPT"].iter().any(|name| {
            strip_prefix_ignore_case(stem, name).is_some_and(|n| {
                matches!(
                    n,
                    "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        });
    reserved.then_some("is a reserved device name")
}

fn unsupported<T>(path: &str, reason: &str) -> Result<T> {
    bail!(
        "Rolldown prototype cannot pass Windows verbatim path {path} to Rolldown: {reason}, \
         so removing the \\\\?\\ prefix could change the file it names. \
         The default esbuild backend accepts this path."
    )
}

#[cfg(test)]
mod tests {
    use super::for_rolldown;

    fn ok(path: &str) -> String {
        for_rolldown(path).unwrap().into_owned()
    }

    fn err(path: &str) -> String {
        format!("{:#}", for_rolldown(path).unwrap_err())
    }

    #[test]
    fn verbatim_disk_path_from_the_windows_failure_becomes_ordinary() {
        let shadow = r"\\?\C:\Users\runneradmin\AppData\Local\Temp\zfb-bundler-cCUG37\entry.mjs";
        let ordinary = ok(shadow);
        assert_eq!(
            ordinary,
            r"C:\Users\runneradmin\AppData\Local\Temp\zfb-bundler-cCUG37\entry.mjs"
        );
        // The spelling Rolldown reported, and the query separator inside it.
        assert!(!ordinary.replace('\\', "/").starts_with("//?/"));
        assert!(!ordinary.contains('?'));
    }

    #[test]
    fn drive_roots_and_trailing_separators_keep_their_directory() {
        assert_eq!(ok(r"\\?\C:\"), r"C:\");
        assert_eq!(ok(r"\\?\d:\work\"), r"d:\work\");
    }

    #[test]
    fn ordinary_paths_pass_through_unchanged() {
        for path in [
            r"C:\Users\runneradmin\AppData\Local\Temp\zfb-bundler-x\entry.mjs",
            r"\\server\share\site\entry.mjs",
            r"\\.\pipe\zfb",
            r"relative\entry.mjs",
            "/tmp/zfb-bundler-x/entry.mjs",
            "",
        ] {
            assert!(matches!(
                for_rolldown(path).unwrap(),
                std::borrow::Cow::Borrowed(p) if p == path
            ));
        }
    }

    #[test]
    fn verbatim_unc_path_keeps_server_and_share() {
        assert_eq!(
            ok(r"\\?\UNC\build-host\share\zfb-bundler-x\entry.mjs"),
            r"\\build-host\share\zfb-bundler-x\entry.mjs"
        );
        assert_eq!(ok(r"\\?\unc\host\share"), r"\\host\share");
        assert!(err(r"\\?\UNC\host").contains("no UNC server and share"));
        assert!(err(r"\\?\UNC\\share\x").contains("no UNC server and share"));
    }

    #[test]
    fn extended_length_paths_that_need_the_prefix_fail_closed() {
        let deep = format!(r"\\?\C:\{}entry.mjs", "segment\\".repeat(40));
        assert!(err(&deep).contains("longer than MAX_PATH"));
        // The 259-unit boundary: one unit shorter is still an ordinary path.
        let fits = format!(r"\\?\C:\{}", "a".repeat(256));
        assert_eq!(ok(&fits).len(), 259);
        let over = format!(r"\\?\C:\{}", "a".repeat(257));
        assert!(err(&over).contains("longer than MAX_PATH"));
    }

    #[test]
    fn components_only_verbatim_paths_can_name_fail_closed() {
        for (path, reason) in [
            (r"\\?\C:\work\..\entry.mjs", "normalized away"),
            (r"\\?\C:\work\.\entry.mjs", "normalized away"),
            (r"\\?\C:\work\\entry.mjs", "is empty"),
            (r"\\?\C:\work.\entry.mjs", "dot or space"),
            (r"\\?\C:\work \entry.mjs", "dot or space"),
            (r"\\?\C:\a/b\entry.mjs", "reserved outside verbatim"),
            (r"\\?\C:\a?url\entry.mjs", "reserved outside verbatim"),
            (r"\\?\C:\a:b\entry.mjs", "reserved outside verbatim"),
            (r"\\?\C:\NUL\entry.mjs", "reserved device name"),
            (r"\\?\C:\work\con.mjs", "reserved device name"),
            (r"\\?\C:\work\Com1 .txt", "reserved device name"),
            (r"\\?\C:\work\lpt9", "reserved device name"),
            (r"\\?\C:\work\COM²", "reserved device name"),
            (r"\\?\C:\work\CONIN$\entry.mjs", "reserved device name"),
            (r"\\?\C:\work\conout$.log", "reserved device name"),
        ] {
            let message = err(path);
            assert!(message.contains(reason), "{path}: {message}");
            assert!(message.contains("default esbuild backend"), "{message}");
        }
        // Names that merely start like a device stay ordinary.
        assert_eq!(
            ok(r"\\?\C:\console\COM10\nul-ish"),
            r"C:\console\COM10\nul-ish"
        );
    }

    #[test]
    fn other_verbatim_namespaces_fail_closed() {
        for path in [
            r"\\?\Volume{0a1b2c3d-0000-0000-0000-000000000000}\entry.mjs",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\entry.mjs",
            r"\\?\C:",
            r"\\?\C:entry.mjs",
            r"\\?\",
        ] {
            assert!(err(path).contains("not a drive or UNC path"), "{path}");
        }
    }
}
