//! Module-worker discovery and source rewriting.
//!
//! esbuild's CLI does not treat
//! `new Worker(new URL("./worker.ts", import.meta.url), { type: "module" })`
//! as a child entry. zfb therefore discovers that exact SWC shape, resolves
//! its first-party worker graph, and rewrites only the URL literal to the
//! stable companion contract from `zfb-types`.
//!
//! Worker graphs are browser-only. This module reports typed dependency edges
//! but never injects an `import` for a worker entry, which keeps those sources
//! out of the SSR/server graph. Installed `node_modules` graphs are a
//! deliberate boundary: zfb neither traverses nor rewrites third-party
//! transitive workers.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use swc_core::common::sync::Lrc;
use swc_core::common::{FileName, SourceMap, Spanned};
use swc_core::ecma::ast::{
    Callee, Expr, ExprOrSpread, ImportSpecifier, Lit, MemberProp, MetaPropKind, Module, ModuleDecl,
    ModuleItem, NewExpr, Prop, PropName, PropOrSpread,
};
use swc_core::ecma::parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};
use swc_core::ecma::visit::{Visit, VisitWith};
use zfb_types::{module_worker_content_hash, module_worker_url_specifier, normalize_path_lexical};

/// A direct `new Worker(...)` edge discovered while rewriting a source.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ModuleWorkerEdge {
    /// Module containing the worker constructor.
    pub importer: PathBuf,
    /// Exact project-local worker entry.
    pub source_path: PathBuf,
}

/// A browser-only worker-graph dependency owned by an SSR/client importer.
///
/// Dev invalidation associates the parent importer with every path in the
/// worker closure, but none of these paths becomes an executable SSR import.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ModuleWorkerDependency {
    /// Parent module whose URL rewrite owns this graph.
    pub importer: PathBuf,
    /// Worker entry or first-party transitive dependency contributing to its
    /// `?v=` content hash.
    pub dependency: PathBuf,
}

/// Result of rewriting all supported module-worker constructors in one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleWorkerRewrite {
    /// Source with only matched `new URL(...)` string literals replaced.
    pub expanded_source: String,
    /// Direct and nested worker-entry edges, sorted and deduplicated.
    pub worker_edges: Vec<ModuleWorkerEdge>,
    /// Full first-party worker graph projected back to the rewritten importer.
    pub dependencies: Vec<ModuleWorkerDependency>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConstructorKind {
    Worker,
    SharedWorker,
}

#[derive(Debug, Clone)]
struct ConstructorOccurrence {
    kind: ConstructorKind,
    specifier: String,
    lo: usize,
    hi: usize,
}

fn atom_to_string(value: &swc_core::atoms::Wtf8Atom) -> String {
    value.to_atom_lossy().to_string()
}

fn parse_module(path: &Path, source: &str) -> Result<(Module, u32)> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(
        FileName::Real(path.to_path_buf()).into(),
        source.to_string(),
    );
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
    let module = parser.parse_module().map_err(|error| {
        anyhow!(
            "zfb bundler: failed to parse {} for module-worker discovery: {error:?}",
            path.display()
        )
    })?;
    Ok((module, base))
}

fn literal_url_arg(args: &[ExprOrSpread]) -> Option<&swc_core::ecma::ast::Str> {
    let first = args.first()?;
    if first.spread.is_some() {
        return None;
    }
    let Expr::New(url) = &*first.expr else {
        return None;
    };
    if !matches!(&*url.callee, Expr::Ident(ident) if ident.sym == "URL") {
        return None;
    }
    let url_args = url.args.as_deref()?;
    if url_args.len() < 2 || url_args[0].spread.is_some() || url_args[1].spread.is_some() {
        return None;
    }
    let Expr::Lit(Lit::Str(specifier)) = &*url_args[0].expr else {
        return None;
    };
    let Expr::Member(import_meta_url) = &*url_args[1].expr else {
        return None;
    };
    if !matches!(&import_meta_url.prop, MemberProp::Ident(ident) if ident.sym == "url")
        || !matches!(&*import_meta_url.obj, Expr::MetaProp(meta) if meta.kind == MetaPropKind::ImportMeta)
    {
        return None;
    }
    Some(specifier)
}

