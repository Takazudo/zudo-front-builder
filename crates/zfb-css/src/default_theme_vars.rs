//! Heuristic scanning of authored CSS for Tailwind default theme variables.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// Version of the accepted Tailwind 4.3 theme-variable namespace table:
/// <https://tailwindcss.com/docs/theme/#theme-variable-namespaces>.
pub const TAILWIND_DEFAULT_THEME_VOCABULARY_VERSION: u32 = 1;

const PREFIXES: &[&str] = &[
    "--color-",
    "--font-",
    "--font-weight-",
    "--text-",
    "--tracking-",
    "--leading-",
    "--breakpoint-",
    "--container-",
    "--spacing-",
    "--radius-",
    "--shadow-",
    "--inset-shadow-",
    "--drop-shadow-",
    "--blur-",
    "--perspective-",
    "--aspect-",
    "--ease-",
    "--animate-",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefaultThemeVarReference {
    pub name: String,
    pub has_fallback: bool,
    pub offset: usize,
    pub line: usize,
    pub column: usize,
}

/// Return custom properties declared at declaration positions in a stylesheet.
pub fn scan_custom_property_declarations(css: &str) -> BTreeSet<String> {
    scan(css).0
}

/// Return references in declaration values, including references inside fallbacks.
pub fn scan_default_theme_var_references(css: &str) -> Vec<DefaultThemeVarReference> {
    scan(css).1
}

/// Resolve declarations across the whole scanned stylesheet set before reporting.
pub fn undeclared_default_theme_var_references(
    stylesheets: &[(PathBuf, String)],
) -> Vec<(PathBuf, DefaultThemeVarReference)> {
    let scans: Vec<_> = stylesheets.iter().map(|(_, css)| scan(css)).collect();
    let declarations: BTreeSet<_> = scans
        .iter()
        .flat_map(|(decls, _)| decls.iter().cloned())
        .collect();
    stylesheets
        .iter()
        .zip(scans)
        .flat_map(|((path, _), (_, refs))| {
            refs.into_iter()
                .filter(|reference| {
                    !reference.has_fallback && !declarations.contains(&reference.name)
                })
                .map(|reference| (path.clone(), reference))
        })
        .collect()
}

fn in_vocabulary(name: &str) -> bool {
    name == "--spacing" || PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

// Replace comments and quoted strings with spaces while retaining byte offsets.
// Newlines remain so the original byte line and column can be computed directly.
fn visible_bytes(css: &str) -> Vec<u8> {
    let bytes = css.as_bytes();
    let mut visible = bytes.to_vec();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"/*") {
            let start = i;
            i += 2;
            while i < bytes.len() && !bytes[i..].starts_with(b"*/") {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            for byte in &mut visible[start..i] {
                if *byte != b'\n' {
                    *byte = b' ';
                }
            }
        } else if bytes[i] == b'\'' || bytes[i] == b'"' {
            let start = i;
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
            for byte in &mut visible[start..i] {
                if *byte != b'\n' {
                    *byte = b' ';
                }
            }
        } else {
            i += 1;
        }
    }
    visible
}

fn skip_space(bytes: &[u8], mut i: usize) -> usize {
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    i
}

fn identifier(bytes: &[u8], mut i: usize) -> (String, usize) {
    let mut name = String::new();
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 1;
            let start = i;
            while i < bytes.len() && i - start < 6 && bytes[i].is_ascii_hexdigit() {
                i += 1;
            }
            if i > start {
                let hex = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
                if let Some(ch) = u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
                    name.push(ch);
                }
                if bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
                    i += 1;
                }
            } else {
                name.push(bytes[i] as char);
                i += 1;
            }
        } else if bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-' || bytes[i] == b'_' {
            name.push(bytes[i] as char);
            i += 1;
        } else {
            break;
        }
    }
    (name, i)
}

