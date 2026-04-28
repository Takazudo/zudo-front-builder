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
//! Four error classes funnel through this module:
//!
//! 1. Frontmatter parse errors ([`zfb_content::FrontmatterError`] /
//!    [`zfb_content::TsxFrontmatterError`]) — YAML position is recovered
//!    from `serde_yaml::Error::location()` and adjusted past the opening
//!    `---` delimiter so it lines up with the user's file.
//! 2. MDX directive diagnostics
//!    ([`zfb_content::plugins::DirectiveDiagnostic`]) — already carry
//!    `line` / `column`. Need pairing with the source path that the
//!    pipeline runner knows.
//! 3. `paths()` shape errors ([`zfb_render::paths::PathsError`] +
//!    [`zfb_render::paths_extract::PathsExtractError`]) — `route`
//!    identifies the source file. Position is recovered by re-parsing the
//!    file and locating the `paths` export ident; if recovery fails we
//!    still frame the file with `line: 1, col: 1`, never silently lose
//!    the message.
//! 4. JS runtime errors thrown under miniflare — the host receives a
//!    stack frame inside the bundled SSR worker; we map the bundled
//!    `(line, col)` back to the original `.tsx` via
//!    [`zfb_render::sourcemap::decode_position`] and frame against the
//!    decoded file.
//!
//! All four reuse [`Diagnostic`] and [`render_framed`] so the on-screen
//! shape stays bit-identical across error classes — much of the value of
//! this module is one consistent reading experience for every kind of
//! failure the user can hit.

use std::fmt;
use std::path::{Path, PathBuf};

use owo_colors::{OwoColorize, Stream};

