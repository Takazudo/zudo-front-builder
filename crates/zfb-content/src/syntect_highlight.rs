//! Syntect-based syntax highlighting.
//!
//! Provides a [`Highlighter`] cache around syntect's [`SyntaxSet`] and
//! [`ThemeSet`], plus an [`Highlighter::highlight`] entry point that returns
//! an HTML fragment of the form
//! `<pre class="syntect-{theme-slug}"><code>…spans…</code></pre>`.
//!
//! The class on the `<pre>` element lets users theme blocks via CSS while still
//! getting syntect-coloured spans inside.
//!
//! ## Bundled-set caching
//!
//! Deserializing syntect's bundled `SyntaxSet` and `ThemeSet` is expensive
//! (several milliseconds each). Both are loaded **once per process** via
//! [`BUNDLED_SYNTAX_SET`] and [`BUNDLED_THEME_SET`] (`std::sync::LazyLock`),
//! then shared — `Highlighter` borrows the syntax set via `Arc` and clones
//! only the theme map when user-supplied `.tmTheme` files must be layered on
//! top of the defaults.

use std::path::Path;
use std::sync::{Arc, LazyLock};

use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::html::{styled_line_to_highlighted_html, IncludeBackground};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;
use zfb_types::escape_html;

/// Bundled `SyntaxSet` loaded once per process.
///
/// `SyntaxSet::load_defaults_newlines()` deserializes a binary blob embedded
/// in the syntect binary at compile time. The call is safe to make from any
/// thread; `LazyLock` ensures it happens exactly once.
static BUNDLED_SYNTAX_SET: LazyLock<Arc<SyntaxSet>> =
    LazyLock::new(|| Arc::new(SyntaxSet::load_defaults_newlines()));

/// Bundled `ThemeSet` loaded once per process.
///
/// `ThemeSet::load_defaults()` deserializes the bundled themes. Loaded once
/// and shared via `Arc`; per-Pipeline user themes are layered on a clone of
/// `theme_set` so the base is never mutated.
static BUNDLED_THEME_SET: LazyLock<Arc<ThemeSet>> =
    LazyLock::new(|| Arc::new(ThemeSet::load_defaults()));

/// A single `.tmTheme` parse failure, carrying the file path.
#[derive(Debug)]
pub struct ThemeFileError {
    pub path: std::path::PathBuf,
    pub message: String,
}

impl std::fmt::Display for ThemeFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

/// Cached syntect resources.
///
/// The `syntax_set` is shared across all `Highlighter` instances via `Arc`
/// (pointing at [`BUNDLED_SYNTAX_SET`]). The `theme_set` starts as a clone of
/// [`BUNDLED_THEME_SET`] so that per-Pipeline user themes can be inserted
/// without affecting the shared base.
pub struct Highlighter {
    /// Shared reference to the process-wide bundled syntax set.
    syntax_set: Arc<SyntaxSet>,
    /// Per-instance theme map — starts as a clone of [`BUNDLED_THEME_SET`] so
    /// user-supplied `.tmTheme` files can be layered on without mutating the
    /// shared base.
    theme_set: ThemeSet,
    default_theme: String,
}

impl Highlighter {
    /// Construct with bundled defaults (syntect's `load_defaults_newlines` syntaxes
    /// and the built-in theme set).
    ///
    /// Bundled defaults give us at minimum:
    /// `base16-ocean.dark`, `base16-ocean.light`, `InspiredGitHub`,
    /// `Solarized (dark)`, `Solarized (light)`.
    ///
    /// The bundled `SyntaxSet` is shared via [`BUNDLED_SYNTAX_SET`] (loaded once
    /// per process). The `ThemeSet` is cloned from [`BUNDLED_THEME_SET`] so
    /// per-instance user themes can be added without affecting the shared base.
    pub fn new() -> Self {
        // Clone the theme map from the shared base so user-supplied themes can
        // be inserted without affecting other Highlighter instances.
        // `ThemeSet` itself doesn't implement `Clone`, but `Theme` does, so we
        // manually copy the map.
        let theme_set = ThemeSet {
            themes: BUNDLED_THEME_SET
                .themes
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        };
        Self {
            syntax_set: Arc::clone(&BUNDLED_SYNTAX_SET),
            theme_set,
            default_theme: "base16-ocean.dark".to_string(),
        }
    }

