//! Vite-compatible terminal `?raw` import expansion (issue #1499).
//!
//! esbuild's CLI has no plugin hook, and treats a query-suffixed path as a
//! literal filename.  zfb therefore rewrites the one supported form
//!
//! ```text
//! import text from "./file.ext?raw";
//! ```
//!
//! to an import of a deterministic adjacent `.zfb-raw-*.mjs` module.  That
//! generated module default-exports the target file's UTF-8 text.  The target
//! is a terminal asset edge: callers materialise/watch it, but never parse it
//! as JavaScript (even when its filename ends in `.js`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use swc_core::common::sync::Lrc;
use swc_core::common::{FileName, SourceMap, Spanned};
use swc_core::ecma::ast::{Callee, Expr, ImportSpecifier, Module, ModuleDecl, ModuleItem};
use swc_core::ecma::parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};
use swc_core::ecma::visit::{Visit, VisitWith};
use zfb_types::normalize_path_lexical;

/// One terminal raw-import edge discovered while expanding a module.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RawImportEdge {
    /// Source module containing the supported import declaration.
    pub importer: PathBuf,
    /// Original on-disk file whose text is exported by the generated module.
    pub target: PathBuf,
}

/// One generated adjacent ESM module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedRawModule {
    /// Basename only (for example `.zfb-raw-a1b2c3d4e5f60718.mjs`).
    pub filename: String,
    /// Complete ESM source (`export default "...";`).
    pub source: String,
    /// Original file represented by this generated module.
    pub target: PathBuf,
}

/// Result of expanding all supported raw imports in one source module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawImportExpansion {
    /// Source with only the import-specifier literals rewritten.
    pub expanded_source: String,
    /// Generated modules, sorted and deduplicated by filename.
    pub generated_modules: Vec<GeneratedRawModule>,
    /// Typed terminal dependency edges for invalidation/bookkeeping.
    pub edges: Vec<RawImportEdge>,
}

#[derive(Debug)]
struct RawOccurrence {
    lo: usize,
    hi: usize,
    specifier: String,
}

fn supported_form_error(reason: impl AsRef<str>) -> anyhow::Error {
    anyhow!(
        "zfb bundler: unsupported import query form: {}. Only a static default import \
         written as `import text from \"./file.ext?raw\"` is supported. `?raw` is \
         terminal text loading; dynamic imports, `?url`, named/namespace/side-effect \
         raw imports, and additional query parameters are not supported.",
        reason.as_ref()
    )
}

fn atom_to_string(value: &swc_core::atoms::Wtf8Atom) -> String {
    value.to_atom_lossy().to_string()
}

fn query_suffix(specifier: &str) -> Option<&str> {
    specifier.split_once('?').map(|(_, query)| query)
}

fn parse_module(source: &str) -> Result<(Module, u32)> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(FileName::Anon.into(), source.to_string());
    let base = fm.start_pos.0;
    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax {
            tsx: true,
            decorators: false,
            dts: false,
            no_early_errors: false,
            disallow_ambiguous_jsx_like: false,
        }),
        swc_core::ecma::ast::EsVersion::Es2022,
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    let module = parser
        .parse_module()
        .map_err(|e| anyhow!("zfb bundler: failed to parse module for ?raw expansion: {e:?}"))?;
    Ok((module, base))
}

