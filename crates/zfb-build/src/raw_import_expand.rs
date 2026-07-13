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
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use swc_core::common::sync::Lrc;
use swc_core::common::{FileName, SourceMap, Spanned};
use swc_core::ecma::ast::{Callee, Expr, ImportSpecifier, Module, ModuleDecl, ModuleItem};
use swc_core::ecma::parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};
use swc_core::ecma::visit::{Visit, VisitWith};
use zfb_plugin_resolver::is_relative_specifier;
pub use zfb_plugin_resolver::{
    resolve_raw_target, resolve_raw_target_with_aliases, validate_raw_candidate,
    RawImportAliasContext,
};
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

#[derive(Debug, Clone, Copy)]
enum RawScanMode {
    Tolerant,
    Validation,
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

fn parse_as_tsx(importer: &Path) -> bool {
    !matches!(
        importer
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("ts" | "mts" | "cts")
    )
}

fn parse_module(importer: &Path, source: &str) -> Result<(Module, u32)> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(
        FileName::Real(importer.to_path_buf()).into(),
        source.to_string(),
    );
    let base = fm.start_pos.0;
    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax {
            tsx: parse_as_tsx(importer),
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

fn collect_raw_occurrences(
    source: &str,
    importer: &Path,
    mode: RawScanMode,
) -> Result<Vec<RawOccurrence>> {
    if !source.contains('?') {
        return Ok(Vec::new());
    }

    let (module, base) = match parse_module(importer, source) {
        Ok(parsed) => parsed,
        Err(error) => match mode {
            RawScanMode::Tolerant => {
                // Source scans are best-effort: esbuild remains the syntax
                // oracle and will report a real query import if this file is
                // selected by the final bundle.
                return Ok(Vec::new());
            }
            RawScanMode::Validation => return Err(error),
        },
    };
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

/// Inspect every static/type-only/re-export/dynamic import and CommonJS
/// require expression for query syntax without reading or rewriting targets.
///
/// `Ok(Some(specifier))` means the source contains the one normally-supported
/// static default `?raw` form and returns its exact spelling for diagnostics.
/// Unsupported query shapes return the same named error as
/// [`expand_raw_imports`]. Virtual-module validation uses this broader AST
/// pass because virtual sources cannot support even the otherwise-valid form.
pub fn supported_raw_import_specifier_for_path(
    source: &str,
    importer: &Path,
) -> Result<Option<String>> {
    Ok(
        collect_raw_occurrences(source, importer, RawScanMode::Validation)?
            .into_iter()
            .next()
            .map(|occurrence| occurrence.specifier),
    )
}

pub fn supported_raw_import_specifier(source: &str) -> Result<Option<String>> {
    supported_raw_import_specifier_for_path(source, Path::new(".zfb-virtual-module.mjs"))
}

fn generated_filename(specifier_without_query: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(specifier_without_query.as_bytes());
    let digest = hex::encode(hasher.finalize());
    format!(".zfb-raw-{}.mjs", &digest[..16])
}

fn lexical_relative_path(target: &Path, base: &Path) -> Option<PathBuf> {
    let target = normalize_path_lexical(target);
    let base = normalize_path_lexical(base);
    let target_components = target.components().collect::<Vec<_>>();
    let base_components = base.components().collect::<Vec<_>>();
    if target_components.first() != base_components.first() {
        return None;
    }

    let mut shared = 0;
    while shared < target_components.len()
        && shared < base_components.len()
        && target_components[shared] == base_components[shared]
    {
        shared += 1;
    }

    let mut out = PathBuf::new();
    for component in &base_components[shared..] {
        match component {
            Component::Normal(_) => out.push(".."),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) | Component::ParentDir => return None,
        }
    }
    for component in &target_components[shared..] {
        match component {
            Component::Normal(value) => out.push(value),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) | Component::ParentDir => return None,
        }
    }
    Some(out)
}

fn generated_key_for_target(target_specifier: &str, importer_dir: &Path, target: &Path) -> String {
    if is_relative_specifier(target_specifier) {
        return target_specifier.to_string();
    }
    let Some(relative) = lexical_relative_path(target, importer_dir) else {
        return normalize_path_lexical(target)
            .to_string_lossy()
            .replace('\\', "/");
    };
    let mut key = relative.to_string_lossy().replace('\\', "/");
    if !key.starts_with("../") {
        key.insert_str(0, "./");
    }
    key
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
    let aliases = RawImportAliasContext::from_project_root(project_root);
    expand_raw_imports_with_aliases(source, importer, project_root, &aliases, is_excluded)
}

pub fn expand_raw_imports_with_aliases(
    source: &str,
    importer: &Path,
    project_root: &Path,
    aliases: &RawImportAliasContext,
    is_excluded: &dyn Fn(&Path) -> bool,
) -> Result<RawImportExpansion> {
    let occurrences = collect_raw_occurrences(source, importer, RawScanMode::Tolerant)?;
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
        let target =
            resolve_raw_target_with_aliases(importer, target_specifier, project_root, aliases)?;
        if is_excluded(&target) {
            bail!(
                "zfb bundler: raw import target {} (imported from {}) is excluded by \
                 `bundle.exclude`. A `?raw` target is a required terminal dependency; \
                 remove the matching exclude pattern or remove the import.",
                target.display(),
                importer.display()
            );
        }

        let filename_key = generated_key_for_target(target_specifier, importer_dir, &target);
        let filename = generated_filename(&filename_key);
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
        fixture_with_importer("entry.tsx", importer_source, target_name, target)
    }