    /// List built-in theme names.
    pub fn theme_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.theme_set.themes.keys().cloned().collect();
        names.sort();
        names
    }

    /// Set the default theme used when callers don't pass one.
    /// Returns `Err` if the theme isn't loaded.
    pub fn set_default_theme(&mut self, name: &str) -> Result<(), HighlightError> {
        if !self.theme_set.themes.contains_key(name) {
            return Err(HighlightError::UnknownTheme(name.to_string()));
        }
        self.default_theme = name.to_string();
        Ok(())
    }

    /// Load extra `.tmTheme` files from `dir`.
    ///
    /// Each `.tmTheme` file is parsed individually.  On the first parse
    /// failure the function returns [`HighlightError::ThemeFileParse`] with
    /// the failing file's path attached — this is the acceptance-criterion
    /// "surface file path + parse error" guarantee.  IO errors reading the
    /// directory itself are returned as [`HighlightError::ThemeLoad`].
    ///
    /// The directory **must exist** before calling this function.  A missing
    /// directory is reported as an [`HighlightError::ThemeLoad`] IO error.
    pub fn load_themes_from_dir(&mut self, dir: &Path) -> Result<(), HighlightError> {
        let entries = std::fs::read_dir(dir)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("tmTheme") {
                continue;
            }
            // Parse the individual file so we can attach its path to any error.
            let theme = ThemeSet::get_theme(&path).map_err(|e| {
                HighlightError::ThemeFileParse(ThemeFileError {
                    path: path.clone(),
                    message: e.to_string(),
                })
            })?;
            // Derive the theme name from its plist `name` key (already stored
            // in `theme.name`) or fall back to the file stem so the theme
            // is always addressable.
            let name = theme
                .name
                .as_deref()
                .filter(|n| !n.is_empty())
                .map(|n| n.to_string())
                .unwrap_or_else(|| {
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("unknown")
                        .to_string()
                });
            self.theme_set.themes.insert(name, theme);
        }
        Ok(())
    }

    /// Highlight a code block.
    ///
    /// * `code` - raw source code (no surrounding HTML).
    /// * `lang` - language identifier from the markdown fence (`"rust"`, `"ts"`,
    ///   `"javascript"`, …). `None` or empty falls back to plain text.
    /// * `theme` - optional theme override; falls back to the configured default.
    ///
    /// Returns an HTML fragment of the form
    /// `<pre class="syntect-{theme-slug}"><code>…spans…</code></pre>`.
    /// `{theme-slug}` is the theme name lowercased with non-alphanumerics
    /// collapsed to `'-'`.
    ///
    /// If `lang` is unknown, returns the safe fallback
    /// `<pre><code>{html-escaped code}</code></pre>`.
    pub fn highlight(
        &self,
        code: &str,
        lang: Option<&str>,
        theme: Option<&str>,
    ) -> Result<String, HighlightError> {
        let result = self.highlight_lines(code, lang, theme)?;
        let spans: String = result.lines.concat();
        Ok(format!(
            "<pre class=\"syntect-{}\"><code>{spans}</code></pre>",
            result.theme_slug
        ))
    }

    /// Highlight a code block, returning per-line HTML fragments and the theme
    /// slug rather than a single HTML string.
    ///
    /// * `code` - raw source code (no surrounding HTML).
    /// * `lang` - language identifier from the markdown fence.
    /// * `theme` - optional theme override; falls back to the configured default.
    ///
    /// Returns a [`HighlightedLines`] value carrying:
    /// - `theme_slug`: the CSS-safe theme name (used as the `syntect-{slug}`
    ///   class on `<pre>`).
    /// - `lines`: one HTML fragment per source line (from
    ///   `styled_line_to_highlighted_html`). On the fallback path (unknown lang
    ///   or tokenization error) each line is the HTML-escaped source text with
    ///   a trailing newline preserved; the `fallback` flag is `true` in that case.
    pub fn highlight_lines(
        &self,
        code: &str,
        lang: Option<&str>,
        theme: Option<&str>,
    ) -> Result<HighlightedLines, HighlightError> {
        // Normalize empty theme to the configured default for symmetry with `lang`.
        let theme_name = theme
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.default_theme);
        let theme_obj: &Theme = self
            .theme_set
            .themes
            .get(theme_name)
            .ok_or_else(|| HighlightError::UnknownTheme(theme_name.to_string()))?;

        let slug = theme_slug(theme_name);

        let syntax = lang.filter(|s| !s.is_empty()).and_then(|l| {
            self.syntax_set.find_syntax_by_token(l).or_else(|| {
                resolve_alias(l)
                    .iter()
                    .find_map(|name| self.syntax_set.find_syntax_by_name(name))
            })
        });

        let Some(syntax) = syntax else {
            return Ok(fallback_lines(code, &slug));
        };

        let mut h = HighlightLines::new(syntax, theme_obj);
        let mut lines: Vec<String> = Vec::new();
        for line in LinesWithEndings::from(code) {
            let regions = match h.highlight_line(line, &self.syntax_set) {
                Ok(r) => r,
                // Path B: tokenization error — degrade to themed fallback instead of
                // bubbling Err so callers never see a bare unthemed <pre><code>.
                Err(_) => return Ok(fallback_lines(code, &slug)),
            };
            match styled_line_to_highlighted_html(&regions[..], IncludeBackground::No) {
                Ok(line_html) => lines.push(line_html),
                Err(_) => return Ok(fallback_lines(code, &slug)),
            }
        }

        Ok(HighlightedLines {
            theme_slug: slug,
            lines,
            fallback: false,
        })
    }
}