fn collect_raw_occurrences(source: &str) -> Result<Vec<RawOccurrence>> {
    if !source.contains('?') {
        return Ok(Vec::new());
    }

    let (module, base) = parse_module(source)?;
    let mut out = Vec::new();

    for item in &module.body {
        let ModuleItem::ModuleDecl(decl) = item else {
            continue;
        };
        match decl {
            ModuleDecl::Import(import) => {
                let specifier = atom_to_string(&import.src.value);
                let Some(query) = query_suffix(&specifier) else {
                    continue;
                };
                if query != "raw" || specifier[..specifier.len() - query.len() - 1].contains('?') {
                    return Err(supported_form_error(format!(
                        "import specifier {specifier:?} uses unsupported query `?{query}`"
                    )));
                }
                if import.type_only
                    || import.specifiers.len() != 1
                    || !matches!(import.specifiers[0], ImportSpecifier::Default(_))
                    || import.with.is_some()
                    || import.phase != swc_core::ecma::ast::ImportPhase::Evaluation
                {
                    return Err(supported_form_error(format!(
                        "{specifier:?} is not a single static default import"
                    )));
                }
                let span = import.src.span();
                out.push(RawOccurrence {
                    lo: (span.lo.0 - base) as usize,
                    hi: (span.hi.0 - base) as usize,
                    specifier,
                });
            }
            ModuleDecl::ExportNamed(named) => {
                if let Some(src) = &named.src {
                    let specifier = atom_to_string(&src.value);
                    if query_suffix(&specifier).is_some() {
                        return Err(supported_form_error(format!(
                            "re-export from {specifier:?} is not a default import"
                        )));
                    }
                }
            }
            ModuleDecl::ExportAll(all) => {
                let specifier = atom_to_string(&all.src.value);
                if query_suffix(&specifier).is_some() {
                    return Err(supported_form_error(format!(
                        "star re-export from {specifier:?} is not a default import"
                    )));
                }
            }
            _ => {}
        }
    }

    struct DynamicQueryFinder {
        error: Option<anyhow::Error>,
    }

    fn query_fragment(expr: &Expr) -> Option<String> {
        struct Finder {
            fragment: Option<String>,
        }
        impl Visit for Finder {
            fn visit_str(&mut self, node: &swc_core::ecma::ast::Str) {
                if self.fragment.is_none() {
                    let value = atom_to_string(&node.value);
                    if value.contains('?') {
                        self.fragment = Some(value);
                    }
                }
            }

            fn visit_tpl_element(&mut self, node: &swc_core::ecma::ast::TplElement) {
                if self.fragment.is_none() {
                    let value = node.raw.to_string();
                    if value.contains('?') {
                        self.fragment = Some(value);
                    }
                }
            }
        }

        let mut finder = Finder { fragment: None };
        expr.visit_with(&mut finder);
        finder.fragment
    }

    impl Visit for DynamicQueryFinder {
        fn visit_call_expr(&mut self, node: &swc_core::ecma::ast::CallExpr) {
            if self.error.is_some() {
                return;
            }
            let kind = match &node.callee {
                Callee::Import(_) => Some("dynamic import"),
                Callee::Expr(expr) if matches!(&**expr, Expr::Ident(ident) if ident.sym == "require") => {
                    Some("CommonJS require")
                }
                _ => None,
            };
            if let Some(kind) = kind {
                if let Some(arg) = node.args.first() {
                    let query_spec = query_fragment(&arg.expr);
                    if let Some(specifier) = query_spec {
                        self.error = Some(supported_form_error(format!(
                            "{kind} of {specifier:?} is not static"
                        )));
                        return;
                    }
                }
            }
            node.visit_children_with(self);
        }
    }
    let mut dynamic = DynamicQueryFinder { error: None };
    module.visit_with(&mut dynamic);
    if let Some(error) = dynamic.error {
        return Err(error);
    }

    Ok(out)
}

