//! Lightweight scanner that discovers `*.module.css` imports inside
//! TSX/JSX/TS/JS source files.
//!
//! The pipeline accepts two ways of feeding CSS Modules in:
//!
//! 1. **Explicit list** — the caller already knows which `.module.css`
//!    files are in play and passes them in
//!    [`crate::CssPipelineConfig::css_modules`]. Used by tests and by
//!    callers that already maintain their own module graph (e.g. the
//!    full `zfb-build` graph in production).
//!
//! 2. **Auto-discovery** — the caller hands in a list of "entry" source
//!    files (page TSX, layout TSX, MDX-compiled output, …) and asks
//!    `zfb-css` to scan them for `import * from "*.module.css"`
//!    statements. That mode is convenient for the dev pipeline, where
//!    the rebuild plan already knows which pages changed but not which
//!    modules they reach.
//!
//! Auto-discovery is intentionally a flat scan — it does NOT recurse
//! into TSX→TSX imports. The full transitive walk is owned by the
//! islands scanner ([`zfb_islands::scanner`]) and the eventual zfb-build
//! module graph; that crate is where the SWC AST already lives. Keeping
//! `zfb-css` regex-based avoids dragging the SWC graph into the CSS
//! crate just to find a string token.
//!
//! ## Pattern
//!
//! The recognised import shape is:
//!
//! ```text
//! import <anything> from "<...>.module.css";
//! import "<...>.module.css";
//! ```
//!
//! Both single and double quotes are accepted. Bare `require("...")`
//! style imports are also recognised so that a hypothetical CommonJS
//! consumer is not silently skipped.
//!
//! ## Path resolution
//!
//! Imports are resolved relative to the importing file. A leading
//! `"./"` or `"../"` is normalised. Bare specifiers (e.g.
//! `"@org/pkg/foo.module.css"`) are returned unresolved — the scanner
//! does not attempt `node_modules` resolution. Callers that need
//! cross-package CSS Modules must pre-resolve and pass them in via
//! [`crate::CssPipelineConfig::css_modules`].
//!
//! ## Output
//!
//! [`scan_css_module_imports`] returns a [`ModuleImportScan`] with two
//! fields:
//!
//! - `modules: Vec<PathBuf>` — deduplicated list of resolved `.module.css`
//!   paths, sorted lexicographically for stable hash inputs.
//! - `per_source: Vec<SourceModuleUsage>` — for each input source file,
//!   the list of `.module.css` paths it imports (ordered as encountered).
//!   This is the metadata the dev/SSE asset-graph crate consumes to
//!   answer "which pages use which CSS modules".

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::Result;

/// Per-source-file record of CSS Modules usage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceModuleUsage {
    /// The source file we scanned (path as provided by the caller).
    pub source: PathBuf,
    /// The `.module.css` paths it imports, in order of appearance.
    /// Unresolved bare specifiers (no leading `./` / `../` / `/`) are
    /// kept as their original string form via [`PathBuf::from`] so the
    /// caller can decide whether to drop or pre-resolve them.
    pub modules: Vec<PathBuf>,
}

/// Aggregate output of [`scan_css_module_imports`].
#[derive(Debug, Clone, Default)]
pub struct ModuleImportScan {
    /// Deduplicated, sorted list of every resolved `.module.css` path
    /// the scanner discovered. Suitable to pass to
    /// [`crate::CssModulesProcessor::process`].
    pub modules: Vec<PathBuf>,
    /// Per-source breakdown, in the same order as the input slice.
    pub per_source: Vec<SourceModuleUsage>,
}