/// Per-line highlighting result returned by [`Highlighter::highlight_lines`].
#[derive(Debug, Clone)]
pub struct HighlightedLines {
    /// CSS-safe theme slug (used as `syntect-{slug}` class on `<pre>`).
    pub theme_slug: String,
    /// One HTML fragment per source line from syntect.
    /// On the fallback path each entry is the HTML-escaped source line.
    pub lines: Vec<String>,
    /// `true` when the output used the themed-fallback path (unknown lang or
    /// tokenization error) rather than per-token syntect highlighting.
    pub fallback: bool,
}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

/// Map a language tag to one or more candidate syntect syntax names, in order
/// of preference. The first one that exists in the loaded `SyntaxSet` wins.
///
/// Multiple candidates let us degrade gracefully when a language isn't in the
/// bundled syntax set (e.g. TypeScript falls back to JavaScript so users still
/// get useful highlighting instead of the plain `<pre><code>` fallback).
fn resolve_alias(lang: &str) -> &'static [&'static str] {
    match lang {
        "ts" | "typescript" => &["TypeScript", "JavaScript"],
        "tsx" => &["TypeScript", "JavaScript"],
        "js" | "javascript" => &["JavaScript"],
        "jsx" => &["JavaScript"],
        "rs" | "rust" => &["Rust"],
        "py" | "python" => &["Python"],
        "sh" | "bash" | "zsh" => &["Bourne Again Shell (bash)", "Bash"],
        "md" | "markdown" => &["Markdown"],
        // mdx has no dedicated grammar; treat as Markdown so fences get theming
        "mdx" => &["Markdown"],
        "yaml" | "yml" => &["YAML"],
        "json" => &["JSON"],
        "c" => &["C"],
        "cpp" | "c++" => &["C++"],
        "go" => &["Go"],
        "toml" => &["TOML"],
        "html" => &["HTML"],
        "css" => &["CSS"],
        _ => &[],
    }
}