fn has_module_options(args: &[ExprOrSpread]) -> bool {
    let Some(options) = args.get(1) else {
        return false;
    };
    if options.spread.is_some() {
        return false;
    }
    let Expr::Object(object) = &*options.expr else {
        return false;
    };
    let mut type_is_module = false;
    for property in &object.props {
        let PropOrSpread::Prop(property) = property else {
            return false;
        };
        let Prop::KeyValue(property) = &**property else {
            return false;
        };
        let is_type = match &property.key {
            PropName::Ident(ident) => ident.sym == "type",
            PropName::Str(value) => value.value == *"type",
            _ => false,
        };
        if is_type {
            type_is_module = matches!(
                &*property.value,
                Expr::Lit(Lit::Str(value)) if value.value == *"module"
            );
        }
    }
    type_is_module
}

fn collect_constructor_occurrences(module: &Module, base: u32) -> Vec<ConstructorOccurrence> {
    struct Collector {
        base: u32,
        occurrences: Vec<ConstructorOccurrence>,
    }

    impl Visit for Collector {
        fn visit_new_expr(&mut self, node: &NewExpr) {
            let kind = match &*node.callee {
                Expr::Ident(ident) if ident.sym == "Worker" => Some(ConstructorKind::Worker),
                Expr::Ident(ident) if ident.sym == "SharedWorker" => {
                    Some(ConstructorKind::SharedWorker)
                }
                _ => None,
            };
            if let (Some(kind), Some(args)) = (kind, node.args.as_deref()) {
                if let Some(specifier) = literal_url_arg(args) {
                    if kind == ConstructorKind::SharedWorker || has_module_options(args) {
                        let span = specifier.span();
                        self.occurrences.push(ConstructorOccurrence {
                            kind,
                            specifier: atom_to_string(&specifier.value),
                            lo: (span.lo.0 - self.base) as usize,
                            hi: (span.hi.0 - self.base) as usize,
                        });
                    }
                }
            }
            node.visit_children_with(self);
        }
    }

    let mut collector = Collector {
        base,
        occurrences: Vec::new(),
    };
    module.visit_with(&mut collector);
    collector.occurrences
}

fn is_js_like(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts")
    )
}

fn is_inside_node_modules(path: &Path) -> bool {
    path.components().any(
        |component| matches!(component, Component::Normal(name) if name == std::ffi::OsStr::new("node_modules")),
    )
}

fn validate_first_party_path(path: &Path, project_root: &Path, context: &str) -> Result<PathBuf> {
    let root = normalize_path_lexical(project_root);
    let logical = normalize_path_lexical(path);
    if !logical.starts_with(&root) || is_inside_node_modules(&logical) {
        bail!(
            "zfb bundler: {context} {} is outside the first-party project root {} or under node_modules",
            logical.display(),
            root.display()
        );
    }
    let canonical_root = project_root
        .canonicalize()
        .with_context(|| format!("canonicalize project root {}", project_root.display()))?;
    let canonical = logical
        .canonicalize()
        .with_context(|| format!("canonicalize {context} {}", logical.display()))?;
    if !canonical.starts_with(&canonical_root) || is_inside_node_modules(&canonical) {
        bail!(
            "zfb bundler: {context} {} escapes project root {} through a symlink or resolves under node_modules (canonical target {})",
            logical.display(),
            project_root.display(),
            canonical.display()
        );
    }
    Ok(logical)
}

fn resolve_worker_target(importer: &Path, specifier: &str, project_root: &Path) -> Result<PathBuf> {
    if !(specifier.starts_with("./") || specifier.starts_with("../"))
        || specifier.contains('?')
        || specifier.contains('#')
    {
        bail!(
            "zfb bundler: unsupported module-worker URL {specifier:?} in {}. The URL must name an exact project-local relative JS/TS file with no query or fragment.",
            importer.display()
        );
    }
    let importer_dir = importer.parent().unwrap_or_else(|| Path::new("."));
    let target = normalize_path_lexical(&importer_dir.join(specifier));
    if !target.is_file() {
        bail!(
            "zfb bundler: module-worker URL {specifier:?} in {} does not resolve to an existing exact file (looked for {})",
            importer.display(),
            target.display()
        );
    }
    let target = validate_first_party_path(&target, project_root, "module-worker source")?;
    if !is_js_like(&target) {
        bail!(
            "zfb bundler: module-worker source {} is not a supported JS/TS entry",
            target.display()
        );
    }
    Ok(target)
}

