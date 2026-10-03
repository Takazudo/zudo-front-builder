//! Per-file `@jsxImportSource` pragma detection for authored renderer input.
//!
//! A per-file pragma overrides the project's `tsconfig.json` JSX import
//! source, so a leftover `/** @jsxImportSource preact */` makes esbuild
//! compile that file against a foreign runtime. The build then fails with an
//! unresolved `preact/jsx-runtime` import or, when Preact is still installed,
//! a `ZR_CHILD` render error — neither of which names the pragma. These
//! helpers find the pragma esbuild would honour so the bundler can warn about
//! it first. They only report; the source is never rewritten.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use swc_core::common::comments::SingleThreadedComments;
use swc_core::common::sync::Lrc;
use swc_core::common::{BytePos, FileName, SourceMap, Span};
use swc_core::ecma::ast::{EsVersion, JSXText};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax, TsSyntax};
use swc_core::ecma::visit::{Visit, VisitWith};

use zfb_types::{owned_runtime::JSX_IMPORT_SOURCE, path_to_posix_string};

/// The effective `@jsxImportSource` pragma of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsxImportSourcePragma {
    /// The import source the pragma names.
    pub value: String,
    /// 1-based line of the `@` that starts the pragma.
    pub line: usize,
    /// 1-based column (in characters) of that `@`.
    pub column: usize,
}

/// Find the `@jsxImportSource` pragma esbuild would apply to `source`.
///
/// Mirrors esbuild's comment pragma scan: the pragma may sit in any line or
/// block comment, the name must be followed by whitespace and a non-empty
/// argument, the last occurrence wins, and `@jsxRuntime classic` disables it.
/// Comments are taken from a real parse, so text inside strings, templates or
/// JSX text never matches. Returns `None` when the file does not parse.
pub fn find_jsx_import_source(source: &str, tsx: bool) -> Option<JsxImportSourcePragma> {
    if !source.contains("@jsxImportSource") {
        return None;
    }
    let cm: Lrc<SourceMap> = Default::default();
    let file = cm.new_source_file(FileName::Anon.into(), source.to_string());
    let comments = SingleThreadedComments::default();
    let syntax = if tsx {
        Syntax::Typescript(TsSyntax {
            tsx: true,
            ..Default::default()
        })
    } else {
        Syntax::Es(EsSyntax {
            jsx: true,
            ..Default::default()
        })
    };
    let lexer = Lexer::new(
        syntax,
        EsVersion::latest(),
        StringInput::from(&*file),
        Some(&comments),
    );
    let mut parser = Parser::new_from(lexer);
    let module = parser.parse_module().ok()?;
    // SWC's lexer also records `//` and `/*` runs inside JSX text as
    // comments; esbuild reads them as text, so they never carry a pragma.
    let mut jsx_text = JsxTextSpans::default();
    module.visit_with(&mut jsx_text);

    let (leading, trailing) = comments.borrow_all();
    let mut all: Vec<_> = leading
        .values()
        .chain(trailing.values())
        .flatten()
        .cloned()
        .collect();
    all.sort_by_key(|comment| comment.span.lo);
    all.dedup_by_key(|comment| comment.span.lo);

    let mut found = None;
    let mut classic = false;
    for comment in all
        .iter()
        .filter(|comment| !jsx_text.contains(comment.span.lo))
    {
        // `//` or `/*` precedes the comment text in the source.
        let text_start = (comment.span.lo.0 - file.start_pos.0) as usize + 2;
        for (offset, value) in pragma_args(&comment.text, "@jsxImportSource") {
            found = Some((text_start + offset, value));
        }
        if let Some((_, value)) = pragma_args(&comment.text, "@jsxRuntime").last() {
            classic = value == "classic";
        }
    }
    if classic {
        return None;
    }
    let (byte, value) = found?;
    let before = source.get(..byte)?;
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    Some(JsxImportSourcePragma {
        value,
        line,
        column: before[line_start..].chars().count() + 1,
    })
}

#[derive(Default)]
struct JsxTextSpans(Vec<Span>);

impl JsxTextSpans {
    fn contains(&self, pos: BytePos) -> bool {
        self.0.iter().any(|span| span.lo <= pos && pos < span.hi)
    }
}

impl Visit for JsxTextSpans {
    fn visit_jsx_text(&mut self, text: &JSXText) {
        self.0.push(text.span);
    }
}

/// Every `(offset, argument)` for `name` in a comment's text.
fn pragma_args(text: &str, name: &str) -> Vec<(usize, String)> {
    text.match_indices(name)
        .filter_map(|(offset, _)| {
            let rest = &text[offset + name.len()..];
            if !rest.starts_with(char::is_whitespace) {
                return None;
            }
            let value: String = rest
                .trim_start()
                .chars()
                .take_while(|c| !c.is_whitespace())
                .collect();
            (!value.is_empty()).then_some((offset, value))
        })
        .collect()
}

/// The warning for a foreign pragma in `file` (project-relative), or `None`
/// when the pragma names the owned runtime.
pub fn foreign_pragma_warning(file: &Path, pragma: &JsxImportSourcePragma) -> Option<String> {
    if pragma.value == JSX_IMPORT_SOURCE {
        return None;
    }
    Some(format!(
        "{}:{}:{}: per-file `@jsxImportSource {}` pragma overrides the project's JSX import source; \
         this file is compiled for the server renderer, which expects `{JSX_IMPORT_SOURCE}`. \
         Remove the pragma, or change it to `@jsxImportSource {JSX_IMPORT_SOURCE}`",
        path_to_posix_string(file),
        pragma.line,
        pragma.column,
        pragma.value,
    ))
}

