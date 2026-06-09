//! Shared string/path helper functions.
//!
//! These utilities are duplicated across several crates and live here to ensure
//! a single canonical implementation that all call sites agree on.

use std::path::{Component, Path, PathBuf};

// ── JSON string escaping ──────────────────────────────────────────────────────

/// Encode `s` as a JSON string literal (with surrounding double quotes).
///
/// Escapes `"`, `\`, and ASCII control characters (`< 0x20`) using standard
/// JSON escape sequences. Used to splice user-supplied strings into generated
/// JS/TS source without risking syntax errors from stray quotes, backslashes,
/// or control characters.
pub fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ── HTML escaping ─────────────────────────────────────────────────────────────

/// Escape HTML special characters in `s`.
///
/// Replaces `&`, `<`, `>`, `"`, and `'` with their named/numeric HTML
/// entity equivalents. Safe for use in both HTML element content and attribute
/// values.
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

// ── POSIX path conversion ─────────────────────────────────────────────────────

/// Convert a `&Path` to a POSIX-style string by replacing `\` with `/`.
///
/// On Unix this is a no-op (paths never contain backslashes). On Windows the
/// resulting string uses forward slashes as separators, which is what tools
/// like esbuild and globset expect regardless of the host OS.
pub fn path_to_posix_string(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    if s.contains('\\') {
        s.replace('\\', "/")
    } else {
        s
    }
}

// ── Lexical path normalization ────────────────────────────────────────────────

/// Lexically normalise a path: collapse `.` and `..` segments without
/// touching the filesystem.
///
/// Rules:
/// - `.` components are dropped.
/// - `..` pops the last [`Component::Normal`] segment if one is present;
///   otherwise the `..` is kept (so leading `../` in relative paths is
///   preserved and `..` past a root/prefix is also preserved).
/// - All other components (prefix, root, normal names) are passed through.
pub fn normalize_path_lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => {
                out.push(comp.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                let last_is_normal = out
                    .components()
                    .next_back()
                    .map(|c| matches!(c, Component::Normal(_)))
                    .unwrap_or(false);
                if last_is_normal {
                    out.pop();
                } else {
                    out.push(comp.as_os_str());
                }
            }
            Component::Normal(name) => out.push(name),
        }
    }
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── json_string ───────────────────────────────────────────────────────────

    #[test]
    fn json_string_plain() {
        assert_eq!(json_string("hello"), "\"hello\"");
    }

    #[test]
    fn json_string_escapes_specials() {
        assert_eq!(json_string("a/b"), "\"a/b\"");
        assert_eq!(json_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(json_string("a\\b"), "\"a\\\\b\"");
        assert_eq!(json_string("a\nb"), "\"a\\nb\"");
        assert_eq!(json_string("a\rb"), "\"a\\rb\"");
        assert_eq!(json_string("a\tb"), "\"a\\tb\"");
    }

    #[test]
    fn json_string_control_char() {
        // BEL (0x07) must be encoded as 
        assert_eq!(json_string("\x07"), "\"\\u0007\"");
    }

    #[test]
    fn json_string_unicode_passthrough() {
        // Non-ASCII printable characters should pass through unchanged.
        assert_eq!(json_string("日本語"), "\"日本語\"");
    }

    // ── escape_html ───────────────────────────────────────────────────────────

    #[test]
    fn escape_html_basic() {
        assert_eq!(escape_html("a & b"), "a &amp; b");
        assert_eq!(escape_html("<em>"), "&lt;em&gt;");
        assert_eq!(escape_html("\"q\""), "&quot;q&quot;");
        assert_eq!(escape_html("plain"), "plain");
    }

    #[test]
    fn escape_html_single_quote() {
        assert_eq!(escape_html("it's"), "it&#39;s");
    }

    #[test]
    fn escape_html_all_specials() {
        assert_eq!(
            escape_html("& < > \" '"),
            "&amp; &lt; &gt; &quot; &#39;"
        );
    }

    // ── path_to_posix_string ──────────────────────────────────────────────────

    #[test]
    fn path_to_posix_unix_path() {
        assert_eq!(path_to_posix_string(Path::new("/abs/foo.tsx")), "/abs/foo.tsx");
    }

    #[test]
    fn path_to_posix_replaces_backslashes() {
        // On Unix Path does not interpret `\` as a separator, but
        // to_string_lossy still returns the literal characters, which our
        // replace then maps to forward slashes — simulating a Windows path.
        assert_eq!(
            path_to_posix_string(Path::new(r"C:\abs\foo.tsx")),
            "C:/abs/foo.tsx"
        );
    }

    #[test]
    fn path_to_posix_relative() {
        assert_eq!(path_to_posix_string(Path::new("sub/dir/file.ts")), "sub/dir/file.ts");
    }

    // ── normalize_path_lexical ────────────────────────────────────────────────

    #[test]
    fn normalize_path_lexical_collapses_dot_dot() {
        assert_eq!(
            normalize_path_lexical(&PathBuf::from("/a/b/../c")),
            PathBuf::from("/a/c")
        );
    }

    #[test]
    fn normalize_path_lexical_collapses_dot() {
        assert_eq!(
            normalize_path_lexical(&PathBuf::from("/a/./b")),
            PathBuf::from("/a/b")
        );
    }

    #[test]
    fn normalize_path_lexical_double_parent() {
        assert_eq!(
            normalize_path_lexical(&PathBuf::from("/a/b/c/../../d")),
            PathBuf::from("/a/d")
        );
    }

    #[test]
    fn normalize_path_lexical_relative_leading_dotdot() {
        // Leading `../` in a relative path must be preserved.
        assert_eq!(
            normalize_path_lexical(&PathBuf::from("../../foo")),
            PathBuf::from("../../foo")
        );
    }

    #[test]
    fn normalize_path_lexical_resolves_relative() {
        assert_eq!(
            normalize_path_lexical(&PathBuf::from("/proj/pages/blog/../shared.tsx")),
            PathBuf::from("/proj/pages/shared.tsx")
        );
    }
}