fn ts_swap_candidates(path: &Path) -> Vec<PathBuf> {
    let swaps: &[&str] = match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("js") => &["ts", "tsx"],
        Some("jsx") => &["tsx"],
        Some("mjs") => &["mts"],
        Some("cjs") => &["cts"],
        _ => &[],
    };
    swaps
        .iter()
        .map(|extension| path.with_extension(extension))
        .collect()
}

fn resolve_graph_import(
    importer: &Path,
    specifier: &str,
    project_root: &Path,
) -> Result<Option<PathBuf>> {
    if !(specifier.starts_with("./") || specifier.starts_with("../")) {
        return Ok(None);
    }
    if specifier.contains('#') {
        bail!(
            "zfb bundler: unsupported fragment-bearing import {specifier:?} in module-worker graph at {}",
            importer.display()
        );
    }
    let importer_dir = importer.parent().unwrap_or_else(|| Path::new("."));
    let mut raw = false;
    let path_specifier = match specifier.split_once('?') {
        None => specifier,
        Some((path, "raw")) if !path.contains('?') => {
            raw = true;
            path
        }
        Some(_) => {
            bail!(
                "zfb bundler: unsupported query-bearing import {specifier:?} in module-worker graph at {}",
                importer.display()
            )
        }
    };
    let candidate = normalize_path_lexical(&importer_dir.join(path_specifier));
    let found = if raw {
        candidate.is_file().then_some(candidate)
    } else {
        ts_swap_candidates(&candidate)
            .into_iter()
            .find(|path| path.is_file())
            .or_else(|| candidate.is_file().then_some(candidate.clone()))
            .or_else(|| {
                [
                    "tsx", "ts", "jsx", "js", "mjs", "cjs", "mts", "cts", "json", "css",
                ]
                .into_iter()
                .map(|extension| {
                    let mut path = candidate.clone();
                    let name = format!(
                        "{}.{}",
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or(""),
                        extension
                    );
                    path.set_file_name(name);
                    path
                })
                .find(|path| path.is_file())
            })
            .or_else(|| {
                candidate.is_dir().then(|| {
                    ["tsx", "ts", "jsx", "js", "mjs", "cjs", "mts", "cts"]
                        .into_iter()
                        .map(|extension| candidate.join(format!("index.{extension}")))
                        .find(|path| path.is_file())
                })?
            })
    };
    let Some(found) = found else {
        return Ok(None);
    };
    let resolves_under_node_modules = is_inside_node_modules(&found)
        || found
            .canonicalize()
            .ok()
            .is_some_and(|canonical| is_inside_node_modules(&canonical));
    if resolves_under_node_modules {
        return Ok(None);
    }
    match validate_first_party_path(&found, project_root, "module-worker dependency") {
        Ok(path) => Ok(Some(path)),
        Err(error) => Err(error),
    }
}

fn collect_import_specifiers(module: &Module) -> Vec<String> {
    let mut specifiers = Vec::new();
    for item in &module.body {
        let ModuleItem::ModuleDecl(declaration) = item else {
            continue;
        };
        match declaration {
            ModuleDecl::Import(import) if !import.type_only => {
                // Type-only named specifiers in an otherwise-runtime import do
                // not make the source edge disappear, so only the declaration
                // level `import type` bit is relevant here.
                let runtime = import.specifiers.iter().any(|specifier| {
                    !matches!(specifier, ImportSpecifier::Named(named) if named.is_type_only)
                });
                if runtime || import.specifiers.is_empty() {
                    specifiers.push(atom_to_string(&import.src.value));
                }
            }
            ModuleDecl::ExportNamed(export) if !export.type_only => {
                if let Some(source) = &export.src {
                    specifiers.push(atom_to_string(&source.value));
                }
            }
            ModuleDecl::ExportAll(export) if !export.type_only => {
                specifiers.push(atom_to_string(&export.src.value));
            }
            _ => {}
        }
    }

    struct DynamicImports {
        specifiers: Vec<String>,
    }
    impl Visit for DynamicImports {
        fn visit_call_expr(&mut self, node: &swc_core::ecma::ast::CallExpr) {
            if matches!(node.callee, Callee::Import(_)) {
                if let Some(argument) = node.args.first() {
                    if argument.spread.is_none() {
                        if let Expr::Lit(Lit::Str(value)) = &*argument.expr {
                            self.specifiers.push(atom_to_string(&value.value));
                        }
                    }
                }
            }
            node.visit_children_with(self);
        }
    }
    let mut dynamic = DynamicImports {
        specifiers: Vec::new(),
    };
    module.visit_with(&mut dynamic);
    specifiers.extend(dynamic.specifiers);
    specifiers
}

