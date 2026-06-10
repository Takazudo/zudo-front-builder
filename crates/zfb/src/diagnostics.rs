//! Framed user-facing diagnostics — converter functions for `zfb`-specific
//! error types.
//!
//! The core types ([`Diagnostic`], [`FramedError`], [`render_framed`]) live in
//! the standalone `zfb-diagnostics` crate. This module re-exports them for
//! backwards compatibility and adds converters for the `zfb`-specific error
//! types that depend on `zfb-content` and `zfb-render`.
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
//! 4. JS runtime errors thrown by the embedded V8 host — the host returns a
//!    stack frame inside the bundled SSR worker; we map the bundled
//!    `(line, col)` back to the original `.tsx` via
//!    [`zfb_render::sourcemap::decode_position`] and frame against the
//!    decoded file.
//!
//! All four reuse [`Diagnostic`] and [`render_framed`] so the on-screen
//! shape stays bit-identical across error classes.

use std::path::Path;

// Re-export the core types so callers can keep using `zfb::diagnostics::*`.
pub use zfb_diagnostics::{
    locate_export_ident, project_relative, render_framed, DecodedPosition, Diagnostic,
    FramedError,
};

// ---------------------------------------------------------------------------
// Converters from each first-class error type
// ---------------------------------------------------------------------------

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
        PathsError::MissingParam {
            name,
            route,
            provided,
        } => {
            let pretty = provided
                .iter()
                .map(|k| format!("`{k}`"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "paths() entry is missing required param `{name}` for route `{route}`: \
                 params must include `{name}`, got [{pretty}]"
            )
        }
        PathsError::ExtraParam {
            name,
            route,
            expected,
        } => {
            let pretty = expected
                .iter()
                .map(|k| format!("`{k}`"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "paths() entry has extra param `{name}` not declared in route `{route}`: \
                 expected one of [{pretty}], got `{name}`"
            )
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
    struct RenderDecoded(zfb_render::sourcemap::DecodedFrame);
    impl DecodedPosition for RenderDecoded {
        fn file(&self) -> &str {
            &self.0.file
        }
        fn line(&self) -> usize {
            self.0.line
        }
        fn col(&self) -> usize {
            self.0.col
        }
        fn source_content(&self) -> Option<String> {
            self.0.source_content.clone()
        }
    }

    zfb_diagnostics::from_js_runtime_error_with_decoder(
        bundle_path,
        bundle_source,
        bundled_line,
        bundled_col,
        message,
        sourcemap_json,
        project_root,
        |map, line, col| {
            zfb_render::sourcemap::decode_position(map, line, col).map(RenderDecoded)
        },
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

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
        let _color_lock = crate::output::color_override_lock::lock();
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
        let _color_lock = crate::output::color_override_lock::lock();
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
        let _color_lock = crate::output::color_override_lock::lock();
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
        let _color_lock = crate::output::color_override_lock::lock();
        owo_colors::set_override(false);
        // YAML on line 2 is broken; serde_yaml reports its own line 1
        // for the unbalanced bracket. We expect that to map to user
        // file line 2 (past the opening `---`).
        let src = "---\ntitle: [unterminated\n---\nbody\n";
        let path = PathBuf::from("posts/intro.md");
        let err = zfb_content::frontmatter::extract(&path, src).expect_err("should fail");
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
        let err = zfb_content::frontmatter::extract(&path, src).expect_err("should fail");
        let diag = from_frontmatter_error(&path, src, &err);
        let _color_lock = crate::output::color_override_lock::lock();
        owo_colors::set_override(false);
        let out = strip_ansi(&render_framed(&diag));
        assert!(out.contains("frontmatter unterminated"), "got:\n{out}");
        assert!(out.contains(" --> posts/oops.md:1:1\n"), "got:\n{out}");
    }

    #[test]
    fn tsx_frontmatter_missing_export_locates_at_top_of_file() {
        let _color_lock = crate::output::color_override_lock::lock();
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
        let _color_lock = crate::output::color_override_lock::lock();
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
        let _color_lock = crate::output::color_override_lock::lock();
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
        let _color_lock = crate::output::color_override_lock::lock();
        owo_colors::set_override(false);
        let src = "import x from 'y';\n\nexport function paths() {\n    return [{ params: { wrong: 'x' } }];\n}\n";
        let path = PathBuf::from("pages/blog/[slug].tsx");
        let err = zfb_render::paths::PathsError::MissingParam {
            name: "slug".to_string(),
            route: "blog/[slug].tsx".to_string(),
            provided: vec!["wrong".to_string()],
        };
        let d = from_paths_error(&path, src, &err);
        let out = strip_ansi(&render_framed(&d));
        // Should land on line 3 where `export function paths` lives.
        assert!(out.contains(" --> pages/blog/[slug].tsx:3:"), "got:\n{out}");
        assert!(out.contains("missing required param `slug`"), "got:\n{out}");
        assert!(
            out.contains("`wrong`"),
            "expected provided-keys `got [...]` clause to mention `wrong`, got:\n{out}",
        );
        assert!(out.contains("export function paths"), "got:\n{out}");
    }

    #[test]
    fn js_runtime_error_falls_back_to_bundle_when_no_sourcemap() {
        let _color_lock = crate::output::color_override_lock::lock();
        owo_colors::set_override(false);
        let bundle = "var a;\nthrow new Error('boom');\n";
        let path = PathBuf::from(".zfb/build/ssg-render.js");
        let d = from_js_runtime_error(&path, bundle, 2, 7, "boom", None, None);
        let out = strip_ansi(&render_framed(&d));
        assert!(out.contains("JS runtime error"), "got:\n{out}");
        assert!(out.contains(" --> .zfb/build/ssg-render.js:2:7\n"), "got:\n{out}");
    }
}