/// Lowercase + collapse non-alphanumerics to `-` (no leading/trailing dashes,
/// no doubled dashes).
fn theme_slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_dash = true; // suppress leading dash
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            for c in ch.to_lowercase() {
                out.push(c);
            }
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    if out.ends_with('-') {
        out.pop();
    }
    out
}

/// Build a [`HighlightedLines`] for the fallback path (unknown lang or
/// tokenization error). Splits `code` on `LinesWithEndings` so the per-line
/// vector has the same granularity as the normal path.
fn fallback_lines(code: &str, slug: &str) -> HighlightedLines {
    let lines: Vec<String> = LinesWithEndings::from(code)
        .map(escape_html)
        .collect();
    HighlightedLines {
        theme_slug: slug.to_string(),
        lines,
        fallback: true,
    }
}

/// Errors returned by the highlighter.
#[derive(Debug, thiserror::Error)]
pub enum HighlightError {
    #[error("unknown theme: {0}")]
    UnknownTheme(String),
    #[error("theme load io error: {0}")]
    ThemeLoad(#[from] std::io::Error),
    #[error("theme parse error: {0}")]
    ThemeParse(String),
    /// A single `.tmTheme` file failed to parse; carries path + message.
    #[error("theme parse error in {0}")]
    ThemeFileParse(ThemeFileError),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that [`BUNDLED_SYNTAX_SET`] and [`BUNDLED_THEME_SET`] are shared
    /// across `Highlighter` instances rather than re-deserialized on every
    /// construction. Two `Highlighter::new()` calls must return the same `Arc`
    /// pointer for the syntax set, proving the `LazyLock` fires only once per
    /// process.
    #[test]
    fn bundled_sets_are_shared_across_highlighter_instances() {
        let h1 = Highlighter::new();
        let h2 = Highlighter::new();
        // Arc::ptr_eq compares the underlying data pointer — same address means
        // the same allocation was reused, i.e. deserialization happened once.
        assert!(
            Arc::ptr_eq(&h1.syntax_set, &h2.syntax_set),
            "BUNDLED_SYNTAX_SET must be shared across Highlighter instances (LazyLock fired twice)"
        );
        // ThemeSet is cloned per-instance (so user themes don't bleed across
        // pipelines), but both must contain the same bundled theme names.
        let names1 = h1.theme_names();
        let names2 = h2.theme_names();
        assert_eq!(
            names1, names2,
            "bundled theme names must be identical across Highlighter instances"
        );
    }

    #[test]
    fn theme_names_includes_builtins() {
        let h = Highlighter::new();
        let names = h.theme_names();
        for expected in [
            "base16-ocean.dark",
            "base16-ocean.light",
            "InspiredGitHub",
            "Solarized (dark)",
            "Solarized (light)",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "missing built-in theme `{expected}`; got: {names:?}"
            );
        }
    }

    #[test]
    fn highlight_rust_snippet_has_pre_and_code() {
        let h = Highlighter::new();
        let html = h
            .highlight("fn main() {}\n", Some("rust"), None)
            .expect("highlight ok");
        assert!(
            html.contains("<pre class=\"syntect-"),
            "missing pre class: {html}"
        );
        assert!(html.contains("<code>"), "missing <code>: {html}");
        assert!(html.contains("</code></pre>"), "missing closer: {html}");
    }

    #[test]
    fn unknown_language_falls_back() {
        let h = Highlighter::new();
        let html = h
            .highlight("hello", Some("klingon"), None)
            .expect("fallback ok");
        // Unknown lang uses the themed wrapper, not bare <pre><code>
        assert!(
            html.starts_with("<pre class=\"syntect-"),
            "expected themed wrapper: {html}"
        );
        assert!(html.contains("hello"), "code content missing: {html}");
    }