fn scan(css: &str) -> (BTreeSet<String>, Vec<DefaultThemeVarReference>) {
    let bytes = visible_bytes(css);
    let mut declarations = BTreeSet::new();
    let mut references = Vec::new();
    let mut i = 0;
    let mut at_statement_start = true;
    let mut in_declaration = false;
    let mut parens = 0usize;
    let mut brackets = 0usize;
    while i < bytes.len() {
        if at_statement_start {
            i = skip_space(&bytes, i);
            if i >= bytes.len() {
                break;
            }
            let (name, end) = identifier(&bytes, i);
            if !name.is_empty() && bytes.get(skip_space(&bytes, end)) == Some(&b':') {
                if name.starts_with("--") {
                    declarations.insert(name);
                }
                i = skip_space(&bytes, end) + 1;
                in_declaration = true;
                at_statement_start = false;
                continue;
            }
            at_statement_start = false;
        }
        if in_declaration
            && bytes[i..].starts_with(b"var")
            && (i == 0 || !is_ident_byte(bytes[i - 1]))
            && bytes.get(i + 3) == Some(&b'(')
        {
            let start = skip_space(&bytes, i + 4);
            let (name, end) = identifier(&bytes, start);
            if in_vocabulary(&name) && name.starts_with("--") {
                let mut depth = 0usize;
                let mut cursor = end;
                let mut has_fallback = false;
                while cursor < bytes.len() {
                    match bytes[cursor] {
                        b'(' => depth += 1,
                        b')' if depth == 0 => break,
                        b')' => depth -= 1,
                        b',' if depth == 0 => {
                            has_fallback = true;
                            break;
                        }
                        b';' | b'}' if depth == 0 => break,
                        _ => {}
                    }
                    cursor += 1;
                }
                let prefix = &css[..i];
                references.push(DefaultThemeVarReference {
                    name,
                    has_fallback,
                    offset: i,
                    line: prefix.bytes().filter(|byte| *byte == b'\n').count() + 1,
                    column: prefix.rsplit('\n').next().unwrap_or("").len() + 1,
                });
            }
        }
        match bytes[i] {
            b'(' => parens += 1,
            b')' => parens = parens.saturating_sub(1),
            b'[' => brackets += 1,
            b']' => brackets = brackets.saturating_sub(1),
            b';' if parens == 0 && brackets == 0 => {
                at_statement_start = true;
                in_declaration = false;
            }
            b'{' if parens == 0 && brackets == 0 => {
                at_statement_start = true;
                in_declaration = false;
            }
            b'}' if parens == 0 && brackets == 0 => {
                at_statement_start = true;
                in_declaration = false;
            }
            _ => {}
        }
        i += 1;
    }
    (declarations, references)
}

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'\\')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_forms_and_nested_reference() {
        let css = ".x { a: var(--color-a); b: var(--color-a, 400); c: var(--color-a,); d: var(--color-a, ); e: var(--color-a,/*c*/); f: var(--color-a, rgb(0, 0, 0)); g: var(--color-a, var(--color-b)); }";
        let refs = scan_default_theme_var_references(css);
        assert_eq!(refs.len(), 8);
        assert_eq!(
            refs.iter()
                .filter(|r| !r.has_fallback)
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            ["--color-a", "--color-b"]
        );
    }

    #[test]
    fn declarations_in_other_sheets_and_nested_rules() {
        let declarations = "@layer base { :root { --color-a /* c */ : red; } } @media screen { .x { --f\\6f nt-weight-normal: 400 } } @supports (display: grid) { .x { --spacing: 1rem; & .y { --radius-sm: 3px } } }";
        let refs = ".x { a: var(--color-a); b: var(--font-weight-normal); c: var(--spacing); d: var(--radius-sm); }";
        assert!(undeclared_default_theme_var_references(&[
            ("a.css".into(), refs.into()),
            ("b.css".into(), declarations.into())
        ])
        .is_empty());
    }

    #[test]
    fn escapes_comments_strings_and_positions() {
        let css = "/* var(--color-fake) */ .x { content: 'var(--color-fake)'; a: var(--f\\6f nt-weight-normal) var(--color-a); }\n.x{a:var(--color-a)}";
        let refs = scan_default_theme_var_references(css);
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0].name, "--font-weight-normal");
        assert_ne!(refs[1].column, refs[0].column);
        assert_eq!((refs[2].line, refs[2].column), (2, 6));
        assert_eq!(refs[2].offset, css.find("\n.x{a:var(").unwrap() + 6);
    }

    #[test]
    fn vocabulary_excludes_unrelated_and_zw_names() {
        let css =
            ".x { a: var(--brand-accent) var(--zw-color-x) var(--spacing) var(--animate-spin); }";
        let names = scan_default_theme_var_references(css)
            .into_iter()
            .map(|r| r.name)
            .collect::<Vec<_>>();
        assert_eq!(names, ["--spacing", "--animate-spin"]);
        assert_eq!(TAILWIND_DEFAULT_THEME_VOCABULARY_VERSION, 1);
    }

    #[test]
    fn repeated_references_get_distinct_byte_columns() {
        let css = ".x{a:var(--color-a) var(--color-a)}";
        let refs = scan_default_theme_var_references(css);
        assert_eq!(refs.len(), 2);
        assert_eq!((refs[0].line, refs[0].column), (1, 6));
        assert_eq!((refs[1].line, refs[1].column), (1, 21));
        assert_eq!(refs[0].name, refs[1].name);
    }

    #[test]
    fn all_version_one_namespaces_are_recognized() {
        let css = format!(
            ".x{{a:{}}}",
            PREFIXES
                .iter()
                .map(|prefix| format!("var({prefix}x)"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        assert_eq!(
            scan_default_theme_var_references(&css).len(),
            PREFIXES.len()
        );
    }
}