/// Scan the given source files for `*.module.css` imports.
///
/// Sources that don't exist on disk (or fail to read for other I/O
/// reasons) are skipped silently with an empty `modules` list — matching
/// the engine-level convention that `sources` is a *hint*, not a
/// hard requirement. Callers that want fail-fast scanning should
/// pre-validate paths.
pub fn scan_css_module_imports(sources: &[PathBuf]) -> Result<ModuleImportScan> {
    let mut all: BTreeSet<PathBuf> = BTreeSet::new();
    let mut per_source: Vec<SourceModuleUsage> = Vec::with_capacity(sources.len());

    for src in sources {
        let text = match std::fs::read_to_string(src) {
            Ok(t) => t,
            Err(_) => {
                per_source.push(SourceModuleUsage {
                    source: src.clone(),
                    modules: Vec::new(),
                });
                continue;
            }
        };
        let mut modules: Vec<PathBuf> = Vec::new();
        for spec in extract_module_css_specifiers(&text) {
            let resolved = resolve_specifier(src, &spec);
            if !modules.contains(&resolved) {
                modules.push(resolved.clone());
            }
            all.insert(resolved);
        }
        per_source.push(SourceModuleUsage {
            source: src.clone(),
            modules,
        });
    }

    let mut modules: Vec<PathBuf> = all.into_iter().collect();
    modules.sort();
    Ok(ModuleImportScan {
        modules,
        per_source,
    })
}

/// In-memory variant — the caller already has the source text.
pub fn scan_css_module_imports_in_memory(sources: &[(PathBuf, String)]) -> ModuleImportScan {
    let mut all: BTreeSet<PathBuf> = BTreeSet::new();
    let mut per_source: Vec<SourceModuleUsage> = Vec::with_capacity(sources.len());
    for (src, text) in sources {
        let mut modules: Vec<PathBuf> = Vec::new();
        for spec in extract_module_css_specifiers(text) {
            let resolved = resolve_specifier(src, &spec);
            if !modules.contains(&resolved) {
                modules.push(resolved.clone());
            }
            all.insert(resolved);
        }
        per_source.push(SourceModuleUsage {
            source: src.clone(),
            modules,
        });
    }
    let mut modules: Vec<PathBuf> = all.into_iter().collect();
    modules.sort();
    ModuleImportScan {
        modules,
        per_source,
    }
}