    fn fixture_with_importer(
        importer_name: &str,
        importer_source: &str,
        target_name: &str,
        target: &[u8],
    ) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let importer = dir.path().join(importer_name);
        std::fs::write(&importer, importer_source).unwrap();
        std::fs::write(dir.path().join(target_name), target).unwrap();
        (dir, importer)
    }

    fn no_exclude(_: &Path) -> bool {
        false
    }

    fn alias_context(
        paths: impl IntoIterator<Item = (&'static str, Vec<String>)>,
        base_url: Option<PathBuf>,
    ) -> RawImportAliasContext {
        RawImportAliasContext::from_paths_and_base_url(
            &paths
                .into_iter()
                .map(|(pattern, targets)| (pattern.to_string(), targets))
                .collect(),
            base_url,
        )
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
    fn ts_importer_generic_arrow_with_query_text_passes_through() {
        let source = "const o = { m: async <T>() => {} };\nconst q = '?';\nexport { o, q };\n";
        let (dir, importer) = fixture_with_importer("entry.ts", source, "unused.txt", b"unused");
        let out = expand_raw_imports(source, &importer, dir.path(), &no_exclude).unwrap();
        assert_eq!(out.expanded_source, source);
        assert!(out.generated_modules.is_empty());
    }

    #[test]
    fn ts_importer_generic_arrow_with_raw_import_expands() {
        let source = "import text from './message.txt?raw';\nconst o = { m: async <T>() => {} };\nexport { text, o };\n";
        let (dir, importer) =
            fixture_with_importer("entry.ts", source, "message.txt", b"hello from raw");
        let out = expand_raw_imports(source, &importer, dir.path(), &no_exclude).unwrap();
        assert!(!out.expanded_source.contains("?raw"));
        assert_eq!(out.generated_modules.len(), 1);
        assert!(out.generated_modules[0].source.contains("hello from raw"));
    }

    #[test]
    fn tsx_importer_with_jsx_still_expands() {
        let source = "import text from './message.txt?raw';\nconst node = <span>{text}</span>;\nexport default node;\n";
        let (dir, importer) = fixture_with_importer("entry.tsx", source, "message.txt", b"jsx raw");
        let out = expand_raw_imports(source, &importer, dir.path(), &no_exclude).unwrap();
        assert!(!out.expanded_source.contains("?raw"));
        assert_eq!(out.generated_modules.len(), 1);
    }

    #[test]
    fn unparseable_query_text_passes_through_for_expansion() {
        let source = "const broken = ;\n// import text from './message.txt?raw'\n";
        let (dir, importer) = fixture_with_importer("entry.ts", source, "message.txt", b"unused");
        let out = expand_raw_imports(source, &importer, dir.path(), &no_exclude).unwrap();
        assert_eq!(out.expanded_source, source);
        assert!(out.generated_modules.is_empty());
        assert!(out.edges.is_empty());
    }

    #[test]
    fn validation_scan_still_fails_loud_on_parse_errors() {
        let source = "const broken = ;\n// import text from './message.txt?raw'\n";
        let error =
            supported_raw_import_specifier_for_path(source, Path::new("entry.ts")).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("failed to parse module for ?raw expansion"),
            "{error:#}"
        );
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

    #[test]
    fn aliased_raw_import_expands_like_equivalent_relative_spelling() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        std::fs::create_dir_all(root.join("src/assets")).unwrap();
        let importer = root.join("src/entry.ts");
        let target = root.join("src/assets/icon.svg");
        std::fs::write(&importer, "placeholder").unwrap();
        std::fs::write(&target, "<svg>zfb</svg>").unwrap();
        let aliases = alias_context(
            [(
                "@/*",
                vec![root.join("src/*").to_string_lossy().into_owned()],
            )],
            None,
        );
        let aliased = "import text from '@/assets/icon.svg?raw';\nexport default text;\n";
        let relative = "import text from './assets/icon.svg?raw';\nexport default text;\n";

        let aliased =
            expand_raw_imports_with_aliases(aliased, &importer, root, &aliases, &no_exclude)
                .unwrap();
        let relative =
            expand_raw_imports_with_aliases(relative, &importer, root, &aliases, &no_exclude)
                .unwrap();

        assert_eq!(aliased.expanded_source, relative.expanded_source);
        assert_eq!(aliased.generated_modules, relative.generated_modules);
        assert_eq!(aliased.edges, relative.edges);
        assert_eq!(aliased.edges, vec![RawImportEdge { importer, target }]);
    }
}