/// Warnings for every foreign pragma among `files` (project-relative paths
/// read from `project_root`). Paths leaving the project, files that are not
/// `.tsx`/`.jsx`, missing files and files without a foreign pragma produce
/// nothing.
pub fn foreign_pragma_warnings<'a>(
    project_root: &Path,
    files: impl IntoIterator<Item = &'a Path>,
) -> Vec<(PathBuf, String)> {
    files
        .into_iter()
        .filter_map(|file| {
            if !file
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
            {
                return None;
            }
            let tsx = match file.extension().and_then(|ext| ext.to_str()) {
                Some("tsx") => true,
                Some("jsx") => false,
                _ => return None,
            };
            let source = std::fs::read_to_string(project_root.join(file)).ok()?;
            let pragma = find_jsx_import_source(&source, tsx)?;
            let message = foreign_pragma_warning(file, &pragma)?;
            Some((file.to_path_buf(), message))
        })
        .collect()
}

static REPORTED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Print each warning once per process, so a dev session's repeated bundles
/// and a build's several bundle passes report an authored file only once.
pub fn emit_foreign_pragma_warnings<'a>(
    project_root: &Path,
    files: impl IntoIterator<Item = &'a Path>,
) {
    for (_, message) in foreign_pragma_warnings(project_root, files) {
        let fresh = REPORTED
            .lock()
            .map(|mut reported| reported.insert(message.clone()))
            .unwrap_or(true);
        if fresh {
            tracing::warn!("{message}");
            eprintln!("zfb warn: {message}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pragma(source: &str) -> Option<JsxImportSourcePragma> {
        find_jsx_import_source(source, true)
    }

    #[test]
    fn finds_a_leading_block_pragma_with_its_position() {
        let source = "/** @jsxImportSource preact */\nexport const A = () => <p />;\n";
        assert_eq!(
            pragma(source),
            Some(JsxImportSourcePragma {
                value: "preact".into(),
                line: 1,
                column: 5,
            })
        );
    }

    #[test]
    fn finds_a_line_comment_pragma_after_code() {
        let source = "import { x } from \"./x\";\n\n  // @jsxImportSource react\nexport const A = () => <p>{x}</p>;\n";
        assert_eq!(
            pragma(source),
            Some(JsxImportSourcePragma {
                value: "react".into(),
                line: 3,
                column: 6,
            })
        );
    }

    #[test]
    fn ignores_strings_templates_and_jsx_text() {
        for source in [
            "export const doc = \"/** @jsxImportSource preact */\";\n",
            "export const doc = `// @jsxImportSource preact`;\n",
            "export const A = () => <p>// @jsxImportSource preact</p>;\n",
        ] {
            assert_eq!(pragma(source), None, "{source}");
        }
    }

    #[test]
    fn ignores_unrelated_comments_and_malformed_pragmas() {
        for source in [
            "/* see the @jsxImportSourceDocs page */\nexport {};\n",
            "/* @jsxImportSource */\nexport {};\n",
            "// mentions jsxImportSource preact without the at sign\nexport {};\n",
        ] {
            assert_eq!(pragma(source), None, "{source}");
        }
    }

    #[test]
    fn last_pragma_wins_and_classic_runtime_disables_it() {
        let source = "/** @jsxImportSource preact */\n/** @jsxImportSource @takazudo/zfb/zudo-react */\nexport {};\n";
        assert_eq!(pragma(source).unwrap().value, JSX_IMPORT_SOURCE);
        let classic = "/** @jsxRuntime classic @jsxImportSource preact */\nexport {};\n";
        assert_eq!(pragma(classic), None);
    }

    #[test]
    fn unparsable_files_report_nothing() {
        assert_eq!(
            pragma("/** @jsxImportSource preact */\nexport const = ;\n"),
            None
        );
    }

    #[test]
    fn plain_jsx_files_are_scanned_with_the_jsx_grammar() {
        let source = "/** @jsxImportSource preact */\nexport const A = () => <p />;\n";
        assert_eq!(
            find_jsx_import_source(source, false).map(|p| p.value),
            Some("preact".into())
        );
    }

    #[test]
    fn warns_with_project_path_position_and_advice_but_accepts_the_owned_runtime() {
        let project = tempfile::tempdir().unwrap();
        let write = |rel: &str, body: &str| {
            let path = project.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        };
        write(
            "components/legacy.tsx",
            "/** @jsxImportSource preact */\nexport const A = () => <p />;\n",
        );
        write(
            "components/owned.tsx",
            "/** @jsxImportSource @takazudo/zfb/zudo-react */\nexport const B = () => <p />;\n",
        );
        write("components/plain.tsx", "export const C = () => <p />;\n");
        write(
            "lib/widget.ts",
            "// @jsxImportSource preact\nexport const D = 1;\n",
        );

        let files = [
            Path::new("components/legacy.tsx"),
            Path::new("components/owned.tsx"),
            Path::new("components/plain.tsx"),
            Path::new("lib/widget.ts"),
            Path::new("components/missing.tsx"),
            Path::new("../outside/legacy.tsx"),
        ];
        let warnings = foreign_pragma_warnings(project.path(), files);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let (file, message) = &warnings[0];
        assert_eq!(file, Path::new("components/legacy.tsx"));
        assert!(
            message.starts_with(
                "components/legacy.tsx:1:5: per-file `@jsxImportSource preact` pragma overrides"
            ),
            "{message}"
        );
        assert!(
            message.contains(
                "Remove the pragma, or change it to `@jsxImportSource @takazudo/zfb/zudo-react`"
            ),
            "{message}"
        );
        assert!(!message.contains(&project.path().to_string_lossy().into_owned()));
    }
}
