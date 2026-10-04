//! Token-aware detection of legacy Tailwind syntax in authored CSS.
use std::path::PathBuf;

use crate::{CssDiagnostic, CssDiagnosticOrigin, CssDiagnosticSeverity, WindDiagnosticsError};

/// Legacy Tailwind at-rules and functions zudo-wind rejects in authored CSS.
pub const FORBIDDEN_WIND_DIRECTIVES: &[&str] = &[
    "import",
    "tailwind",
    "theme",
    "source",
    "custom-variant",
    "apply",
    "utility",
    "variant",
    "plugin",
    "config",
    "reference",
    "--spacing",
    "--alpha",
    "--value",
];

/// Every forbidden directive in every stylesheet, as one ZW009 error, so a
/// migration sees all of them in one run instead of one per run.
pub fn check_forbidden_directives(
    stylesheets: &[(PathBuf, String)],
) -> Result<(), WindDiagnosticsError> {
    let diagnostics: Vec<_> = stylesheets
        .iter()
        .flat_map(|(path, css)| {
            scan_leftover_directives(css, FORBIDDEN_WIND_DIRECTIVES)
                .into_iter()
                .map(move |directive| CssDiagnostic {
                    severity: CssDiagnosticSeverity::Error,
                    code: "ZW009".into(),
                    message: format!(
                        "forbidden {}; migrate this stylesheet to zudo-wind: /docs/zudo-wind/coming-from-tailwind/",
                        directive.name
                    ),
                    origin: CssDiagnosticOrigin {
                        path: Some(path.clone()),
                        line: Some(directive.line),
                        column: Some(directive.column),
                        byte_offset: Some(directive.offset),
                        label: None,
                    },
                    candidate: None,
                })
        })
        .collect();
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(WindDiagnosticsError { diagnostics })
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeftoverDirective {
    pub name: String,
    pub offset: usize,
    pub line: usize,
    pub column: usize,
}

/// Scan selected at-rules and Tailwind functions, ignoring comments and strings.
/// Pass spellings without their leading `@`; `import` matches only Tailwind packages.
pub fn scan_leftover_directives(css: &str, directives: &[&str]) -> Vec<LeftoverDirective> {
    let bytes = css.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if !css.is_char_boundary(i) {
            i += 1;
            continue;
        }
        if bytes[i..].starts_with(b"/*") {
            i += 2;
            while i < bytes.len() && !bytes[i..].starts_with(b"*/") {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        if bytes[i] == b'\'' || bytes[i] == b'"' {
            let quote = bytes[i];
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i = (i + 2).min(bytes.len());
                } else if bytes[i] == quote {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        if let Some(function) = ["theme", "--spacing", "--alpha", "--value"]
            .into_iter()
            .find(|function| {
                directives.contains(function)
                    && css[i..].starts_with(function)
                    && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'-')
                    && bytes.get(i + function.len()) == Some(&b'(')
            })
        {
            let prefix = &css[..i];
            found.push(LeftoverDirective {
                name: format!("{function}()"),
                offset: i,
                line: prefix.bytes().filter(|b| *b == b'\n').count() + 1,
                column: prefix.rsplit('\n').next().unwrap_or("").len() + 1,
            });
            i += function.len();
            continue;
        }
        if bytes[i] == b'@' {
            let start = i;
            i += 1;
            let mut name = String::new();
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 1;
                    if bytes[i].is_ascii_hexdigit() {
                        let begin = i;
                        while i < bytes.len() && i - begin < 6 && bytes[i].is_ascii_hexdigit() {
                            i += 1;
                        }
                        if let Ok(value) = u32::from_str_radix(&css[begin..i], 16) {
                            if let Some(ch) = char::from_u32(value) {
                                name.push(ch);
                            }
                        }
                        if i < bytes.len() && bytes[i].is_ascii_whitespace() {
                            i += 1;
                        }
                    } else {
                        name.push(bytes[i] as char);
                        i += 1;
                    }
                } else if bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-' {
                    name.push(bytes[i] as char);
                    i += 1;
                } else {
                    break;
                }
            }
            let name = name.to_ascii_lowercase();
            let mut forbidden = directives.contains(&name.as_str());
            if name == "import" && forbidden {
                let rest = trim_css_trivia(&css[i..]);
                let rest = rest
                    .strip_prefix("url(")
                    .map(trim_css_trivia)
                    .unwrap_or(rest);
                let value = if let Some(quoted) = rest.strip_prefix('"') {
                    quoted.split('"').next().unwrap_or("")
                } else if let Some(quoted) = rest.strip_prefix('\'') {
                    quoted.split('\'').next().unwrap_or("")
                } else {
                    rest.split(|c: char| c.is_ascii_whitespace() || c == ')' || c == ';')
                        .next()
                        .unwrap_or("")
                };
                let decoded = decode_css_escapes(value);
                forbidden = decoded == "tailwindcss" || decoded.starts_with("tailwindcss/");
            }
            if forbidden {
                let prefix = &css[..start];
                found.push(LeftoverDirective {
                    name: format!("@{name}"),
                    offset: start,
                    line: prefix.bytes().filter(|b| *b == b'\n').count() + 1,
                    column: prefix.rsplit('\n').next().unwrap_or("").len() + 1,
                });
            }
            continue;
        }
        i += 1;
    }
    found
}

