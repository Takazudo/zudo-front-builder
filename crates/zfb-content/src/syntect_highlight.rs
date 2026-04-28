//! Syntect-based syntax highlighting.
//!
//! Provides a [`Highlighter`] cache around syntect's [`SyntaxSet`] and
//! [`ThemeSet`], plus an [`Highlighter::highlight`] entry point that returns
//! an HTML fragment of the form
//! `<pre class="syntect-{theme-slug}"><code>…spans…</code></pre>`.
//!
//! The class on the `<pre>` element lets users theme blocks via CSS while still
//! getting syntect-coloured spans inside.

use std::path::Path;

use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::html::{styled_line_to_highlighted_html, IncludeBackground};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// Cached syntect resources.
pub struct Highlighter {
    syntax_set: SyntaxSet,
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
    pub fn new() -> Self {
        Self {
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
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

    /// Load extra themes from a directory at runtime (`.tmTheme` files).
    pub fn load_themes_from_dir(&mut self, dir: &Path) -> Result<(), HighlightError> {
        self.theme_set
            .add_from_folder(dir)
            .map_err(|e| HighlightError::ThemeParse(e.to_string()))?;
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
        // Normalize empty theme to the configured default for symmetry with `lang`.
        let theme_name = theme
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.default_theme);
        let theme_obj: &Theme = self
            .theme_set
            .themes
            .get(theme_name)
            .ok_or_else(|| HighlightError::UnknownTheme(theme_name.to_string()))?;

        let syntax = lang.filter(|s| !s.is_empty()).and_then(|l| {
            self.syntax_set.find_syntax_by_token(l).or_else(|| {
                resolve_alias(l)
                    .iter()
                    .find_map(|name| self.syntax_set.find_syntax_by_name(name))
            })
        });

        let Some(syntax) = syntax else {
            return Ok(fallback_html(code));
        };

        let mut h = HighlightLines::new(syntax, theme_obj);
        let mut spans = String::new();
        for line in LinesWithEndings::from(code) {
            let regions = h
                .highlight_line(line, &self.syntax_set)
                .map_err(|e| HighlightError::ThemeParse(e.to_string()))?;
            let line_html = styled_line_to_highlighted_html(&regions[..], IncludeBackground::No)
                .map_err(|e| HighlightError::ThemeParse(e.to_string()))?;
            spans.push_str(&line_html);
        }

        let slug = theme_slug(theme_name);
        Ok(format!(
            "<pre class=\"syntect-{slug}\"><code>{spans}</code></pre>"
        ))
    }
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

/// HTML-escape and wrap in `<pre><code>…</code></pre>` for unknown languages.
fn fallback_html(code: &str) -> String {
    format!("<pre><code>{}</code></pre>", html_escape(code))
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(html, "<pre><code>hello</code></pre>");
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
        assert_eq!(html, "<pre><code>x = 1</code></pre>");
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
}