struct WorkerGraph {
    hash: String,
    worker_edges: BTreeSet<ModuleWorkerEdge>,
    files: BTreeSet<PathBuf>,
}

fn inspect_worker_graph(entry: &Path, project_root: &Path) -> Result<WorkerGraph> {
    let mut visited = BTreeSet::new();
    let mut stack = vec![entry.to_path_buf()];
    let mut file_bytes: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();
    let mut worker_edges = BTreeSet::new();

    while let Some(current) = stack.pop() {
        if !visited.insert(current.clone()) {
            continue;
        }
        let bytes = std::fs::read(&current)
            .with_context(|| format!("read module-worker dependency {}", current.display()))?;
        file_bytes.insert(current.clone(), bytes.clone());
        if !is_js_like(&current) {
            continue;
        }
        let source = String::from_utf8(bytes).map_err(|error| {
            anyhow!(
                "zfb bundler: module-worker source {} is not valid UTF-8: {error}",
                current.display()
            )
        })?;
        let (module, base) = parse_module(&current, &source)?;
        for occurrence in collect_constructor_occurrences(&module, base) {
            if occurrence.kind == ConstructorKind::SharedWorker {
                bail!(
                    "zfb bundler: unsupported SharedWorker in {} for {:?}. Only module `Worker` constructors are supported.",
                    current.display(),
                    occurrence.specifier
                );
            }
            let nested = resolve_worker_target(&current, &occurrence.specifier, project_root)?;
            worker_edges.insert(ModuleWorkerEdge {
                importer: current.clone(),
                source_path: nested.clone(),
            });
            if !visited.contains(&nested) {
                stack.push(nested);
            }
        }
        for specifier in collect_import_specifiers(&module) {
            if let Some(dependency) = resolve_graph_import(&current, &specifier, project_root)? {
                if !visited.contains(&dependency) {
                    stack.push(dependency);
                }
            }
        }
    }

    // Length-prefix every path and body so concatenation is unambiguous. Paths
    // are project-relative and slash-normalized, making the cache key stable
    // across worktree locations and host operating systems.
    let root = normalize_path_lexical(project_root);
    let mut aggregate = Vec::new();
    for (path, bytes) in &file_bytes {
        let relative = path.strip_prefix(&root).map_err(|_| {
            anyhow!(
                "zfb bundler: worker dependency {} lost project-root containment",
                path.display()
            )
        })?;
        let relative = relative
            .components()
            .filter_map(|component| match component {
                Component::Normal(value) => Some(value.to_string_lossy()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/");
        aggregate.extend_from_slice(&(relative.len() as u64).to_le_bytes());
        aggregate.extend_from_slice(relative.as_bytes());
        aggregate.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        aggregate.extend_from_slice(bytes);
    }
    Ok(WorkerGraph {
        hash: module_worker_content_hash(&aggregate),
        worker_edges,
        files: file_bytes.into_keys().collect(),
    })
}

/// Rewrite supported module-worker URLs in a JS/TS source module.
///
/// Only the first string-literal argument of the exact
/// `new Worker(new URL("./x.ts", import.meta.url), { type: "module" })`
/// shape changes. The replacement is
/// `./worker-<sanitized-relative-path>-<path-hash>.js?v=<graph-hash>`.
/// The filename is stable and CSP-matchable; the query changes when the entry,
/// a first-party transitive import, or a nested worker changes.
///
/// No imports are injected. Worker entries are browser-only and remain absent
/// from the SSR graph. `SharedWorker` in the same literal URL shape is a named
/// hard error. Constructors reached only through installed `node_modules` are
/// outside this first-party pre-pass and intentionally skipped by callers.
pub fn rewrite_module_worker_urls(
    source: &str,
    importer: &Path,
    project_root: &Path,
) -> Result<ModuleWorkerRewrite> {
    if !source.contains("Worker") || is_inside_node_modules(&normalize_path_lexical(importer)) {
        return Ok(ModuleWorkerRewrite {
            expanded_source: source.to_string(),
            worker_edges: Vec::new(),
            dependencies: Vec::new(),
        });
    }
    validate_first_party_path(importer, project_root, "module-worker importer")?;
    let (module, base) = parse_module(importer, source)?;
    let occurrences = collect_constructor_occurrences(&module, base);
    if occurrences.is_empty() {
        return Ok(ModuleWorkerRewrite {
            expanded_source: source.to_string(),
            worker_edges: Vec::new(),
            dependencies: Vec::new(),
        });
    }

    let mut replacements = Vec::new();
    let mut worker_edges = BTreeSet::new();
    let mut dependencies = BTreeSet::new();
    for occurrence in occurrences {
        if occurrence.kind == ConstructorKind::SharedWorker {
            bail!(
                "zfb bundler: unsupported SharedWorker in {} for {:?}. Only `new Worker(new URL(\"./worker.ts\", import.meta.url), {{ type: \"module\" }})` is supported.",
                importer.display(),
                occurrence.specifier
            );
        }
        let worker = resolve_worker_target(importer, &occurrence.specifier, project_root)?;
        let graph = inspect_worker_graph(&worker, project_root)?;
        let rewritten = module_worker_url_specifier(project_root, &worker, &graph.hash)
            .map_err(|error| anyhow!("zfb bundler: {error}"))?;
        replacements.push((
            occurrence.lo,
            occurrence.hi,
            serde_json::to_string(&rewritten).context("serialize module-worker URL")?,
        ));
        worker_edges.insert(ModuleWorkerEdge {
            importer: importer.to_path_buf(),
            source_path: worker,
        });
        worker_edges.extend(graph.worker_edges);
        dependencies.extend(
            graph
                .files
                .into_iter()
                .map(|dependency| ModuleWorkerDependency {
                    importer: importer.to_path_buf(),
                    dependency,
                }),
        );
    }

    let mut expanded_source = source.to_string();
    for (lo, hi, replacement) in replacements.into_iter().rev() {
        let valid = source
            .get(lo..hi)
            .is_some_and(|slice| slice.starts_with(['\'', '"']));
        if !valid {
            bail!(
                "zfb bundler: internal module-worker span mismatch in {}",
                importer.display()
            );
        }
        expanded_source.replace_range(lo..hi, &replacement);
    }
    Ok(ModuleWorkerRewrite {
        expanded_source,
        worker_edges: worker_edges.into_iter().collect(),
        dependencies: dependencies.into_iter().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, source: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }

    #[test]
    fn rewrite_is_span_local_and_uses_stable_filename_with_graph_hash() {
        let project = tempfile::tempdir().unwrap();
        let importer = project.path().join("src/Island.tsx");
        let worker = project.path().join("src/workers/search.ts");
        write(&importer, "placeholder");
        write(&worker, "self.postMessage('ready');");
        let source = "const url = './workers/search.ts';\nnew Worker(new URL('./workers/search.ts', import.meta.url), { name: 'search', type: 'module' });\n";
        let rewrite = rewrite_module_worker_urls(source, &importer, project.path()).unwrap();
        assert!(rewrite
            .expanded_source
            .contains("const url = './workers/search.ts'"));
        assert!(rewrite
            .expanded_source
            .contains("new Worker(new URL(\"./worker-src-workers-search-ts-"));
        assert!(rewrite.expanded_source.contains(".js?v="));
        assert!(rewrite
            .expanded_source
            .contains("{ name: 'search', type: 'module' }"));
        assert_eq!(
            rewrite.worker_edges,
            vec![ModuleWorkerEdge {
                importer,
                source_path: worker,
            }]
        );
    }

    #[test]
    fn transitive_and_nested_edits_change_parent_cache_query() {
        let project = tempfile::tempdir().unwrap();
        let importer = project.path().join("src/app.client.ts");
        let worker = project.path().join("src/worker.ts");
        let helper = project.path().join("src/helper.ts");
        let nested = project.path().join("src/nested.ts");
        write(&importer, "placeholder");
        write(
            &worker,
            "import { value } from './helper'; new Worker(new URL('./nested.ts', import.meta.url), { type: 'module' }); self.postMessage(value);",
        );
        write(&helper, "export const value = 'a';");
        write(&nested, "self.postMessage('nested-a');");
        let source = "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });";
        let first = rewrite_module_worker_urls(source, &importer, project.path()).unwrap();
        write(&helper, "export const value = 'b';");
        let transitive = rewrite_module_worker_urls(source, &importer, project.path()).unwrap();
        assert_ne!(first.expanded_source, transitive.expanded_source);
        write(&nested, "self.postMessage('nested-b');");
        let nested_changed = rewrite_module_worker_urls(source, &importer, project.path()).unwrap();
        assert_ne!(transitive.expanded_source, nested_changed.expanded_source);
        assert!(first.worker_edges.contains(&ModuleWorkerEdge {
            importer: worker,
            source_path: nested,
        }));
        assert!(first
            .dependencies
            .iter()
            .any(|edge| edge.dependency == helper));
    }

    #[test]
    fn shared_worker_is_a_named_hard_error() {
        let project = tempfile::tempdir().unwrap();
        let importer = project.path().join("src/app.ts");
        let worker = project.path().join("src/worker.ts");
        write(&importer, "placeholder");
        write(&worker, "self.onconnect = () => {};");
        let error = rewrite_module_worker_urls(
            "new SharedWorker(new URL('./worker.ts', import.meta.url));",
            &importer,
            project.path(),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("unsupported SharedWorker"), "{error}");
        assert!(error.contains("worker.ts"), "{error}");
    }

    #[test]
    fn rejects_non_exact_and_node_modules_worker_targets() {
        let project = tempfile::tempdir().unwrap();
        let importer = project.path().join("src/app.ts");
        write(&importer, "placeholder");
        write(
            &project.path().join("node_modules/pkg/worker.ts"),
            "self.postMessage(1);",
        );
        for specifier in [
            "./missing",
            "./worker.ts?v=1",
            "/absolute.ts",
            "../node_modules/pkg/worker.ts",
        ] {
            let source = format!(
                "new Worker(new URL({specifier:?}, import.meta.url), {{ type: 'module' }});"
            );
            assert!(
                rewrite_module_worker_urls(&source, &importer, project.path()).is_err(),
                "{specifier} must be rejected"
            );
        }
    }

    #[test]
    fn installed_node_modules_importer_is_left_for_third_party_tooling() {
        let project = tempfile::tempdir().unwrap();
        let importer = project.path().join("node_modules/pkg/index.js");
        write(&importer, "placeholder");
        let source = "new SharedWorker(new URL('./worker.js', import.meta.url));";
        let rewrite = rewrite_module_worker_urls(source, &importer, project.path()).unwrap();
        assert_eq!(rewrite.expanded_source, source);
        assert!(rewrite.worker_edges.is_empty());
        assert!(rewrite.dependencies.is_empty());
    }

    #[test]
    fn non_module_or_overridden_worker_options_remain_untouched() {
        let project = tempfile::tempdir().unwrap();
        let importer = project.path().join("src/app.ts");
        let worker = project.path().join("src/worker.ts");
        write(&importer, "placeholder");
        write(&worker, "self.postMessage(1);");
        for source in [
            "new Worker(new URL('./worker.ts', import.meta.url));",
            "new Worker(new URL('./worker.ts', import.meta.url), { type: 'classic' });",
            "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module', type: 'classic' });",
            "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module', ...options });",
        ] {
            let rewrite = rewrite_module_worker_urls(source, &importer, project.path()).unwrap();
            assert_eq!(rewrite.expanded_source, source);
            assert!(rewrite.worker_edges.is_empty());
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_worker_symlink_escape_without_changing_logical_naming() {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let importer = project.path().join("src/app.ts");
        let alias = project.path().join("src/worker.ts");
        let escaped = outside.path().join("worker.ts");
        write(&importer, "placeholder");
        write(&escaped, "self.postMessage(1);");
        std::os::unix::fs::symlink(&escaped, &alias).unwrap();
        let error = rewrite_module_worker_urls(
            "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });",
            &importer,
            project.path(),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("escapes project root"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn worker_graph_skips_symlinked_installed_dependency() {
        let project = tempfile::tempdir().unwrap();
        let importer = project.path().join("src/app.ts");
        let worker = project.path().join("src/worker.ts");
        let package = project.path().join("node_modules/pkg");
        write(&importer, "placeholder");
        write(
            &worker,
            "import './vendor/helper.js'; self.postMessage('ready');",
        );
        write(&package.join("helper.js"), "thirdPartySideEffect();");
        std::os::unix::fs::symlink(&package, project.path().join("src/vendor")).unwrap();

        let rewrite = rewrite_module_worker_urls(
            "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });",
            &importer,
            project.path(),
        )
        .unwrap();
        assert_eq!(rewrite.dependencies.len(), 1);
        assert_eq!(rewrite.dependencies[0].dependency, worker);
    }
}