/// Resolve a raw target without routing the stripped path through the normal
/// JavaScript module resolver.
///
/// The supported spelling is project-local and relative (`./` or `../`).  An
/// exact file must exist; no JS extension probing or `index.*` resolution is
/// performed because the edge is terminal and accepts any extension.
pub fn resolve_raw_target(
    importer: &Path,
    specifier_without_query: &str,
    project_root: &Path,
) -> Result<PathBuf> {
    if !(specifier_without_query.starts_with("./") || specifier_without_query.starts_with("../")) {
        bail!(
            "zfb bundler: unsupported ?raw target {specifier_without_query:?} in {}. \
             Only project-local relative targets in the form \
             `import text from \"./file.ext?raw\"` are supported.",
            importer.display()
        );
    }
    let importer_dir = importer.parent().unwrap_or_else(|| Path::new("."));
    let candidate = normalize_path_lexical(&importer_dir.join(specifier_without_query));
    let lexical_root = normalize_path_lexical(project_root);
    if !candidate.starts_with(&lexical_root) {
        bail!(
            "zfb bundler: raw import target {specifier_without_query:?} from {} escapes the \
             logical project root {} (resolved lexical path {}). Raw targets must remain \
             project-local.",
            importer.display(),
            lexical_root.display(),
            candidate.display()
        );
    }
    if !candidate.is_file() {
        bail!(
            "zfb bundler: raw import target {specifier_without_query:?} from {} does not \
             resolve to an existing file (looked for {}).",
            importer.display(),
            candidate.display()
        );
    }
    let canonical_root = project_root.canonicalize().with_context(|| {
        format!(
            "canonicalize logical project root {} for ?raw containment",
            project_root.display()
        )
    })?;
    let canonical_target = candidate.canonicalize().with_context(|| {
        format!(
            "canonicalize raw import target {} before reading it",
            candidate.display()
        )
    })?;
    if !canonical_target.starts_with(&canonical_root) {
        bail!(
            "zfb bundler: raw import target {} from {} escapes the project root {} through \
             a symlink (canonical target {}). Raw targets must remain project-local.",
            candidate.display(),
            importer.display(),
            canonical_root.display(),
            canonical_target.display()
        );
    }
    // Preserve the lexical project-root spelling for `bundle.exclude`
    // relativisation and invalidation aliases (notably macOS `/var` ->
    // `/private/var` temp roots). Consumers register both this logical path
    // and its current canonical target.
    Ok(candidate)
}

fn generated_filename(specifier_without_query: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(specifier_without_query.as_bytes());
    let digest = hex::encode(hasher.finalize());
    format!(".zfb-raw-{}.mjs", &digest[..16])
}

