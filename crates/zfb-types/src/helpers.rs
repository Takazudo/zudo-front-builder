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
            // U+2028 LINE SEPARATOR and U+2029 PARAGRAPH SEPARATOR are
            // valid JSON string content per the spec but terminate JS
            // string literals in older environments; escape them so the
            // output is safe to embed in inline <script> tags.
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
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
/// On Unix backslash is a valid filename character, not a path separator, so
/// the replacement is gated behind `cfg!(windows)`. On Windows the resulting
/// string uses forward slashes as separators, which is what tools like esbuild
/// and globset expect regardless of the host OS.
pub fn path_to_posix_string(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    if cfg!(windows) && s.contains('\\') {
        s.replace('\\', "/")
    } else {
        s
    }
}

// ── node_modules path detection ───────────────────────────────────────────────

/// True when any path component is literally `node_modules` — i.e. the path
/// was reached by resolving a bare package specifier through some install
/// root, rather than a relative/project-rooted import.
///
/// This is the CANONICAL home for that predicate (issue #2051). The durable
/// rule, which does not go stale: **delegate to it — with a local alias when a
/// shorter name reads better in context, as `zfb-build`'s
/// `bundler::path_is_inside_node_modules` does — and never re-inline the
/// component walk.** It is trivial enough to retype from memory, which is
/// exactly why copies keep appearing, and each one is a place a future
/// refinement (UNC prefixes, case-insensitive volumes, a nested-install
/// carve-out) would silently miss.
///
/// Migration is still NOT complete, and this doc deliberately does not claim
/// otherwise (issue #2128: it went stale twice already — once when the earlier
/// doc enumerated #2051's two sites as "the" implementation and a third turned
/// up in `zfb-build`'s `bundler.rs`, and again when #2128 itself assumed the
/// three copies it named were the last ones). As of #2128's fix, every site in
/// `zfb-build` (`bundler.rs`, `module_worker.rs`) and `zfb-islands`
/// (`scanner.rs` — all four of its component-walk sites, including two the
/// #2128 audit missed: `is_workspace_package` and
/// `is_within_injected_route_closure`) delegates. One hand-written boolean
/// copy is known to remain: `crates/zfb/src/commands/build.rs`'s
/// `usable_rel`. It was left alone deliberately — folding it in means editing
/// and rebuilding the `zfb` crate (the one that embeds V8; a 15–30 min
/// compile), out of scope for the `zfb-build`/`zfb-islands`-only pass that
/// produced this doc update — not because it's exempt from the rule above.
/// Don't trust this paragraph either: re-run the audit and read every hit
/// yourself, since index/position-based lookups (e.g. `rposition` to find the
/// *last* `node_modules` segment for path-splitting, or a `.next()` check for
/// whether the *first* component is `node_modules`) are a different question
/// and legitimately don't delegate here:
///
/// ```sh
/// grep -rn -A2 'components()' crates/ --include='*.rs' | grep node_modules
/// ```
pub fn has_node_modules_segment(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c, Component::Normal(name) if name == "node_modules"))
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

    #[test]
    fn json_string_escapes_line_and_paragraph_separators() {
        // U+2028 LINE SEPARATOR and U+2029 PARAGRAPH SEPARATOR must be
        // escaped so the output is safe to embed in inline <script> tags
        // (they terminate JS string literals in older JS engines even
        // though they are valid JSON string content).
        assert_eq!(json_string("\u{2028}"), "\"\\u2028\"");
        assert_eq!(json_string("\u{2029}"), "\"\\u2029\"");
        assert_eq!(json_string("a\u{2028}b\u{2029}c"), "\"a\\u2028b\\u2029c\"");
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
        assert_eq!(escape_html("& < > \" '"), "&amp; &lt; &gt; &quot; &#39;");
    }

    // ── path_to_posix_string ──────────────────────────────────────────────────

    #[test]
    fn path_to_posix_unix_path() {
        assert_eq!(
            path_to_posix_string(Path::new("/abs/foo.tsx")),
            "/abs/foo.tsx"
        );
    }

    #[test]
    fn path_to_posix_backslash_behaviour_is_platform_gated() {
        // On Windows backslash is the path separator and must be converted.
        // On Unix backslash is a valid filename byte; we must NOT corrupt it.
        let input = Path::new(r"C:\abs\foo.tsx");
        let result = path_to_posix_string(input);
        if cfg!(windows) {
            assert_eq!(result, "C:/abs/foo.tsx", "Windows: backslashes converted");
        } else {
            // The literal string r"C:\abs\foo.tsx" contains backslashes that
            // on Unix are valid filename characters — they must be preserved.
            assert_eq!(result, r"C:\abs\foo.tsx", "Unix: backslashes preserved");
        }
    }

    #[test]
    fn path_to_posix_relative() {
        assert_eq!(
            path_to_posix_string(Path::new("sub/dir/file.ts")),
            "sub/dir/file.ts"
        );
    }

    // ── has_node_modules_segment ──────────────────────────────────────────────

    #[test]
    fn has_node_modules_segment_detects_middle_component() {
        assert!(has_node_modules_segment(Path::new(
            "/proj/node_modules/@acme/ui/src/index.ts"
        )));
    }

    #[test]
    fn has_node_modules_segment_absent_for_ordinary_source_path() {
        assert!(!has_node_modules_segment(Path::new(
            "/proj/pages/blog/index.tsx"
        )));
    }

    #[test]
    fn has_node_modules_segment_requires_exact_component_match() {
        // A directory merely containing "node_modules" as a substring of a
        // longer name must not match — only a literal path component does.
        assert!(!has_node_modules_segment(Path::new(
            "/proj/not_node_modules_really/file.ts"
        )));
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
