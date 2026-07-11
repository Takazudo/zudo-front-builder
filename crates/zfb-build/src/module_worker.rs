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
use swc_core::common::{FileName, Globals, Mark, SourceMap, Spanned, SyntaxContext, GLOBALS};
use swc_core::ecma::ast::{
    Callee, Expr, ExprOrSpread, ImportSpecifier, Lit, MemberProp, MetaPropKind, Module, ModuleDecl,
    ModuleItem, NewExpr, Program, Prop, PropName, PropOrSpread,
};
use swc_core::ecma::parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{Visit, VisitWith};
use zfb_plugin_resolver::{read_tsconfig_paths, TsConfigPaths};
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

fn parse_module(path: &Path, source: &str) -> Result<(Module, u32, SyntaxContext)> {
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
    let globals = Globals::new();
    let (module, unresolved_ctxt) = GLOBALS.set(&globals, || {
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        let program =
            Program::Module(module).apply(resolver(unresolved_mark, top_level_mark, true));
        let module = match program {
            Program::Module(module) => module,
            Program::Script(_) => unreachable!("the parser produced an ES module"),
        };
        (module, SyntaxContext::empty().apply_mark(unresolved_mark))
    });
    Ok((module, base, unresolved_ctxt))
}

fn literal_url_arg(
    args: &[ExprOrSpread],
    unresolved_ctxt: SyntaxContext,
) -> Option<&swc_core::ecma::ast::Str> {
    let first = args.first()?;
    if first.spread.is_some() {
        return None;
    }
    let Expr::New(url) = &*first.expr else {
        return None;
    };
    if !matches!(&*url.callee, Expr::Ident(ident) if ident.sym == "URL" && ident.ctxt == unresolved_ctxt)
    {
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
            PropName::Computed(computed) => match &*computed.expr {
                Expr::Lit(Lit::Str(value)) => value.value == *"type",
                // A runtime-computed key might overwrite `type`; claiming the
                // constructor would risk rewriting a classic worker.
                _ => return false,
            },
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

fn collect_constructor_occurrences(
    module: &Module,
    base: u32,
    unresolved_ctxt: SyntaxContext,
) -> Vec<ConstructorOccurrence> {
    struct Collector {
        base: u32,
        unresolved_ctxt: SyntaxContext,
        occurrences: Vec<ConstructorOccurrence>,
    }

    impl Visit for Collector {
        fn visit_new_expr(&mut self, node: &NewExpr) {
            let kind = match &*node.callee {
                Expr::Ident(ident)
                    if ident.sym == "Worker" && ident.ctxt == self.unresolved_ctxt =>
                {
                    Some(ConstructorKind::Worker)
                }
                Expr::Ident(ident)
                    if ident.sym == "SharedWorker" && ident.ctxt == self.unresolved_ctxt =>
                {
                    Some(ConstructorKind::SharedWorker)
                }
                _ => None,
            };
            if let (Some(kind), Some(args)) = (kind, node.args.as_deref()) {
                if let Some(specifier) = literal_url_arg(args, self.unresolved_ctxt) {
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
        unresolved_ctxt,
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

fn is_css_like(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("css"))
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

fn probe_graph_candidate(candidate: &Path, exact: bool) -> Option<PathBuf> {
    if exact {
        return candidate.is_file().then(|| candidate.to_path_buf());
    }
    ts_swap_candidates(candidate)
        .into_iter()
        .find(|path| path.is_file())
        .or_else(|| candidate.is_file().then(|| candidate.to_path_buf()))
        .or_else(|| {
            [
                "tsx", "ts", "jsx", "js", "mjs", "cjs", "mts", "cts", "json", "css",
            ]
            .into_iter()
            .map(|extension| {
                let mut path = candidate.to_path_buf();
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
                ["tsx", "ts", "jsx", "js", "mjs", "cjs", "mts", "cts", "css"]
                    .into_iter()
                    .map(|extension| candidate.join(format!("index.{extension}")))
                    .find(|path| path.is_file())
            })?
        })
}

fn match_tsconfig_pattern(pattern: &str, specifier: &str) -> Option<Option<String>> {
    match pattern.matches('*').count() {
        0 => (pattern == specifier).then_some(None),
        1 => {
            let star = pattern.find('*')?;
            let prefix = &pattern[..star];
            let suffix = &pattern[star + 1..];
            if specifier.len() < prefix.len() + suffix.len()
                || !specifier.starts_with(prefix)
                || !specifier.ends_with(suffix)
            {
                return None;
            }
            Some(Some(
                specifier[prefix.len()..specifier.len().saturating_sub(suffix.len())].to_string(),
            ))
        }
        _ => None,
    }
}

fn tsconfig_pattern_specificity(pattern: &str) -> usize {
    pattern
        .find('*')
        .map(|star| star.max(pattern.len().saturating_sub(star + 1)))
        .unwrap_or(pattern.len())
}

fn substitute_tsconfig_target(target: &str, capture: Option<&str>) -> String {
    match (target.find('*'), capture) {
        (Some(star), Some(capture)) => {
            let mut out = String::with_capacity(target.len() + capture.len());
            out.push_str(&target[..star]);
            out.push_str(capture);
            out.push_str(&target[star + 1..]);
            out
        }
        _ => target.to_string(),
    }
}

fn resolve_tsconfig_graph_alias(
    paths: Option<&TsConfigPaths>,
    specifier: &str,
) -> Result<Option<Option<PathBuf>>> {
    let Some(paths) = paths else {
        return Ok(None);
    };
    let mut matches = paths
        .aliases
        .iter()
        .filter_map(|alias| {
            match_tsconfig_pattern(&alias.pattern, specifier)
                .map(|capture| (alias, capture, tsconfig_pattern_specificity(&alias.pattern)))
        })
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return Ok(None);
    }
    matches.sort_by_key(|(_, _, specificity)| std::cmp::Reverse(*specificity));
    for (alias, capture, _) in matches {
        for target in &alias.targets {
            let target = substitute_tsconfig_target(target, capture.as_deref());
            let candidate = normalize_path_lexical(&paths.base_dir.join(target));
            if let Some(found) = probe_graph_candidate(&candidate, false) {
                return Ok(Some(Some(found)));
            }
        }
    }
    Ok(Some(None))
}

fn resolve_tsconfig_base_url(paths: Option<&TsConfigPaths>, specifier: &str) -> Option<PathBuf> {
    let base_url = paths?.base_url.as_ref()?;
    probe_graph_candidate(&normalize_path_lexical(&base_url.join(specifier)), false)
}

fn bare_package_name(specifier: &str) -> Option<PathBuf> {
    let mut parts = specifier.split('/');
    let first = parts.next()?;
    if first.is_empty() {
        return None;
    }
    if first.starts_with('@') {
        let second = parts.next()?;
        Some(PathBuf::from(first).join(second))
    } else {
        Some(PathBuf::from(first))
    }
}

fn installed_package_exists(importer_dir: &Path, project_root: &Path, specifier: &str) -> bool {
    let Some(package) = bare_package_name(specifier) else {
        return false;
    };
    let mut current = Some(importer_dir);
    while let Some(dir) = current {
        if dir.join("node_modules").join(&package).exists() {
            return true;
        }
        current = dir.parent();
    }
    project_root.join("node_modules").join(package).exists()
}

struct ProjectGraphResolver {
    project_root: PathBuf,
    tsconfig_paths: Option<TsConfigPaths>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GraphSpecifierKind {
    JavaScript,
    CssImport,
    CssUrl,
}

fn has_css_url_scheme(specifier: &str) -> bool {
    let Some((scheme, _)) = specifier.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|ch| ch.is_ascii_alphabetic())
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
}

impl ProjectGraphResolver {
    fn new(project_root: &Path) -> Self {
        Self {
            project_root: project_root.to_path_buf(),
            tsconfig_paths: read_tsconfig_paths(project_root),
        }
    }

    fn resolve(
        &self,
        importer: &Path,
        specifier: &str,
        kind: GraphSpecifierKind,
    ) -> Result<Option<PathBuf>> {
        if specifier.starts_with("http://")
            || specifier.starts_with("https://")
            || specifier.starts_with("data:")
            || specifier.starts_with("//")
        {
            return Ok(None);
        }
        if kind != GraphSpecifierKind::JavaScript && has_css_url_scheme(specifier) {
            return Ok(None);
        }
        if kind != GraphSpecifierKind::JavaScript
            && (specifier.starts_with('/') || specifier.starts_with('#'))
        {
            return Ok(None);
        }
        if kind == GraphSpecifierKind::JavaScript
            && specifier.find('#').is_some_and(|fragment| fragment != 0)
        {
            bail!(
                "zfb bundler: unsupported fragment-bearing import {specifier:?} in module-worker graph at {}",
                importer.display()
            );
        }
        let importer_dir = importer.parent().unwrap_or_else(|| Path::new("."));
        let mut raw = false;
        let path_specifier = if kind == GraphSpecifierKind::JavaScript {
            match specifier.split_once('?') {
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
            }
        } else {
            let suffix = specifier
                .char_indices()
                .find_map(|(index, ch)| matches!(ch, '?' | '#').then_some(index))
                .unwrap_or(specifier.len());
            let path = &specifier[..suffix];
            if path.is_empty() {
                bail!(
                    "zfb bundler: CSS reference {specifier:?} in module-worker graph at {} has no local file path",
                    importer.display()
                )
            }
            path
        };
        let css_plain_relative = kind == GraphSpecifierKind::CssImport
            && !path_specifier.starts_with('@')
            && !path_specifier.contains('/')
            && Path::new(path_specifier).extension().is_some();
        let is_relative = path_specifier.starts_with("./")
            || path_specifier.starts_with("../")
            || css_plain_relative
            || kind == GraphSpecifierKind::CssUrl;
        let found = if is_relative {
            let candidate = normalize_path_lexical(&importer_dir.join(path_specifier));
            probe_graph_candidate(&candidate, raw).ok_or_else(|| {
                anyhow!(
                    "zfb bundler: module-worker dependency {specifier:?} in {} cannot be resolved exactly enough to produce a safe cache key (looked from {})",
                    importer.display(),
                    importer_dir.display()
                )
            })?
        } else {
            if path_specifier.starts_with('/') {
                bail!(
                    "zfb bundler: absolute module-worker dependency {specifier:?} in {} is outside the project graph contract",
                    importer.display()
                );
            }
            let alias_resolution =
                resolve_tsconfig_graph_alias(self.tsconfig_paths.as_ref(), path_specifier)?;
            if let Some(Some(found)) = alias_resolution.as_ref() {
                found.clone()
            } else if alias_resolution.is_some() {
                bail!(
                    "zfb bundler: tsconfig path alias {specifier:?} imported by {} matched a first-party mapping but no target resolved; refusing to produce a stale worker cache key",
                    importer.display()
                )
            } else if let Some(found) =
                resolve_tsconfig_base_url(self.tsconfig_paths.as_ref(), path_specifier)
            {
                found
            } else if path_specifier.starts_with("node:")
                || installed_package_exists(importer_dir, &self.project_root, path_specifier)
            {
                return Ok(None);
            } else {
                bail!(
                    "zfb bundler: non-relative dependency {specifier:?} imported by {} is neither a resolvable project tsconfig alias/baseUrl file nor an installed package; refusing to omit it from the worker cache key",
                    importer.display()
                )
            }
        };
        let resolves_under_node_modules = is_inside_node_modules(&found)
            || found
                .canonicalize()
                .ok()
                .is_some_and(|canonical| is_inside_node_modules(&canonical));
        if resolves_under_node_modules {
            return Ok(None);
        }
        match validate_first_party_path(&found, &self.project_root, "module-worker dependency") {
            Ok(path) => Ok(Some(path)),
            Err(error) => Err(error),
        }
    }
}

#[derive(Default)]
struct CssReferences {
    imports: Vec<String>,
    urls: Vec<String>,
}

fn collect_css_references(css: &str, path: &Path) -> Result<CssReferences> {
    let without_comments = strip_css_block_comments(css);
    let bytes = without_comments.as_bytes();
    let mut references = CssReferences::default();
    let mut index = 0;
    while index < bytes.len() {
        if matches!(bytes[index], b'\'' | b'"') {
            index = skip_css_string(&without_comments, index).ok_or_else(|| {
                anyhow!(
                    "zfb bundler: unterminated CSS string in module-worker dependency {}",
                    path.display()
                )
            })?;
            continue;
        }
        if bytes[index] == b'@' && ascii_ci_starts_with(bytes, index, b"@import") {
            let mut cursor = index + "@import".len();
            let keyword_boundary = bytes.get(cursor).is_none_or(|byte| {
                byte.is_ascii_whitespace() || matches!(*byte, b'\'' | b'"' | b'u' | b'U')
            });
            if !keyword_boundary {
                index += 1;
                continue;
            }
            skip_css_whitespace(bytes, &mut cursor);
            let (specifier, next) = if let Some(target_start) = css_url_target_start(bytes, cursor)
            {
                read_css_url_target(&without_comments, target_start).ok_or_else(|| {
                    anyhow!(
                        "zfb bundler: unsupported CSS @import url(...) syntax in module-worker dependency {}; refusing to omit a possibly bundled edge from the cache key",
                        path.display()
                    )
                })?
            } else {
                read_css_target(&without_comments, cursor, true).ok_or_else(|| {
                    anyhow!(
                        "zfb bundler: unsupported CSS @import syntax in module-worker dependency {}; refusing to omit a possibly bundled edge from the cache key",
                        path.display()
                    )
                })?
            };
            if specifier.is_empty() {
                bail!(
                    "zfb bundler: empty CSS @import in module-worker dependency {}",
                    path.display()
                );
            }
            references.imports.push(specifier);
            index = next;
            continue;
        }

        let url_boundary = index == 0
            || !matches!(bytes[index - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-');
        if url_boundary {
            if let Some(target_start) = css_url_target_start(bytes, index) {
                let (specifier, next) =
                    read_css_url_target(&without_comments, target_start).ok_or_else(|| {
                        anyhow!(
                            "zfb bundler: unsupported CSS url(...) syntax in module-worker dependency {}; unresolved local loader inputs cannot be omitted from the cache key",
                            path.display()
                        )
                    })?;
                if specifier.is_empty() {
                    bail!(
                        "zfb bundler: empty CSS url(...) in module-worker dependency {}",
                        path.display()
                    );
                }
                references.urls.push(specifier);
                index = next;
                continue;
            }
        }
        index += 1;
    }
    Ok(references)
}

fn skip_css_whitespace(bytes: &[u8], cursor: &mut usize) {
    while bytes
        .get(*cursor)
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        *cursor += 1;
    }
}

fn css_url_target_start(bytes: &[u8], start: usize) -> Option<usize> {
    if !ascii_ci_starts_with(bytes, start, b"url") {
        return None;
    }
    let mut cursor = start + 3;
    skip_css_whitespace(bytes, &mut cursor);
    if bytes.get(cursor) != Some(&b'(') {
        return None;
    }
    cursor += 1;
    skip_css_whitespace(bytes, &mut cursor);
    Some(cursor)
}

fn read_css_url_target(css: &str, start: usize) -> Option<(String, usize)> {
    let (specifier, mut next) = read_css_target(css, start, false)?;
    let bytes = css.as_bytes();
    skip_css_whitespace(bytes, &mut next);
    if bytes.get(next) != Some(&b')') {
        return None;
    }
    Some((specifier, next + 1))
}

fn skip_css_string(css: &str, start: usize) -> Option<usize> {
    let bytes = css.as_bytes();
    let quote = *bytes.get(start)?;
    let mut end = start + 1;
    while end < bytes.len() {
        if bytes[end] == b'\\' {
            end = end.saturating_add(2);
            continue;
        }
        if bytes[end] == quote {
            return Some(end + 1);
        }
        end += 1;
    }
    None
}

fn read_css_target(css: &str, start: usize, stop_at_semicolon: bool) -> Option<(String, usize)> {
    let bytes = css.as_bytes();
    let quote = *bytes.get(start)?;
    if matches!(quote, b'\'' | b'"') {
        let end = skip_css_string(css, start)? - 1;
        return Some((css[start + 1..end].to_string(), end + 1));
    }
    let mut end = start;
    while end < bytes.len()
        && bytes[end] != b')'
        && (!stop_at_semicolon || bytes[end] != b';')
        && !bytes[end].is_ascii_whitespace()
    {
        end += 1;
    }
    (end > start).then(|| (css[start..end].trim().to_string(), end))
}

fn ascii_ci_starts_with(bytes: &[u8], at: usize, needle: &[u8]) -> bool {
    bytes.len() >= at + needle.len() && bytes[at..at + needle.len()].eq_ignore_ascii_case(needle)
}

fn strip_css_block_comments(css: &str) -> String {
    let bytes = css.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'*' {
            index += 2;
            while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/') {
                index += 1;
            }
            index = (index + 2).min(bytes.len());
            out.push(b' ');
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| css.to_string())
}

fn collect_import_specifiers(module: &Module, unresolved_ctxt: SyntaxContext) -> Vec<String> {
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
            ModuleDecl::TsImportEquals(import_equals) if !import_equals.is_type_only => {
                if let swc_core::ecma::ast::TsModuleRef::TsExternalModuleRef(module_ref) =
                    &import_equals.module_ref
                {
                    specifiers.push(atom_to_string(&module_ref.expr.value));
                }
            }
            _ => {}
        }
    }

    struct RuntimeCalls {
        unresolved_ctxt: SyntaxContext,
        specifiers: Vec<String>,
    }
    impl Visit for RuntimeCalls {
        fn visit_call_expr(&mut self, node: &swc_core::ecma::ast::CallExpr) {
            let is_dynamic_import = matches!(node.callee, Callee::Import(_));
            let is_global_require = matches!(
                &node.callee,
                Callee::Expr(callee)
                    if matches!(&**callee, Expr::Ident(ident)
                        if ident.sym == "require" && ident.ctxt == self.unresolved_ctxt)
            );
            if is_dynamic_import || is_global_require {
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
    let mut runtime_calls = RuntimeCalls {
        unresolved_ctxt,
        specifiers: Vec::new(),
    };
    module.visit_with(&mut runtime_calls);
    specifiers.extend(runtime_calls.specifiers);
    specifiers
}

struct WorkerGraph {
    hash: String,
    worker_edges: BTreeSet<ModuleWorkerEdge>,
    files: BTreeSet<PathBuf>,
}

fn inspect_worker_graph(entry: &Path, project_root: &Path) -> Result<WorkerGraph> {
    let resolver = ProjectGraphResolver::new(project_root);
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
        if !is_js_like(&current) && !is_css_like(&current) {
            continue;
        }
        let source = String::from_utf8(bytes).map_err(|error| {
            anyhow!(
                "zfb bundler: module-worker source {} is not valid UTF-8: {error}",
                current.display()
            )
        })?;
        if is_css_like(&current) {
            let references = collect_css_references(&source, &current)?;
            for specifier in references.imports {
                if let Some(dependency) =
                    resolver.resolve(&current, &specifier, GraphSpecifierKind::CssImport)?
                {
                    if !visited.contains(&dependency) {
                        stack.push(dependency);
                    }
                }
            }
            for specifier in references.urls {
                if let Some(dependency) =
                    resolver.resolve(&current, &specifier, GraphSpecifierKind::CssUrl)?
                {
                    if !visited.contains(&dependency) {
                        stack.push(dependency);
                    }
                }
            }
            continue;
        }
        let (module, base, unresolved_ctxt) = parse_module(&current, &source)?;
        for occurrence in collect_constructor_occurrences(&module, base, unresolved_ctxt) {
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
        for specifier in collect_import_specifiers(&module, unresolved_ctxt) {
            if let Some(dependency) =
                resolver.resolve(&current, &specifier, GraphSpecifierKind::JavaScript)?
            {
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
/// `./worker-<injectively-encoded-relative-path>.js?v=<graph-hash>`.
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
    let (module, base, unresolved_ctxt) = parse_module(importer, source)?;
    let occurrences = collect_constructor_occurrences(&module, base, unresolved_ctxt);
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
            .contains("new Worker(new URL(\"./worker-src-s-workers-s-search-d-ts.js?v="));
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
    fn alias_require_and_css_subimports_join_hash_and_invalidation_closure() {
        let project = tempfile::tempdir().unwrap();
        let importer = project.path().join("src/app.ts");
        let worker = project.path().join("src/worker.ts");
        let alias = project.path().join("src/aliased.ts");
        let required = project.path().join("src/required.ts");
        let css = project.path().join("src/styles.css");
        let tokens = project.path().join("src/tokens.css");
        let icon = project.path().join("src/assets/icon.bin");
        let raw = project.path().join("src/payload.txt");
        write(&importer, "placeholder");
        write(
            &project.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/*"]}}}"#,
        );
        write(
            &worker,
            "import { value } from '@/aliased'; const required = require('./required'); import './styles.css'; import payload from './payload.txt?raw'; self.postMessage([value, required, payload]);",
        );
        write(&alias, "export const value = 'alias-a';");
        write(&required, "module.exports = 'required-a';");
        write(
            &css,
            "@import './tokens.css';\n.worker { background: url(  \"./assets/icon.bin?theme=dark#mask\"  ); mask: url(data:image/png;base64,AAAA); cursor: url(https://example.com/cursor.png); filter: url(blob:https://example.com/filter); }",
        );
        write(&tokens, ":root { --worker: a; }");
        std::fs::create_dir_all(icon.parent().unwrap()).unwrap();
        std::fs::write(&icon, [0_u8, 1, 2, 255]).unwrap();
        write(&raw, "raw-a");
        let source = "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });";

        let first = rewrite_module_worker_urls(source, &importer, project.path()).unwrap();
        for expected in [&alias, &required, &css, &tokens, &icon, &raw] {
            assert!(
                first
                    .dependencies
                    .iter()
                    .any(|edge| edge.dependency == *expected),
                "missing graph dependency {}: {:?}",
                expected.display(),
                first.dependencies
            );
        }

        write(&alias, "export const value = 'alias-b';");
        let alias_changed = rewrite_module_worker_urls(source, &importer, project.path()).unwrap();
        assert_ne!(first.expanded_source, alias_changed.expanded_source);
        write(&required, "module.exports = 'required-b';");
        let require_changed =
            rewrite_module_worker_urls(source, &importer, project.path()).unwrap();
        assert_ne!(
            alias_changed.expanded_source,
            require_changed.expanded_source
        );
        write(&tokens, ":root { --worker: b; }");
        let css_changed = rewrite_module_worker_urls(source, &importer, project.path()).unwrap();
        assert_ne!(require_changed.expanded_source, css_changed.expanded_source);
        std::fs::write(&icon, [0_u8, 1, 3, 255]).unwrap();
        let url_asset_changed =
            rewrite_module_worker_urls(source, &importer, project.path()).unwrap();
        assert_ne!(
            css_changed.expanded_source,
            url_asset_changed.expanded_source
        );
        write(&raw, "raw-b");
        let raw_changed = rewrite_module_worker_urls(source, &importer, project.path()).unwrap();
        assert_ne!(
            url_asset_changed.expanded_source,
            raw_changed.expanded_source
        );
    }

    #[test]
    fn base_url_import_equals_precedes_same_named_installed_package() {
        let project = tempfile::tempdir().unwrap();
        let importer = project.path().join("src/app.ts");
        let worker = project.path().join("src/worker.ts");
        let local = project.path().join("src/shared.ts");
        let installed = project.path().join("node_modules/shared/index.js");
        write(&importer, "placeholder");
        write(
            &project.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":"./src"}}"#,
        );
        write(
            &worker,
            "import shared = require('shared'); self.postMessage(shared);",
        );
        write(&local, "export = 'local-a';");
        write(&installed, "module.exports = 'installed';");
        let source = "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });";

        let first = rewrite_module_worker_urls(source, &importer, project.path()).unwrap();
        assert!(first
            .dependencies
            .iter()
            .any(|edge| edge.dependency == local));
        assert!(!first.dependencies.iter().any(|edge| edge
            .dependency
            .starts_with(project.path().join("node_modules"))));
        write(&local, "export = 'local-b';");
        let changed = rewrite_module_worker_urls(source, &importer, project.path()).unwrap();
        assert_ne!(first.expanded_source, changed.expanded_source);
    }

    #[test]
    fn unresolved_local_worker_dependency_fails_instead_of_hashing_partial_graph() {
        let project = tempfile::tempdir().unwrap();
        let importer = project.path().join("src/app.ts");
        let worker = project.path().join("src/worker.ts");
        write(&importer, "placeholder");
        write(&worker, "import './missing'; self.postMessage('ready');");
        let error = rewrite_module_worker_urls(
            "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });",
            &importer,
            project.path(),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("cannot be resolved"), "{error}");
        assert!(error.contains("missing"), "{error}");
    }

    #[test]
    fn unresolved_local_css_url_fails_instead_of_hashing_partial_graph() {
        let project = tempfile::tempdir().unwrap();
        let importer = project.path().join("src/app.ts");
        let worker = project.path().join("src/worker.ts");
        let css = project.path().join("src/worker.css");
        write(&importer, "placeholder");
        write(&worker, "import './worker.css';");
        write(
            &css,
            ".worker { background: url( './missing.bin?v=1#asset' ); }",
        );
        let error = rewrite_module_worker_urls(
            "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });",
            &importer,
            project.path(),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("missing.bin"), "{error}");
        assert!(error.contains("cannot be resolved"), "{error}");
    }

    #[test]
    fn overlapping_tsconfig_pattern_does_not_panic_or_false_match() {
        assert_eq!(match_tsconfig_pattern("a*a", "a"), None);
        assert_eq!(
            match_tsconfig_pattern("a*a", "aba"),
            Some(Some("b".to_string()))
        );
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
            "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module', ['type']: 'classic' });",
            "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module', [kind]: 'classic' });",
        ] {
            let rewrite = rewrite_module_worker_urls(source, &importer, project.path()).unwrap();
            assert_eq!(rewrite.expanded_source, source);
            assert!(rewrite.worker_edges.is_empty());
        }
        let computed_last_module = "new Worker(new URL('./worker.ts', import.meta.url), { type: 'classic', ['type']: 'module' });";
        let rewrite =
            rewrite_module_worker_urls(computed_last_module, &importer, project.path()).unwrap();
        assert_ne!(rewrite.expanded_source, computed_last_module);
    }

    #[test]
    fn shadowed_worker_shared_worker_and_url_bindings_are_not_claimed() {
        let project = tempfile::tempdir().unwrap();
        let importer = project.path().join("src/app.ts");
        let worker = project.path().join("src/worker.ts");
        write(&importer, "placeholder");
        write(&worker, "self.postMessage(1);");
        for source in [
            "import Worker from './fake'; new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });",
            "function start(Worker) { new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' }); }",
            "const URL = LocalURL; new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });",
            "import SharedWorker from './fake'; new SharedWorker(new URL('./worker.ts', import.meta.url));",
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
