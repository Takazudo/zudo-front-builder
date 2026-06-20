//! Framed user-facing diagnostics.
//!
//! Replaces the `Debug`-printed `anyhow::Error` blobs that used to surface
//! out of the CLI when a user's project hit a structured error. The shape
//! every framed diagnostic shares:
//!
//! ```text
//! error: <short human message>
//!  --> <project-relative path>:<line>:<col>
//!    |
//!  N | <line-2 from source>
//!  N | <offending line>
//!    |     ^
//!  N | <line+1 from source>
//!    |
//! ```
//!
//! The format intentionally mirrors what rustc / clippy / esbuild ship:
//! file:line:col with a short snippet centred on the offending line and a
//! caret pointing at the column. Editors and humans both pick this up.
//!
//! All framed diagnostics share [`Diagnostic`] and [`render_framed`] so the
//! on-screen shape stays bit-identical across error classes — much of the
//! value of this module is one consistent reading experience for every kind of
//! failure the user can hit.

use std::fmt;
use std::path::{Path, PathBuf};

use owo_colors::{OwoColorize, Stream};

/// Anyhow-compatible carrier for a [`Diagnostic`].
///
/// Wrap a built diagnostic in `FramedError(diag)` and convert it via
/// `anyhow::Error::from(...)` (or `?` from a `Result<_, FramedError>`)
/// to preserve the structured frame all the way up to the output formatter,
/// which detects the wrapper and emits the framed snippet block instead of
/// the legacy chain-only shape.
///
/// `Display` produces a single-line summary so the wrapper still looks
/// reasonable when something else logs it before the formatter sees it.
#[derive(Debug)]
pub struct FramedError(pub Diagnostic);

impl fmt::Display for FramedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: {}",
            self.0.file, self.0.line, self.0.col, self.0.message
        )
    }
}

impl std::error::Error for FramedError {}

impl FramedError {
    /// Convenience: wrap a [`Diagnostic`] for return as
    /// `Result<_, anyhow::Error>`.
    pub fn into_anyhow(self) -> anyhow::Error {
        anyhow::Error::new(self)
    }
}

/// One framed diagnostic.
///
/// `file` is rendered as-is; callers should pass a project-relative path
/// when one is available (it shows up directly in the `-->` line).
///
/// `line` and `col` are 1-based — matches editor conventions.
///
/// `source` is the **full** source text of the offending file. The
/// renderer slices a 3-line window centred on `line`. When `source` is
/// `None` (rare, only used as a fallback when the file can't be read at
/// format-time), the frame is rendered without a snippet block.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// Display path of the offending file (project-relative when known).
    pub file: String,
    /// 1-based line of the offending position.
    pub line: usize,
    /// 1-based column of the offending position.
    pub col: usize,
    /// Short human-readable message — single line, no trailing newline.
    pub message: String,
    /// Full source text of `file`, used to render the snippet window.
    pub source: Option<String>,
}

impl Diagnostic {
    /// Construct a diagnostic without source text (snippet block omitted
    /// at render time).
    pub fn new(
        file: impl Into<String>,
        line: usize,
        col: usize,
        message: impl Into<String>,
    ) -> Self {
        Self {
            file: file.into(),
            line: line.max(1),
            col: col.max(1),
            message: message.into(),
            source: None,
        }
    }

    /// Construct a diagnostic and attach the full source text. Calling
    /// this enables the framed snippet block at render time.
    pub fn with_source(
        file: impl Into<String>,
        line: usize,
        col: usize,
        message: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            file: file.into(),
            line: line.max(1),
            col: col.max(1),
            message: message.into(),
            source: Some(source.into()),
        }
    }

    /// Best-effort: read `path` from disk to populate `source`. Silent on
    /// failure — a diagnostic without a snippet still renders, just
    /// without the framed window. Returns `self` for chaining.
    #[must_use]
    pub fn try_attach_source_from_disk(mut self, path: &Path) -> Self {
        if self.source.is_some() {
            return self;
        }
        if let Ok(text) = std::fs::read_to_string(path) {
            self.source = Some(text);
        }
        self
    }
}