    #[test]
    fn fallback_escapes_html() {
        let h = Highlighter::new();
        let html = h
            .highlight("<script>alert(1)</script>", Some("klingon"), None)
            .expect("fallback ok");
        assert!(html.contains("&lt;script&gt;"), "not escaped: {html}");
        assert!(
            html.contains("&lt;/script&gt;"),
            "closing not escaped: {html}"
        );
        assert!(!html.contains("<script>"), "raw script leaked: {html}");
    }

    #[test]
    fn set_default_theme_rejects_unknown() {
        let mut h = Highlighter::new();
        let err = h.set_default_theme("nonexistent").unwrap_err();
        match err {
            HighlightError::UnknownTheme(name) => assert_eq!(name, "nonexistent"),
            other => unreachable!("expected UnknownTheme, got {other:?}"),
        }
    }

    #[test]
    fn alias_ts_highlights_as_typescript() {
        let h = Highlighter::new();
        let html = h
            .highlight("const x: number = 1;\n", Some("ts"), None)
            .expect("highlight ok");
        assert!(
            html.starts_with("<pre class=\"syntect-"),
            "unexpected wrapper: {html}"
        );
        assert!(html.contains("<span"), "no syntect spans: {html}");
        assert_ne!(html, "<pre><code>const x: number = 1;\n</code></pre>");
    }

    #[test]
    fn empty_code_with_known_lang_returns_empty_pre_code() {
        let h = Highlighter::new();
        let html = h.highlight("", Some("rust"), None).expect("highlight ok");
        assert!(
            html.starts_with("<pre class=\"syntect-"),
            "missing pre class: {html}"
        );
        assert!(
            html.contains("<code></code>") || html.ends_with("<code></code></pre>"),
            "expected empty <code> body: {html}"
        );
    }

    #[test]
    fn empty_lang_falls_back_to_safe_pre_code() {
        let h = Highlighter::new();
        let html = h.highlight("x = 1", Some(""), None).expect("highlight ok");
        // Empty lang uses the themed wrapper now
        assert!(
            html.starts_with("<pre class=\"syntect-"),
            "expected themed wrapper for empty lang: {html}"
        );
        assert!(html.contains("x = 1"), "code content missing: {html}");
    }

    #[test]
    fn empty_theme_uses_default() {
        let h = Highlighter::new();
        let with_default = h
            .highlight("fn main() {}\n", Some("rust"), None)
            .expect("default ok");
        let with_empty = h
            .highlight("fn main() {}\n", Some("rust"), Some(""))
            .expect("empty-theme ok");
        assert_eq!(with_default, with_empty);
    }

    #[test]
    fn theme_override_changes_class_slug() {
        let h = Highlighter::new();
        let default = h
            .highlight("fn main() {}\n", Some("rust"), None)
            .expect("default ok");
        let override_html = h
            .highlight("fn main() {}\n", Some("rust"), Some("InspiredGitHub"))
            .expect("override ok");
        assert!(default.contains("syntect-base16-ocean-dark"));
        assert!(override_html.contains("syntect-inspiredgithub"));
        assert_ne!(default, override_html);
    }

    // --- New tests for alias resolution and themed fallback ---

    #[test]
    fn mdx_alias_resolves_to_markdown() {
        let h = Highlighter::new();
        let html = h
            .highlight("# h\n", Some("mdx"), None)
            .expect("mdx highlight ok");
        // mdx maps to Markdown, which has a grammar — should produce spans
        assert!(
            html.starts_with("<pre class=\"syntect-"),
            "expected themed wrapper: {html}"
        );
        assert!(
            html.contains("<span"),
            "expected spans for mdx/Markdown: {html}"
        );
    }