/// Expand supported terminal `?raw` imports in `source`.
///
/// `importer` is the logical project source path, not a shadow/overlay copy.
/// `project_root` is the matching logical root and bounds both lexical `..`
/// traversal and canonical symlink resolution before target bytes are read.
/// Targets are read on every invocation, making a persistent dev shadow
/// sensitive to target-only edits. `is_excluded` receives the resolved logical
/// target and must return true for a `bundle.exclude` match; excluding a
/// terminal target is a hard, named error.
pub fn expand_raw_imports(
    source: &str,
    importer: &Path,
    project_root: &Path,
    is_excluded: &dyn Fn(&Path) -> bool,
) -> Result<RawImportExpansion> {
    let occurrences = collect_raw_occurrences(source)?;
    if occurrences.is_empty() {
        return Ok(RawImportExpansion {
            expanded_source: source.to_string(),
            generated_modules: Vec::new(),
            edges: Vec::new(),
        });
    }

    let importer_dir = importer.parent().unwrap_or_else(|| Path::new("."));
    let mut replacements = Vec::with_capacity(occurrences.len());
    let mut modules: BTreeMap<String, GeneratedRawModule> = BTreeMap::new();
    let mut edges = Vec::with_capacity(occurrences.len());

    for occurrence in occurrences {
        let target_specifier = occurrence
            .specifier
            .strip_suffix("?raw")
            .expect("collector accepts only exact ?raw suffix");
        let target = resolve_raw_target(importer, target_specifier, project_root)?;
        if is_excluded(&target) {
            bail!(
                "zfb bundler: raw import target {} (imported from {}) is excluded by \
                 `bundle.exclude`. A `?raw` target is a required terminal dependency; \
                 remove the matching exclude pattern or remove the import.",
                target.display(),
                importer.display()
            );
        }

        let filename = generated_filename(target_specifier);
        let reserved_path = importer_dir.join(&filename);
        if reserved_path.exists() && reserved_path != target {
            bail!(
                "zfb bundler: cannot materialise raw import from {} because reserved \
                 generated path {} already exists. Rename that `.zfb-raw-*.mjs` file.",
                importer.display(),
                reserved_path.display()
            );
        }

        let bytes = std::fs::read(&target)
            .with_context(|| format!("read raw import target {}", target.display()))?;
        let text = String::from_utf8(bytes).map_err(|e| {
            anyhow!(
                "zfb bundler: raw import target {} is not valid UTF-8 text: {e}",
                target.display()
            )
        })?;
        let encoded = serde_json::to_string(&text)
            .context("serialize raw import target as a JavaScript string")?;
        let module_source = format!("export default {encoded};\n");
        match modules.get(&filename) {
            Some(existing) if existing.target != target => {
                bail!(
                    "zfb bundler: internal ?raw generated-name collision between {} and {}",
                    existing.target.display(),
                    target.display()
                );
            }
            Some(_) => {}
            None => {
                modules.insert(
                    filename.clone(),
                    GeneratedRawModule {
                        filename: filename.clone(),
                        source: module_source,
                        target: target.clone(),
                    },
                );
            }
        }

        let generated_specifier = format!("./{filename}");
        let replacement = serde_json::to_string(&generated_specifier)
            .context("serialize generated raw-module import specifier")?;
        replacements.push((occurrence.lo, occurrence.hi, replacement));
        edges.push(RawImportEdge {
            importer: importer.to_path_buf(),
            target,
        });
    }

    let mut expanded_source = source.to_string();
    for (lo, hi, replacement) in replacements.iter().rev() {
        let valid = source
            .get(*lo..*hi)
            .is_some_and(|slice| slice.starts_with(['\'', '"']));
        if !valid {
            bail!(
                "zfb bundler: internal error — ?raw import splice range [{lo}..{hi}] \
                 is invalid (source length {}).",
                source.len()
            );
        }
        expanded_source.replace_range(*lo..*hi, replacement);
    }
    edges.sort();
    edges.dedup();

    Ok(RawImportExpansion {
        expanded_source,
        generated_modules: modules.into_values().collect(),
        edges,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(
        importer_source: &str,
        target_name: &str,
        target: &[u8],
    ) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let importer = dir.path().join("entry.tsx");
        std::fs::write(&importer, importer_source).unwrap();
        std::fs::write(dir.path().join(target_name), target).unwrap();
        (dir, importer)
    }

    fn no_exclude(_: &Path) -> bool {
        false
    }

    #[test]
    fn expands_default_raw_import_and_preserves_text_exactly() {
        let source = "import shader from './demo.frag?raw';\nexport { shader };\n";
        let (dir, importer) = fixture(source, "demo.frag", b"line 1\n\"quoted\"\\tail\n");
        let out = expand_raw_imports(source, &importer, dir.path(), &no_exclude).unwrap();
        assert!(!out.expanded_source.contains("?raw"));
        assert_eq!(out.generated_modules.len(), 1);
        let module = &out.generated_modules[0];
        assert!(out
            .expanded_source
            .contains(&format!("./{}", module.filename)));
        assert_eq!(module.target, dir.path().join("demo.frag"));
        assert_eq!(
            module.source,
            "export default \"line 1\\n\\\"quoted\\\"\\\\tail\\n\";\n"
        );
        assert_eq!(out.edges.len(), 1);
    }

    #[test]
    fn js_target_is_terminal_text_not_parsed() {
        let source = "import text from './broken.js?raw';\nexport default text;\n";
        let (dir, importer) = fixture(source, "broken.js", b"this is not valid javascript {{{\n");
        let out = expand_raw_imports(source, &importer, dir.path(), &no_exclude).unwrap();
        assert!(out.generated_modules[0]
            .source
            .contains("this is not valid javascript"));
    }

    #[test]
    fn multiple_imports_are_deterministic_and_deduplicated() {
        let source = "import a from './a.txt?raw';\nimport b from './b.txt?raw';\nimport a2 from './a.txt?raw';\n";
        let (dir, importer) = fixture(source, "a.txt", b"A");
        std::fs::write(dir.path().join("b.txt"), "B").unwrap();
        let first = expand_raw_imports(source, &importer, dir.path(), &no_exclude).unwrap();
        let second = expand_raw_imports(source, &importer, dir.path(), &no_exclude).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.generated_modules.len(), 2);
        assert_eq!(first.edges.len(), 2);
    }

    #[test]
    fn unsupported_queries_and_import_forms_fail_loud() {
        let cases = [
            "import url from './x.txt?url';",
            "import { x } from './x.txt?raw';",
            "import * as x from './x.txt?raw';",
            "import './x.txt?raw';",
            "const x = import('./x.txt?raw');",
            "const x = import(`./x.txt?raw`);",
            "const x = import(`./x.txt?url`);",
            "const x = import('./x.txt?raw' + suffix);",
            "const x = require('./x.txt?raw');",
            "const x = require(`./x.txt?url`);",
            "const x = require(prefix + '?url');",
            "export { default } from './x.txt?raw';",
            "import x from './x.txt?raw&inline';",
            "import type x from './x.txt?raw';",
            "import x from './x.txt?raw' with { type: 'text' };",
        ];
        for source in cases {
            let (dir, importer) = fixture(source, "x.txt", b"x");
            let error = expand_raw_imports(source, &importer, dir.path(), &no_exclude)
                .expect_err(source)
                .to_string();
            assert!(
                error.contains("Only a static default import"),
                "{source}: {error}"
            );
        }
    }

    #[test]
    fn strings_and_comments_are_not_imports() {
        let source = "// import x from './x?raw'\nconst x = './x?url';\n";
        let (dir, importer) = fixture(source, "x", b"x");
        let out = expand_raw_imports(source, &importer, dir.path(), &no_exclude).unwrap();
        assert_eq!(out.expanded_source, source);
        assert!(out.generated_modules.is_empty());
    }

    #[test]
    fn excluded_and_missing_targets_are_clear_errors() {
        let source = "import x from './x.txt?raw';";
        let (dir, importer) = fixture(source, "x.txt", b"x");
        let error = expand_raw_imports(source, &importer, dir.path(), &|_| true)
            .unwrap_err()
            .to_string();
        assert!(error.contains("bundle.exclude"), "{error}");
        std::fs::remove_file(dir.path().join("x.txt")).unwrap();
        let error = expand_raw_imports(source, &importer, dir.path(), &no_exclude)
            .unwrap_err()
            .to_string();
        assert!(error.contains("does not resolve"), "{error}");
    }

    #[test]
    fn non_utf8_target_is_a_named_error() {
        let source = "import x from './x.bin?raw';";
        let (dir, importer) = fixture(source, "x.bin", &[0xff, 0xfe]);
        let error = expand_raw_imports(source, &importer, dir.path(), &no_exclude)
            .unwrap_err()
            .to_string();
        assert!(error.contains("not valid UTF-8"), "{error}");
    }

    #[test]
    fn rejects_lexical_parent_traversal_outside_project_before_read() {
        let outer = tempfile::tempdir().unwrap();
        let root = outer.path().join("project");
        std::fs::create_dir_all(root.join("pages")).unwrap();
        std::fs::write(outer.path().join("secret.txt"), "secret").unwrap();
        let importer = root.join("pages/entry.ts");
        let source = "import secret from '../../secret.txt?raw';\n";
        std::fs::write(&importer, source).unwrap();

        let error = expand_raw_imports(source, &importer, &root, &no_exclude)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("escapes the logical project root"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_target_escape_before_read() {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let importer = project.path().join("entry.ts");
        let source = "import secret from './secret.txt?raw';\n";
        std::fs::write(&importer, source).unwrap();
        let outside_target = outside.path().join("secret.txt");
        std::fs::write(&outside_target, "secret").unwrap();
        std::os::unix::fs::symlink(&outside_target, project.path().join("secret.txt")).unwrap();

        let error = expand_raw_imports(source, &importer, project.path(), &no_exclude)
            .unwrap_err()
            .to_string();
        assert!(error.contains("escapes the project root"), "{error}");
        assert!(error.contains("through a symlink"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn logical_symlinked_project_root_accepts_contained_target() {
        let real = tempfile::tempdir().unwrap();
        std::fs::write(real.path().join("message.txt"), "inside").unwrap();
        std::fs::write(
            real.path().join("entry.ts"),
            "import text from './message.txt?raw';\n",
        )
        .unwrap();
        let holder = tempfile::tempdir().unwrap();
        let logical_root = holder.path().join("project");
        std::os::unix::fs::symlink(real.path(), &logical_root).unwrap();
        let importer = logical_root.join("entry.ts");
        let source = std::fs::read_to_string(&importer).unwrap();

        let expansion = expand_raw_imports(&source, &importer, &logical_root, &no_exclude).unwrap();
        assert_eq!(expansion.edges[0].target, logical_root.join("message.txt"));
    }
}