/// Render a [`Diagnostic`] as the framed multi-line block documented at
/// the module level.
pub fn render_framed(diag: &Diagnostic) -> String {
    let mut out = String::with_capacity(256);

    let label_error = "error".if_supports_color(Stream::Stderr, |t| t.red().bold().to_string());
    out.push_str(&format!("{label_error}: {}\n", diag.message));

    let arrow = "-->".if_supports_color(Stream::Stderr, |t| t.cyan().to_string());
    out.push_str(&format!(
        " {arrow} {}:{}:{}\n",
        diag.file, diag.line, diag.col
    ));

    if let Some(src) = &diag.source {
        // `split('\n')` on a newline-terminated file produces a trailing empty
        // segment (e.g. `"a\nb\n".split('\n')` → `["a", "b", ""]`). Drop it
        // so window arithmetic counts real lines only — without this, the
        // after-context line at EOF can be the empty trailing segment, which
        // either renders as a blank line or is silently skipped.
        let raw: Vec<&str> = src.split('\n').collect();
        let lines: Vec<&str> = match raw.last() {
            Some(&"") => &raw[..raw.len() - 1],
            _ => &raw[..],
        }
        .to_vec();
        if lines.is_empty() {
            return out;
        }

        // 1-based line index. Window is [line-1, line+1] clamped.
        let zero_based = diag.line.saturating_sub(1);
        // Short-circuit to header-only form when the reported line is past the
        // end of the source (e.g. stale/synthetic diagnostics). Emitting the
        // header is still truthful; emitting a snippet would be misleading.
        if zero_based >= lines.len() {
            return out;
        }
        let start = zero_based.saturating_sub(1);
        let end = (zero_based + 1).min(lines.len().saturating_sub(1));

        // Width of the largest line number in the window (pad the gutter
        // so all `N | ` prefixes line up).
        let max_line_no = end + 1;
        let gutter_width = max_line_no.to_string().len();

        let bar = "|".if_supports_color(Stream::Stderr, |t| t.cyan().to_string());

        // Top spacer: `   |`
        out.push_str(&format!("{:>w$} {bar}\n", "", w = gutter_width));

        for idx in start..=end {
            let Some(text) = lines.get(idx) else {
                continue;
            };
            let n = idx + 1;
            // Strip a single trailing CR for nicer Windows output.
            let trimmed = text.strip_suffix('\r').unwrap_or(text);
            let n_text = format!("{:>w$}", n, w = gutter_width);
            let n_styled = n_text.if_supports_color(Stream::Stderr, |t| t.cyan().to_string());
            out.push_str(&format!("{n_styled} {bar} {trimmed}\n"));
            if idx == zero_based {
                // Caret line. Render a single `^` under `col` (1-based).
                let caret_offset = diag.col.saturating_sub(1);
                let pad = " ".repeat(caret_offset);
                let caret = "^".if_supports_color(Stream::Stderr, |t| t.red().bold().to_string());
                out.push_str(&format!(
                    "{:>w$} {bar} {pad}{caret}\n",
                    "",
                    w = gutter_width
                ));
            }
        }

        // Bottom spacer.
        out.push_str(&format!("{:>w$} {bar}\n", "", w = gutter_width));
    }

    out
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Render `path` relative to `project_root` if it is a child; otherwise
/// fall back to the absolute display form. Always emits `/`-separated
/// segments so output is stable across platforms (matches the convention
/// used by `zfb_content::content_bridge`).
pub fn project_relative(path: &Path, project_root: Option<&Path>) -> String {
    let candidate = match project_root {
        Some(root) => path.strip_prefix(root).unwrap_or(path),
        None => path,
    };
    candidate
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str(),
            std::path::Component::RootDir => Some("/"),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
        .replace("//", "/")
}

/// Build a [`Diagnostic`] for a JS runtime error reported by the SSR
/// worker. `bundled_line` / `bundled_col` come from the JS engine's
/// stack frame inside the bundle. If a sourcemap decoder is supplied we
/// decode back to the original `.tsx` location via the closure; otherwise
/// we frame against the bundle itself.
///
/// The `decode_position` parameter is a closure that, given a sourcemap
/// JSON string, a line, and a column, returns `Option<DecodedPosition>`.
/// Callers from `zfb` pass `zfb_render::sourcemap::decode_position` here;
/// other callers (tests, other crates) can supply a stub.
// All 8 params carry independent caller-owned data; a builder struct would add
// boilerplate with no clarity gain.
#[allow(clippy::too_many_arguments)]
pub fn from_js_runtime_error_with_decoder<F, D>(
    bundle_path: &Path,
    bundle_source: &str,
    bundled_line: usize,
    bundled_col: usize,
    message: &str,
    sourcemap_json: Option<&str>,
    project_root: Option<&Path>,
    decode_fn: F,
) -> Diagnostic
where
    F: Fn(&str, usize, usize) -> Option<D>,
    D: DecodedPosition,
{
    if let Some(map) = sourcemap_json {
        if let Some(decoded) = decode_fn(map, bundled_line, bundled_col) {
            let raw = PathBuf::from(decoded.file());
            let abs = match project_root {
                Some(root) if raw.is_relative() => {
                    let bundle_dir = bundle_path.parent().unwrap_or(Path::new(""));
                    let candidate = bundle_dir.join(&raw);
                    candidate
                        .canonicalize()
                        .unwrap_or(candidate)
                        .strip_prefix(root)
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|_| raw.clone())
                }
                _ => raw.clone(),
            };
            let display_path = project_relative(&abs, project_root);
            let mut diag = Diagnostic::new(display_path, decoded.line(), decoded.col(), message);
            if let Some(content) = decoded.source_content() {
                diag.source = Some(content);
            } else {
                let candidate = match project_root {
                    Some(root) => root.join(&raw),
                    None => raw.clone(),
                };
                if let Ok(text) = std::fs::read_to_string(&candidate) {
                    diag.source = Some(text);
                }
            }
            return diag;
        }
    }
    Diagnostic::with_source(
        project_relative(bundle_path, None),
        bundled_line,
        bundled_col,
        format!("JS runtime error (sourcemap unavailable): {message}"),
        bundle_source,
    )
}