fn trim_css_trivia(mut value: &str) -> &str {
    loop {
        value = value.trim_start_matches(char::is_whitespace);
        if let Some(comment) = value.strip_prefix("/*") {
            if let Some(end) = comment.find("*/") {
                value = &comment[end + 2..];
                continue;
            }
        }
        return value;
    }
}

fn decode_css_escapes(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        let mut hex = String::new();
        while chars.peek().is_some_and(|next| next.is_ascii_hexdigit()) && hex.len() < 6 {
            hex.push(chars.next().unwrap());
        }
        if hex.is_empty() {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            if let Some(decoded) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                out.push(decoded);
            }
            if chars.peek().is_some_and(|next| next.is_ascii_whitespace()) {
                chars.next();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ignores_comments_and_strings() {
        let result = scan_leftover_directives(
            "/* @apply x */ .a{content:'@theme'}\n@\\61 pply x;",
            &["apply", "theme"],
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "@apply");
        assert_eq!(result[0].line, 2);
    }
    #[test]
    fn import_only_tailwind() {
        let result = scan_leftover_directives(
            "@import 'package/style.css'; @import 'tailwindcss/utilities';",
            &["import"],
        );
        assert_eq!(result.len(), 1);
    }
    #[test]
    fn escaped_import_and_legacy_function_are_detected() {
        let css = r#"@import "tail\windcss/utilities"; .x { color: theme(red); }"#;
        let result = scan_leftover_directives(css, &["import", "theme"]);
        assert_eq!(
            result.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["@import", "theme()"]
        );
    }
    #[test]
    fn comments_between_import_and_specifier_do_not_hide_tailwind() {
        let result =
            scan_leftover_directives("@import /* note */ url( /* x */ tailwindcss);", &["import"]);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn every_forbidden_directive_in_every_stylesheet_is_reported() {
        let error = check_forbidden_directives(&[
            (
                PathBuf::from("entry.css"),
                "@theme {}\n@theme static {}\n@source \"x\";\n".into(),
            ),
            (
                PathBuf::from("imported.css"),
                ".a { color: red; }\n@apply p-1;\n".into(),
            ),
            (PathBuf::from("clean.css"), ".b { color: blue; }\n".into()),
        ])
        .unwrap_err();
        let locations: Vec<_> = error
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.origin.location().unwrap())
            .collect();
        assert_eq!(
            locations,
            [
                "entry.css:1:1",
                "entry.css:2:1",
                "entry.css:3:1",
                "imported.css:2:1"
            ]
        );
        assert!(error.diagnostics.iter().all(|d| d.code == "ZW009"));
        let rendered = error.to_string();
        assert!(
            rendered.starts_with("wind CSS failed with 4 errors:"),
            "{rendered}"
        );
        assert!(
            rendered.contains("entry.css:2:1: ZW009: forbidden @theme"),
            "{rendered}"
        );
        assert!(
            rendered.contains("imported.css:2:1: ZW009: forbidden @apply"),
            "{rendered}"
        );
        assert!(check_forbidden_directives(&[(PathBuf::from("clean.css"), ".b{}".into())]).is_ok());
    }
}