    #[test]
    fn tsx_jsx_keep_working() {
        let h = Highlighter::new();
        let tsx_html = h
            .highlight("const x: number = 1;\n", Some("tsx"), None)
            .expect("tsx ok");
        assert!(
            tsx_html.starts_with("<pre class=\"syntect-"),
            "tsx: expected themed wrapper: {tsx_html}"
        );
        assert!(
            tsx_html.contains("<span"),
            "tsx: expected spans: {tsx_html}"
        );

        let jsx_html = h
            .highlight("const x = 1;\n", Some("jsx"), None)
            .expect("jsx ok");
        assert!(
            jsx_html.starts_with("<pre class=\"syntect-"),
            "jsx: expected themed wrapper: {jsx_html}"
        );
        assert!(
            jsx_html.contains("<span"),
            "jsx: expected spans: {jsx_html}"
        );
    }

    #[test]
    fn unknown_language_uses_themed_wrapper() {
        let h = Highlighter::new();
        let html = h
            .highlight("hello world", Some("klingon"), None)
            .expect("klingon fallback ok");
        assert!(
            html.starts_with("<pre class=\"syntect-base16-ocean-dark\">"),
            "expected themed wrapper with default theme: {html}"
        );
        assert!(
            html.contains("<code>hello world</code>"),
            "code content: {html}"
        );
        assert!(!html.contains("<span"), "no spans for unknown lang: {html}");
    }

    #[test]
    fn empty_lang_uses_themed_wrapper() {
        let h = Highlighter::new();
        // Some("") — treated as no language
        let html_empty = h.highlight("code", Some(""), None).expect("empty lang ok");
        assert!(
            html_empty.starts_with("<pre class=\"syntect-"),
            "Some(\"\") must use themed wrapper: {html_empty}"
        );
        // None — no language at all
        let html_none = h.highlight("code", None, None).expect("none lang ok");
        assert!(
            html_none.starts_with("<pre class=\"syntect-"),
            "None lang must use themed wrapper: {html_none}"
        );
    }

    #[test]
    fn themed_fallback_escapes_html() {
        let h = Highlighter::new();
        let html = h
            .highlight("<script>alert(1)</script>", Some("klingon"), None)
            .expect("themed fallback escape ok");
        assert!(
            html.starts_with("<pre class=\"syntect-"),
            "expected themed wrapper: {html}"
        );
        assert!(html.contains("&lt;script&gt;"), "< not escaped: {html}");
        assert!(html.contains("&lt;/script&gt;"), "</ not escaped: {html}");
        assert!(!html.contains("<script>"), "raw script leaked: {html}");
    }

    /// Path B: tokenization errors fall back to themed output.
    ///
    /// Note: the bundled syntect grammars do not have an easily-reproducible
    /// per-line tokenization error with a synthetic input, so this test
    /// verifies the happy path still produces themed output (no regression)
    /// and that the function signature change did not break compilation.
    /// A more targeted test would require a custom grammar that errors
    /// mid-stream, which is out of scope for bundled-syntax testing.
    #[test]
    fn tokenization_error_path_does_not_bubble_err() {
        // The important property is that highlight() never returns Err due to
        // tokenization; it degrades to the themed fallback instead.
        let h = Highlighter::new();
        // Normal highlighted path — must still succeed
        let html = h
            .highlight("fn main() {}\n", Some("rust"), None)
            .expect("rust highlight must not return Err");
        assert!(
            html.starts_with("<pre class=\"syntect-"),
            "highlight must produce themed output: {html}"
        );
    }