/// Trait for decoded source-map positions. Lets callers inject their own
/// decoded-position type without this crate depending on `zfb-render`.
pub trait DecodedPosition {
    /// Path of the original source file (e.g. `src/pages/index.tsx`).
    fn file(&self) -> &str;
    /// 1-based line in the original source.
    fn line(&self) -> usize;
    /// 1-based column in the original source.
    fn col(&self) -> usize;
    /// Optional source content embedded in the sourcemap.
    fn source_content(&self) -> Option<String>;
}

// ---------------------------------------------------------------------------
// Internal helpers (pub for use from zfb crate)
// ---------------------------------------------------------------------------

/// Locate the first `export ... <ident>` occurrence in `source` and
/// return a 1-based `(line, col)` of the `ident`.
///
/// Two forms are handled:
///
/// 1. **Declaration prefix** — lines whose (trimmed) start matches
///    `export const `, `export let `, `export var `, `export function `,
///    `export async function `, or `export class `. The identifier
///    immediately following the keyword prefix is compared to `ident`.
///
/// 2. **Named re-export** — lines whose (trimmed) start matches
///    `export {` are scanned for `ident` as a member name. Only the
///    unaliased form (`export { ident }` or `export { ident, ... }`) is
///    found; aliased members (`export { orig as ident }`) are not matched
///    because their binding name differs from the declared source name
///    that `ident` refers to.
///
/// **Known limitations** (document rather than paper over):
/// - Indented exports (`  export const foo`) are supported (leading
///   whitespace is stripped before matching).
/// - Multi-line `export { ... }` blocks (opening brace on one line,
///   members on subsequent lines) are **not** walked — only single-line
///   `export { ... }` forms are detected.
/// - `export default function` / `export default class` without an
///   explicit name (anonymous default) cannot be located by `ident` and
///   will not produce a match.
/// - `export * from` re-exports are not matched (no local name).
pub fn locate_export_ident(source: &str, ident: &str) -> Option<(usize, usize)> {
    let prefixes = [
        "export const ",
        "export let ",
        "export var ",
        "export function ",
        "export async function ",
        "export class ",
        "export default function ",
        "export default async function ",
        "export default class ",
    ];
    for (line_idx, line) in source.lines().enumerate() {
        let trimmed_start = line.trim_start();
        let leading_ws = line.len() - trimmed_start.len();

        // Form 1: declaration keyword prefix.
        for prefix in prefixes {
            if let Some(rest) = trimmed_start.strip_prefix(prefix) {
                // The next token (up to whitespace, `(`, `=`, or `<`) is the
                // declared identifier.
                let end = rest
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(rest.len());
                let declared = &rest[..end];
                if declared == ident {
                    let col = leading_ws + prefix.len() + 1; // 1-based to ident start
                    return Some((line_idx + 1, col));
                }
            }
        }

        // Form 2: named re-export `export { a, b, c }` on a single line.
        // Scan the member list for `ident` as an unaliased name.
        if let Some(rest) = trimmed_start.strip_prefix("export {") {
            // Find the closing `}` — only handles single-line forms.
            if let Some(close) = rest.find('}') {
                let members = &rest[..close];
                for raw_member in members.split(',') {
                    let member = raw_member.trim();
                    // Skip aliased members (`orig as alias`); only the
                    // unaliased name (or the original name in `orig as alias`)
                    // matches the declared source binding.
                    let name = if let Some((orig, _alias)) = member.split_once(" as ") {
                        orig.trim()
                    } else {
                        member
                    };
                    if name == ident {
                        // Locate the exact byte offset of `ident` in the
                        // original (non-trimmed) line for the column.
                        if let Some(offset) = line.find(ident) {
                            return Some((line_idx + 1, offset + 1)); // 1-based
                        }
                    }
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Strip ANSI escape sequences so assertions are colour-agnostic.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' && chars.peek() == Some(&'[') {
                chars.next();
                for inner in chars.by_ref() {
                    if inner == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn render_framed_includes_path_line_col_and_caret() {
        owo_colors::set_override(false);
        let src = "line 1\nline 2 has the bug\nline 3\nline 4\n";
        let diag = Diagnostic::with_source("project/file.md", 2, 8, "kaboom", src);
        let out = strip_ansi(&render_framed(&diag));

        assert!(out.starts_with("error: kaboom\n"), "got:\n{out}");
        assert!(out.contains(" --> project/file.md:2:8\n"), "got:\n{out}");
        // The caret pad is 7 spaces (col-1) followed by `^`.
        assert!(out.contains("\n  |        ^\n"), "got:\n{out}");
        // Both surrounding lines appear.
        assert!(out.contains("1 | line 1\n"), "got:\n{out}");
        assert!(out.contains("2 | line 2 has the bug\n"), "got:\n{out}");
        assert!(out.contains("3 | line 3\n"), "got:\n{out}");
    }

    #[test]
    fn render_framed_without_source_emits_header_only() {
        owo_colors::set_override(false);
        let diag = Diagnostic::new("file.tsx", 4, 2, "kapow");
        let out = strip_ansi(&render_framed(&diag));
        assert!(out.starts_with("error: kapow\n"), "got:\n{out}");
        assert!(out.contains(" --> file.tsx:4:2\n"), "got:\n{out}");
        // No snippet block.
        assert!(!out.contains("|"), "got:\n{out}");
    }

    #[test]
    fn render_framed_clamps_window_at_file_start() {
        owo_colors::set_override(false);
        let src = "first\nsecond\nthird\n";
        let diag = Diagnostic::with_source("a.md", 1, 1, "early", src);
        let out = strip_ansi(&render_framed(&diag));
        // Should still render, with the bug on line 1, no preceding line.
        assert!(out.contains("1 | first\n"), "got:\n{out}");
        assert!(out.contains("2 | second\n"), "got:\n{out}");
    }

    #[test]
    fn render_framed_header_only_when_line_past_end() {
        // After dropping the trailing empty segment, "first\nsecond\nthird\n"
        // yields 3 real lines. line=100 → zero_based=99 ≥ 3 → short-circuits
        // to header-only form.
        owo_colors::set_override(false);
        let src = "first\nsecond\nthird\n";
        let diag = Diagnostic::with_source("a.md", 100, 1, "boom", src);
        let out = strip_ansi(&render_framed(&diag));
        assert!(out.starts_with("error: boom\n"), "got:\n{out}");
        assert!(out.contains(" --> a.md:100:1\n"), "got:\n{out}");
        // No snippet block emitted — no bar character.
        assert!(!out.contains('|'), "got:\n{out}");
    }

    #[test]
    fn render_framed_header_only_when_line_one_past_end() {
        // After dropping the trailing empty segment, the 3-line source has
        // lines.len() == 3. line=4 → zero_based=3 ≥ 3 → short-circuits.
        // line=5 also short-circuits for the same reason.
        owo_colors::set_override(false);
        let src = "first\nsecond\nthird\n";
        let diag = Diagnostic::with_source("a.md", 5, 1, "boom", src);
        let out = strip_ansi(&render_framed(&diag));
        assert!(out.starts_with("error: boom\n"), "got:\n{out}");
        assert!(out.contains(" --> a.md:5:1\n"), "got:\n{out}");
        // No snippet block emitted.
        assert!(!out.contains('|'), "got:\n{out}");
    }

    #[test]
    fn render_framed_last_line_no_blank_after_context() {
        // Regression: before fix, `split('\n')` on a newline-terminated source
        // produced a trailing empty segment, so the window's after-context
        // line was that empty segment — rendered as a blank `N |` line at EOF.
        // After the fix the trailing empty segment is dropped and `end` clamps
        // to the real last line, so the after-context is omitted cleanly.
        owo_colors::set_override(false);
        let src = "first\nsecond\nthird\n";
        // Report an error on the last real line (line 3).
        let diag = Diagnostic::with_source("a.md", 3, 1, "boom", src);
        let out = strip_ansi(&render_framed(&diag));
        // The error line itself must be present.
        assert!(
            out.contains("3 | third\n"),
            "expected last line in output: {out:?}"
        );
        // There must be no blank-content line in the snippet window
        // (a trailing-segment blank would appear as "4 | \n" or similar).
        assert!(
            !out.contains("4 |"),
            "unexpected blank after-context line: {out:?}"
        );
    }

    #[test]
    fn project_relative_strips_prefix_when_child() {
        let root = Path::new("/project");
        let file = Path::new("/project/pages/index.tsx");
        assert_eq!(project_relative(file, Some(root)), "pages/index.tsx");
    }

    #[test]
    fn project_relative_falls_back_when_not_child() {
        let root = Path::new("/other");
        let file = Path::new("/project/pages/index.tsx");
        // strip_prefix fails; falls back to absolute
        let result = project_relative(file, Some(root));
        assert!(result.contains("project"), "got: {result}");
    }

    /// A minimal [`DecodedPosition`] stub for tests.
    struct FakeDecoded {
        file: &'static str,
        line: usize,
        col: usize,
        source_content: Option<String>,
    }
    impl DecodedPosition for FakeDecoded {
        fn file(&self) -> &str {
            self.file
        }
        fn line(&self) -> usize {
            self.line
        }
        fn col(&self) -> usize {
            self.col
        }
        fn source_content(&self) -> Option<String> {
            self.source_content.clone()
        }
    }

    #[test]
    fn absolute_sourcemap_path_is_stripped_against_project_root() {
        // esbuild can emit absolute paths in `sources`; previously the call
        // site passed `None` so project_root was ignored and the full
        // on-disk path leaked into the diagnostic.  After the fix,
        // project_root is forwarded and the path is made project-relative.
        let project_root = Path::new("/home/alice/proj");
        let bundle_path = Path::new("/home/alice/proj/.zfb/bundle.js");

        let diag = from_js_runtime_error_with_decoder(
            bundle_path,
            "// bundle",
            1,
            1,
            "oops",
            Some("ignored"),
            Some(project_root),
            |_map, _line, _col| {
                Some(FakeDecoded {
                    file: "/home/alice/proj/src/pages/index.tsx",
                    line: 3,
                    col: 5,
                    source_content: Some("// src".into()),
                })
            },
        );

        // Must be project-relative, not the full on-disk absolute path.
        assert_eq!(
            diag.file, "src/pages/index.tsx",
            "expected project-relative path, got: {}",
            diag.file
        );
    }

    #[test]
    fn locate_export_ident_finds_export_function() {
        let src = "import x from 'y';\n\nexport function paths() {\n}\n";
        let found = locate_export_ident(src, "paths");
        assert!(found.is_some());
        let (line, _col) = found.unwrap();
        assert_eq!(line, 3);
    }

    #[test]
    fn locate_export_ident_returns_none_when_absent() {
        let src = "export const foo = 1;\n";
        assert!(locate_export_ident(src, "bar").is_none());
    }

    #[test]
    fn locate_export_ident_finds_named_re_export_single_member() {
        // `export { paths }` — the identifier is the only member.
        let src = "const paths = [];\nexport { paths };\n";
        let found = locate_export_ident(src, "paths");
        assert!(found.is_some(), "expected a match for export {{ paths }}");
        let (line, _col) = found.unwrap();
        assert_eq!(line, 2);
    }

    #[test]
    fn locate_export_ident_finds_named_re_export_multi_member() {
        // `export { foo, paths, bar }` — target is not the first member.
        let src = "export { foo, paths, bar };\n";
        let found = locate_export_ident(src, "paths");
        assert!(
            found.is_some(),
            "expected a match for paths in multi-member export"
        );
        let (line, col) = found.unwrap();
        assert_eq!(line, 1);
        // "paths" starts at byte offset 14 (0-based) → col 15 (1-based).
        assert_eq!(
            col, 15,
            "unexpected column for `paths` in `export {{ foo, paths, bar }}`"
        );
    }

    #[test]
    fn locate_export_ident_named_re_export_does_not_match_alias_target() {
        // `export { orig as paths }` — `paths` is the alias, not the source
        // binding. The function should not match aliased exports by alias name.
        let src = "export { orig as paths };\n";
        // `paths` is the alias; the source binding is `orig`. Neither should
        // match when searching for `paths` (alias target lookup is out of scope).
        let found = locate_export_ident(src, "paths");
        assert!(
            found.is_none(),
            "aliased export target `paths` must not match (alias lookup is not supported)"
        );
    }
}