/// Extract every string-literal specifier ending in `.module.css` from a
/// source-file text.
///
/// The regex-free implementation is deliberate: a tiny hand-rolled state
/// machine over the byte stream is faster than a regex engine for our
/// pattern, doesn't pull in a new dependency, and is trivial to audit.
///
/// Line (`//`) and block (`/* … */`) comments are skipped before specifier
/// extraction, so a commented-out `import "….module.css"` is ignored (see
/// the `skips_comments` test). String scanning takes precedence over comment
/// scanning, so a `/*` appearing inside a string literal is treated as part
/// of the string, not a comment start.
fn extract_module_css_specifiers(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' || b == b'\'' || b == b'`' {
            let quote = b;
            // Find the matching quote on the same line. We don't
            // support escape sequences other than skipping `\\` and the
            // next char, because module specifiers don't legitimately
            // contain them.
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() {
                let c = bytes[j];
                if c == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                    continue;
                }
                if c == b'\n' {
                    // String never closed (syntax error in the source);
                    // bail out of this string and continue scanning.
                    break;
                }
                if c == quote {
                    if let Ok(s) = std::str::from_utf8(&bytes[start..j]) {
                        if s.ends_with(".module.css") {
                            out.push(s.to_string());
                        }
                    }
                    j += 1;
                    break;
                }
                j += 1;
            }
            i = j;
        } else if b == b'/' && i + 1 < bytes.len() {
            // Skip line / block comments so we don't match inside them.
            let next = bytes[i + 1];
            if next == b'/' {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            } else if next == b'*' {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = i.saturating_add(2);
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Resolve a `*.module.css` specifier relative to its importing file.
///
/// Rules:
///
/// - Specifiers that start with `./` or `../` resolve against the
///   importing file's parent directory and have `.` / `..` segments
///   normalised away.
/// - Absolute specifiers (`/foo/bar.module.css`) are kept as-is.
/// - Bare specifiers (no leading slash or dot — e.g. an npm package
///   subpath) are kept as-is. Real `node_modules` resolution is the
///   bundler's job, not the CSS scanner's.
fn resolve_specifier(importer: &Path, specifier: &str) -> PathBuf {
    if specifier.starts_with("./") || specifier.starts_with("../") {
        let parent = importer.parent().unwrap_or_else(|| Path::new(""));
        let joined = parent.join(specifier);
        normalise_path(&joined)
    } else {
        PathBuf::from(specifier)
    }
}

/// Lexically normalise a path: collapse `.` and `..` segments without
/// touching the filesystem. `..` past the start is dropped silently —
/// the alternative is making the function fallible just to match a
/// shape the rest of the pipeline doesn't handle either.
fn normalise_path(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out: Vec<Component> = Vec::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.last(), Some(Component::Normal(_))) {
                    out.pop();
                } else {
                    // RootDir / Prefix / nothing — leave it as-is so
                    // we don't escape /.
                    out.push(comp);
                }
            }
            other => out.push(other),
        }
    }
    let mut buf = PathBuf::new();
    for c in out {
        buf.push(c.as_os_str());
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_default_import() {
        let src = r#"import styles from "./button.module.css";"#;
        assert_eq!(
            extract_module_css_specifiers(src),
            vec!["./button.module.css".to_string()]
        );
    }

    #[test]
    fn extracts_namespace_and_bare() {
        let src = r#"
            import * as s from './a.module.css';
            import "./side-effect.module.css";
        "#;
        let got = extract_module_css_specifiers(src);
        assert_eq!(
            got,
            vec![
                "./a.module.css".to_string(),
                "./side-effect.module.css".to_string()
            ]
        );
    }

    #[test]
    fn ignores_non_module_css_imports() {
        let src = r#"
            import "./global.css";
            import x from "./foo.ts";
        "#;
        assert!(extract_module_css_specifiers(src).is_empty());
    }

    #[test]
    fn skips_comments() {
        let src = r#"
            // import x from "./commented.module.css";
            /* import x from "./block.module.css"; */
            import real from "./real.module.css";
        "#;
        assert_eq!(
            extract_module_css_specifiers(src),
            vec!["./real.module.css".to_string()]
        );
    }

    #[test]
    fn resolve_specifier_normalises_dots() {
        let importer = PathBuf::from("/proj/pages/blog/post.tsx");
        assert_eq!(
            resolve_specifier(&importer, "./post.module.css"),
            PathBuf::from("/proj/pages/blog/post.module.css")
        );
        assert_eq!(
            resolve_specifier(&importer, "../shared.module.css"),
            PathBuf::from("/proj/pages/shared.module.css")
        );
    }

    #[test]
    fn resolve_specifier_keeps_bare_and_absolute() {
        let importer = PathBuf::from("/proj/pages/index.tsx");
        assert_eq!(
            resolve_specifier(&importer, "/abs/x.module.css"),
            PathBuf::from("/abs/x.module.css")
        );
        assert_eq!(
            resolve_specifier(&importer, "@org/pkg/x.module.css"),
            PathBuf::from("@org/pkg/x.module.css")
        );
    }

    #[test]
    fn in_memory_scan_dedupes_and_sorts() {
        let inputs = vec![
            (
                PathBuf::from("/proj/pages/a.tsx"),
                r#"import s from "./shared.module.css";"#.to_string(),
            ),
            (
                PathBuf::from("/proj/pages/b.tsx"),
                r#"import s from "./shared.module.css";
                   import u from "./unique.module.css";"#
                    .to_string(),
            ),
        ];
        let scan = scan_css_module_imports_in_memory(&inputs);
        assert_eq!(
            scan.modules,
            vec![
                PathBuf::from("/proj/pages/shared.module.css"),
                PathBuf::from("/proj/pages/unique.module.css")
            ]
        );
        assert_eq!(scan.per_source.len(), 2);
        assert_eq!(scan.per_source[0].modules.len(), 1);
        assert_eq!(scan.per_source[1].modules.len(), 2);
    }
}