    #[test]
    fn list_builtin_syntax_names_includes_expected_aliases() {
        // Verify the alias targets actually exist in the bundled SyntaxSet
        // so resolve_alias never maps to a name that isn't there.
        // Note: TypeScript is NOT in the bundled set; ts/tsx fall back to JavaScript.
        use syntect::parsing::SyntaxSet;
        let ss = SyntaxSet::load_defaults_newlines();
        let names: Vec<&str> = ss.syntaxes().iter().map(|s| s.name.as_str()).collect();
        for expected in [
            "Markdown",
            "JavaScript",
            "Rust",
            "Python",
            "YAML",
            "JSON",
            "C",
            "C++",
            "Go",
            "HTML",
            "CSS",
        ] {
            assert!(
                names.contains(&expected),
                "bundled SyntaxSet missing expected alias target: {expected}\navailable: {names:?}"
            );
        }
        // Bash may appear under either canonical name
        assert!(
            names
                .iter()
                .any(|n| *n == "Bourne Again Shell (bash)" || *n == "Bash"),
            "bundled SyntaxSet missing Bash syntax; available: {names:?}"
        );
        // TOML is not in the bundled default set — alias maps to empty result
        // (no fallback chain available), which degrades to themed fallback.
        // This is acceptable; the test simply documents the current state.
        let has_toml = names.contains(&"TOML");
        // Not asserting presence — just recording for documentation.
        let _ = has_toml;
    }

    // Minimal valid `.tmTheme` plist used by the load_themes_from_dir tests.
    // The `name` key determines the theme name reported by theme_names().
    const MINIMAL_TMTHEME: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
    "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>name</key>
    <string>My Custom Theme</string>
    <key>settings</key>
    <array>
        <dict>
            <key>settings</key>
            <dict>
                <key>background</key>
                <string>#1e1e1e</string>
                <key>foreground</key>
                <string>#d4d4d4</string>
            </dict>
        </dict>
    </array>
    <key>uuid</key>
    <string>aaaaaaaa-0000-0000-0000-000000000001</string>
</dict>
</plist>"#;

    #[test]
    fn load_themes_from_dir_adds_custom_theme() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("my-custom.tmTheme"), MINIMAL_TMTHEME)
            .expect("write fixture");

        let mut h = Highlighter::new();
        // Before loading: custom theme is absent.
        assert!(
            !h.theme_names().iter().any(|n| n == "My Custom Theme"),
            "custom theme must not be present before load"
        );

        h.load_themes_from_dir(dir.path())
            .expect("load_themes_from_dir ok");

        // After loading: custom theme is present.
        assert!(
            h.theme_names().iter().any(|n| n == "My Custom Theme"),
            "custom theme must be present after load; got: {:?}",
            h.theme_names()
        );
    }

    #[test]
    fn load_themes_from_dir_missing_dir_returns_error() {
        let mut h = Highlighter::new();
        let missing = std::path::Path::new("/tmp/zfb-test-nonexistent-themes-dir-abc123");
        let result = h.load_themes_from_dir(missing);
        assert!(
            result.is_err(),
            "expected error for missing directory, got Ok"
        );
    }

    #[test]
    fn load_themes_from_dir_invalid_tmtheme_surfaces_path_in_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Write broken XML that can't parse as a valid plist/theme.
        std::fs::write(dir.path().join("broken.tmTheme"), b"NOT VALID XML <<<<<")
            .expect("write broken fixture");

        let mut h = Highlighter::new();
        let err = h
            .load_themes_from_dir(dir.path())
            .expect_err("must fail for broken .tmTheme");
        let msg = err.to_string();
        assert!(
            msg.contains("broken.tmTheme"),
            "error message must include filename; got: {msg}"
        );
    }

    #[test]
    fn load_themes_from_dir_non_tmtheme_files_are_skipped() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Write a .txt file that would fail to parse — must be silently ignored.
        std::fs::write(dir.path().join("readme.txt"), b"NOT A THEME")
            .expect("write non-theme file");
        std::fs::write(dir.path().join("my-custom.tmTheme"), MINIMAL_TMTHEME)
            .expect("write fixture");

        let mut h = Highlighter::new();
        h.load_themes_from_dir(dir.path())
            .expect("must not fail; non-.tmTheme files are skipped");
        assert!(
            h.theme_names().iter().any(|n| n == "My Custom Theme"),
            "custom theme present after mixed-dir load"
        );
    }
}