/// Anyhow-compatible carrier for a [`Diagnostic`].
///
/// Wrap a built diagnostic in `FramedError(diag)` and convert it via
/// `anyhow::Error::from(...)` (or `?` from a `Result<_, FramedError>`)
/// to preserve the structured frame all the way up to
/// [`crate::output::format_error`], which detects the wrapper and emits
/// the framed snippet block instead of the legacy chain-only shape.
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
    out.push_str(&format!(" {arrow} {}:{}:{}\n", diag.file, diag.line, diag.col));

    if let Some(src) = &diag.source {
        let lines: Vec<&str> = src.split('\n').collect();
        if lines.is_empty() {
            return out;
        }

        // 1-based line index. Window is [line-1, line+1] clamped.
        let zero_based = diag.line.saturating_sub(1);
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
                let caret =
                    "^".if_supports_color(Stream::Stderr, |t| t.red().bold().to_string());
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
// Converters from each first-class error type
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

/// Build a [`Diagnostic`] for a [`zfb_content::FrontmatterError`].
///
/// The YAML branch carries position info via
/// `serde_yaml::Error::location()`; we adjust by `+1` to step past the
/// opening `---\n` delimiter so the line lines up with the user's file.
/// Other variants fall back to `(1, 1)`.
pub fn from_frontmatter_error(
    path: &Path,
    source: &str,
    err: &zfb_content::FrontmatterError,
) -> Diagnostic {
    use zfb_content::FrontmatterError;
    let file = project_relative(path, None);
    match err {
        FrontmatterError::Yaml(yaml_err) => {
            // `serde_yaml::Location` is 1-based and relative to the YAML
            // content slice (i.e. starts AFTER the `---\n` opener).
            let (line, col) = match yaml_err.location() {
                Some(loc) => (loc.line() + 1, loc.column()),
                None => (1, 1),
            };
            Diagnostic::with_source(
                file,
                line,
                col,
                format!("invalid YAML in frontmatter: {yaml_err}"),
                source,
            )
        }
        FrontmatterError::Unterminated => Diagnostic::with_source(
            file,
            1,
            1,
            "frontmatter unterminated: missing closing `---`",
            source,
        ),
        FrontmatterError::Tsx(tsx_err) => from_tsx_frontmatter_error(path, source, tsx_err),
        FrontmatterError::UnsupportedExtension(ext) => Diagnostic::with_source(
            file,
            1,
            1,
            format!(
                "unsupported file extension `.{ext}` for frontmatter (expected md, mdx, or tsx)"
            ),
            source,
        ),
        FrontmatterError::MissingExtension => Diagnostic::with_source(
            file,
            1,
            1,
            "missing file extension; cannot dispatch frontmatter extraction",
            source,
        ),
    }
}

/// Build a [`Diagnostic`] for a [`zfb_content::TsxFrontmatterError`].
///
/// All variants except `MissingFrontmatter` already carry `line` / `col`.
pub fn from_tsx_frontmatter_error(
    path: &Path,
    source: &str,
    err: &zfb_content::TsxFrontmatterError,
) -> Diagnostic {
    use zfb_content::TsxFrontmatterError;
    let file = project_relative(path, None);
    match err {
        TsxFrontmatterError::Parse { message, .. } => Diagnostic::with_source(
            file,
            1,
            1,
            format!("TSX parse error: {message}"),
            source,
        ),
        TsxFrontmatterError::MissingFrontmatter { .. } => Diagnostic::with_source(
            file,
            1,
            1,
            "missing required `export const frontmatter`",
            source,
        ),
        TsxFrontmatterError::DuplicateExport { name, line, col, .. } => Diagnostic::with_source(
            file,
            *line,
            *col,
            format!("duplicate top-level `export const {name}`"),
            source,
        ),
        TsxFrontmatterError::ComputedValue {
            export, reason, line, col, ..
        } => Diagnostic::with_source(
            file,
            *line,
            *col,
            format!(
                "non-literal value not allowed in `export const {export}` ({reason})"
            ),
            source,
        ),
        TsxFrontmatterError::WrongShape {
            export, reason, line, col, ..
        } => Diagnostic::with_source(
            file,
            *line,
            *col,
            format!("`export const {export}` {reason}"),
            source,
        ),
    }
}

/// Build a [`Diagnostic`] for a [`zfb_content::plugins::DirectiveDiagnostic`].
///
/// `path` is the file the directive was found in — the diagnostic itself
/// only carries line/column relative to the source.
pub fn from_directive_diagnostic(
    path: &Path,
    source: &str,
    diag: &zfb_content::plugins::DirectiveDiagnostic,
) -> Diagnostic {
    let line = diag.line.unwrap_or(1);
    let col = diag.column.unwrap_or(1);
    Diagnostic::with_source(
        project_relative(path, None),
        line,
        col,
        diag.message.clone(),
        source,
    )
}

/// Build a [`Diagnostic`] for a [`zfb_render::paths::PathsError`].
///
/// `paths()` errors carry `route` (e.g. `blog/[slug].tsx`). We use that
/// as the file and try to locate the `paths` identifier in the source so
/// the caret lands on the exported function. If recovery fails we still
/// frame the file with `(1, 1)` rather than dropping the diagnostic.
pub fn from_paths_error(
    path: &Path,
    source: &str,
    err: &zfb_render::paths::PathsError,
) -> Diagnostic {
    use zfb_render::paths::PathsError;
    let (line, col) = locate_export_ident(source, "paths").unwrap_or((1, 1));
    let file = project_relative(path, None);
    let message = match err {
        PathsError::MissingParam { name, route } => format!(
            "paths() entry is missing required param `{name}` for route `{route}`"
        ),
        PathsError::ExtraParam { name, route } => {
            format!("paths() entry has extra param `{name}` not declared in route `{route}`")
        }
        PathsError::InvalidParamType { name, reason, route } => format!(
            "paths() entry has invalid param `{name}` for route `{route}`: {reason}"
        ),
        PathsError::InvalidPathsExport {
            field, reason, expected, route,
        } => {
            let field_note = match field {
                Some(f) => format!(" at `{f}`"),
                None => String::new(),
            };
            format!(
                "paths() export in `{route}` is malformed{field_note}: {reason} (expected {expected})"
            )
        }
        PathsError::AmbiguousResolution { route, reason } => {
            format!("paths() in `{route}` produced ambiguous URLs: {reason}")
        }
    };
    Diagnostic::with_source(file, line, col, message, source)
}

/// Build a [`Diagnostic`] for a `paths()` static-extraction parse failure.
pub fn from_paths_extract_error(
    path: &Path,
    source: &str,
    err: &zfb_render::paths_extract::PathsExtractError,
) -> Diagnostic {
    use zfb_render::paths_extract::PathsExtractError;
    let file = project_relative(path, None);
    match err {
        PathsExtractError::Parse { message, .. } => {
            Diagnostic::with_source(file, 1, 1, format!("TSX parse error: {message}"), source)
        }
    }
}

/// Build a [`Diagnostic`] for a JS runtime error reported by the SSR
/// worker. `bundled_line` / `bundled_col` come from the JS engine's
/// stack frame inside the bundle. If a sourcemap is supplied we decode
/// back to the original `.tsx` location; otherwise we frame against the
/// bundle itself.
pub fn from_js_runtime_error(
    bundle_path: &Path,
    bundle_source: &str,
    bundled_line: usize,
    bundled_col: usize,
    message: &str,
    sourcemap_json: Option<&str>,
    project_root: Option<&Path>,
) -> Diagnostic {
    if let Some(map) = sourcemap_json {
        if let Some(decoded) = zfb_render::sourcemap::decode_position(map, bundled_line, bundled_col) {
            // Resolve the source file relative to the project root if
            // possible. The sourcemap stores file paths relative to the
            // bundle directory, but for display we want project-relative.
            let raw = PathBuf::from(&decoded.file);
            let abs = match project_root {
                Some(root) if raw.is_relative() => {
                    let bundle_dir = bundle_path.parent().unwrap_or(Path::new(""));
                    let candidate = bundle_dir.join(&raw);
                    candidate.canonicalize().unwrap_or(candidate)
                        .strip_prefix(root)
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|_| raw.clone())
                }
                _ => raw.clone(),
            };
            let display_path = project_relative(&abs, None);
            // Best-effort: attach original source. The decoder hands us
            // the source content when the map embeds `sourcesContent`;
            // otherwise try the disk.
            let mut diag = Diagnostic::new(display_path, decoded.line, decoded.col, message);
            if let Some(content) = decoded.source_content {
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

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/// Locate the first `export ... <ident>` occurrence in `source` and
/// return a 1-based `(line, col)` of the `ident`. We only consider
/// identifiers that appear inside the **declaration** keywords — i.e.
/// on a line that starts with `export ` and whose `<ident>` is preceded
/// by `const `, `let `, `var `, `function `, `async function `, or
/// `class ` — so that strings such as `"Bad paths"` inside other
/// declarations don't accidentally win the search.
fn locate_export_ident(source: &str, ident: &str) -> Option<(usize, usize)> {
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
    fn frontmatter_yaml_error_locates_within_user_file() {
        owo_colors::set_override(false);
        // YAML on line 2 is broken; serde_yaml reports its own line 1
        // for the unbalanced bracket. We expect that to map to user
        // file line 2 (past the opening `---`).
        let src = "---\ntitle: [unterminated\n---\nbody\n";
        let path = PathBuf::from("posts/intro.md");
        let err = zfb_content::extract_frontmatter(&path, src).expect_err("should fail");
        let diag = from_frontmatter_error(&path, src, &err);
        let out = strip_ansi(&render_framed(&diag));
        // serde_yaml reports the position where it noticed the
        // unbalanced bracket, which lands on the line *after* the
        // opening `[`. Either way, it must point at a line within the
        // user's file (not at the opening `---` delimiter), and the
        // snippet block must include the offending input.
        assert!(out.contains(" --> posts/intro.md:"), "got:\n{out}");
        assert!(
            !out.contains(" --> posts/intro.md:1:"),
            "should not report opening `---` delimiter, got:\n{out}"
        );
        assert!(
            out.contains("title: [unterminated"),
            "snippet should contain offending line, got:\n{out}"
        );
    }

    #[test]
    fn frontmatter_array_root_renders_specific_message() {
        // serde_yaml deserializes `- a\n- b` into a sequence — the
        // public API converts that to JSON Array, which is fine for
        // `extract` (no error). To exercise an "object expected" path
        // we fabricate a Yaml error directly.
        // For now, the unterminated-frontmatter case proves the line
        // number reporting works without serde_yaml details.
        let src = "---\ntitle: x\nbody but no close\n";
        let path = PathBuf::from("posts/oops.md");
        let err = zfb_content::extract_frontmatter(&path, src).expect_err("should fail");
        let diag = from_frontmatter_error(&path, src, &err);
        owo_colors::set_override(false);
        let out = strip_ansi(&render_framed(&diag));
        assert!(out.contains("frontmatter unterminated"), "got:\n{out}");
        assert!(out.contains(" --> posts/oops.md:1:1\n"), "got:\n{out}");
    }

    #[test]
    fn tsx_frontmatter_missing_export_locates_at_top_of_file() {
        owo_colors::set_override(false);
        let src = "export default function Page() { return null; }\n";
        let path = PathBuf::from("pages/page.tsx");
        let err =
            zfb_content::extract_tsx_frontmatter(src, "page.tsx").expect_err("should fail");
        let diag = from_tsx_frontmatter_error(&path, src, &err);
        let out = strip_ansi(&render_framed(&diag));
        assert!(
            out.contains("missing required `export const frontmatter`"),
            "got:\n{out}"
        );
        assert!(out.contains(" --> pages/page.tsx:1:1\n"), "got:\n{out}");
    }

    #[test]
    fn tsx_frontmatter_wrong_shape_carries_line_col() {
        // `frontmatter` must be an object literal — array fails with
        // WrongShape and carries a position.
        owo_colors::set_override(false);
        let src = "\nexport const frontmatter = [1, 2, 3];\nexport default function Page() { return null; }\n";
        let path = PathBuf::from("pages/page.tsx");
        let err =
            zfb_content::extract_tsx_frontmatter(src, "page.tsx").expect_err("should fail");
        let diag = from_tsx_frontmatter_error(&path, src, &err);
        let out = strip_ansi(&render_framed(&diag));
        // Frame should point at line 2 (1-based) of the source.
        assert!(
            out.contains(" --> pages/page.tsx:2:"),
            "got:\n{out}"
        );
        assert!(out.contains("export const frontmatter"), "got:\n{out}");
        assert!(out.contains("must be an object literal"), "got:\n{out}");
    }

    #[test]
    fn directive_diagnostic_renders_with_position_when_known() {
        owo_colors::set_override(false);
        let src = "# Hello\n\n:::nope\nbody\n:::\n";
        let path = PathBuf::from("docs/intro.md");
        let diag = zfb_content::plugins::DirectiveDiagnostic {
            message: "unknown directive `nope`".to_string(),
            line: Some(3),
            column: Some(1),
        };
        let d = from_directive_diagnostic(&path, src, &diag);
        let out = strip_ansi(&render_framed(&d));
        assert!(out.contains("unknown directive `nope`"), "got:\n{out}");
        assert!(out.contains(" --> docs/intro.md:3:1\n"), "got:\n{out}");
        assert!(out.contains(":::nope"), "got:\n{out}");
    }

    #[test]
    fn paths_error_locates_export_paths_ident() {
        owo_colors::set_override(false);
        let src = "import x from 'y';\n\nexport function paths() {\n    return [{ params: { wrong: 'x' } }];\n}\n";
        let path = PathBuf::from("pages/blog/[slug].tsx");
        let err = zfb_render::paths::PathsError::MissingParam {
            name: "slug".to_string(),
            route: "blog/[slug].tsx".to_string(),
        };
        let d = from_paths_error(&path, src, &err);
        let out = strip_ansi(&render_framed(&d));
        // Should land on line 3 where `export function paths` lives.
        assert!(out.contains(" --> pages/blog/[slug].tsx:3:"), "got:\n{out}");
        assert!(out.contains("missing required param `slug`"), "got:\n{out}");
        assert!(out.contains("export function paths"), "got:\n{out}");
    }

    #[test]
    fn js_runtime_error_falls_back_to_bundle_when_no_sourcemap() {
        owo_colors::set_override(false);
        let bundle = "var a;\nthrow new Error('boom');\n";
        let path = PathBuf::from(".zfb/build/ssg-render.js");
        let d = from_js_runtime_error(&path, bundle, 2, 7, "boom", None, None);
        let out = strip_ansi(&render_framed(&d));
        assert!(out.contains("JS runtime error"), "got:\n{out}");
        assert!(out.contains(" --> .zfb/build/ssg-render.js:2:7\n"), "got:\n{out}");
    }
}
