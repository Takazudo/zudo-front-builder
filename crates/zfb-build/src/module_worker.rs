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
use zfb_plugin_resolver::{read_tsconfig_paths_file, TsConfigDependencyInput, TsConfigPaths};
use zfb_types::{module_worker_content_hash, module_worker_url_specifier, normalize_path_lexical};

/// Every non-source input that can change emitted module-worker bytes.
///
/// The stable worker filename is path-derived, so the rewritten `?v=` query
/// must cover both the first-party source graph and the esbuild/resolver
/// semantics used by the later browser-only emission pass. Callers construct
/// one context from the same bundle options and plugin registrations they
/// hand to esbuild, then pass it to every SSR/islands/client rewrite.
/// `preserve_symlinks` is intentionally absent: it is a preprocessing-shadow
/// transport choice that may differ between SSR and browser staging but must
/// resolve to the same logical graph. Hashing it would make those call sites
/// disagree even when the emitted worker bytes are identical.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleWorkerBuildContext {
    production: bool,
    minify: bool,
    sourcemap: bool,
    loader_args: Vec<String>,
    define: BTreeMap<String, String>,
    jsx_import_source: String,
    plugin_alias_entries: Vec<(String, String)>,
    plugin_virtual_modules: Vec<(String, String)>,
}

impl Default for ModuleWorkerBuildContext {
    fn default() -> Self {
        Self::new(false, &BTreeMap::new(), &BTreeMap::new(), "preact")
    }
}

impl ModuleWorkerBuildContext {
    /// Construct a context from the canonical bundle loader/define maps.
    pub fn new(
        production: bool,
        loaders: &BTreeMap<String, String>,
        define: &BTreeMap<String, String>,
        jsx_import_source: impl Into<String>,
    ) -> Self {
        Self {
            production,
            minify: production,
            sourcemap: !production,
            loader_args: loaders
                .iter()
                .map(|(extension, loader)| format!("--loader:{extension}={loader}"))
                .collect(),
            define: define.clone(),
            jsx_import_source: jsx_import_source.into(),
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
        }
    }

    /// Construct from the already-validated esbuild loader argv used by the
    /// SSR bundler. Sorting makes equivalent config maps hash identically.
    pub fn from_esbuild_loader_args(
        production: bool,
        loader_args: &[String],
        define: &BTreeMap<String, String>,
        jsx_import_source: impl Into<String>,
    ) -> Self {
        let mut loader_args = loader_args.to_vec();
        loader_args.sort();
        loader_args.dedup();
        Self {
            production,
            minify: production,
            sourcemap: !production,
            loader_args,
            define: define.clone(),
            jsx_import_source: jsx_import_source.into(),
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
        }
    }

    /// Attach the exact plugin resolver inputs consumed by esbuild.
    pub fn with_plugins(
        mut self,
        alias_entries: Vec<(String, String)>,
        virtual_modules: Vec<(String, String)>,
    ) -> Self {
        self.plugin_alias_entries = alias_entries;
        self.plugin_virtual_modules = virtual_modules;
        self
    }

    /// Record output-affecting flags that are independently overrideable on
    /// the lower-level bundler API. CLI presets normally derive these from
    /// mode, but spelling them out prevents library callers from reusing a
    /// query across different worker bytes.
    pub fn with_output_semantics(mut self, minify: bool, sourcemap: bool) -> Self {
        self.minify = minify;
        self.sourcemap = sourcemap;
        self
    }

    /// Whether plugin-aware discovery can find graph edges that the ordinary
    /// filesystem scanner cannot see. Callers use this to retain the
    /// zero-registration fast path and avoid a duplicate strict graph walk.
    pub fn has_plugin_resolver_inputs(&self) -> bool {
        !self.plugin_alias_entries.is_empty() || !self.plugin_virtual_modules.is_empty()
    }

    /// Drop virtual registrations suppressed by the documented user-wins
    /// tsconfig policy. Used by global virtual preflight so a losing plugin
    /// source cannot fail or materialise ahead of the user's mapping.
    pub fn without_user_claimed_virtual_modules(
        mut self,
        user_tsconfig_paths: &BTreeMap<String, Vec<String>>,
    ) -> Self {
        self.plugin_virtual_modules.retain(|(specifier, _)| {
            !zfb_plugin_resolver::user_claims_specifier(user_tsconfig_paths, specifier)
        });
        self
    }

    fn virtual_module_source(&self, specifier: &str) -> Option<&str> {
        self.plugin_virtual_modules
            .iter()
            .find_map(|(candidate, source)| (candidate == specifier).then_some(source.as_str()))
    }

    fn append_cache_envelope(&self, aggregate: &mut Vec<u8>, project_root: &Path) {
        fn field(aggregate: &mut Vec<u8>, tag: &[u8], value: &[u8]) {
            aggregate.extend_from_slice(&(tag.len() as u64).to_le_bytes());
            aggregate.extend_from_slice(tag);
            aggregate.extend_from_slice(&(value.len() as u64).to_le_bytes());
            aggregate.extend_from_slice(value);
        }

        field(aggregate, b"abi", b"zfb-module-worker-cache-v2");
        field(
            aggregate,
            b"esbuild",
            zfb_toolchain_pins::EXPECTED_ESBUILD_VERSION.as_bytes(),
        );
        field(
            aggregate,
            b"mode",
            if self.production {
                b"production"
            } else {
                b"development"
            },
        );
        field(
            aggregate,
            b"minify",
            if self.minify { b"true" } else { b"false" },
        );
        field(
            aggregate,
            b"sourcemap",
            if self.sourcemap { b"true" } else { b"false" },
        );
        field(
            aggregate,
            b"jsx-import-source",
            self.jsx_import_source.as_bytes(),
        );
        for loader in &self.loader_args {
            field(aggregate, b"loader", loader.as_bytes());
        }
        for (key, value) in &self.define {
            field(aggregate, b"define-key", key.as_bytes());
            field(aggregate, b"define-value", value.as_bytes());
        }

        // Resolver registries are semantically maps. Sort their serialized
        // pairs so plugin registration iteration order cannot perturb URLs.
        let mut aliases = self.plugin_alias_entries.clone();
        aliases.sort();
        let root = normalize_path_lexical(project_root);
        for (specifier, target) in aliases {
            let target_path = normalize_path_lexical(Path::new(&target));
            let stable_target = target_path
                .strip_prefix(&root)
                .map(|relative| {
                    format!("project:/{}", relative.to_string_lossy().replace('\\', "/"))
                })
                .unwrap_or_else(|_| format!("external:{}", target_path.to_string_lossy()));
            field(aggregate, b"plugin-alias", specifier.as_bytes());
            field(aggregate, b"plugin-alias-target", stable_target.as_bytes());
        }
        let mut virtual_modules = self.plugin_virtual_modules.clone();
        virtual_modules.sort();
        for (specifier, source) in virtual_modules {
            let stable_source = stable_virtual_module_source(&source, project_root);
            field(aggregate, b"plugin-virtual", specifier.as_bytes());
            field(
                aggregate,
                b"plugin-virtual-source",
                stable_source.as_bytes(),
            );
        }
    }
}

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
    /// Worker entry, first-party transitive dependency, effective config, or
    /// watch-only config candidate associated with its `?v=` graph.
    pub dependency: PathBuf,
}

/// A terminal `?raw` edge found through the plugin-aware worker resolver.
///
/// Command-layer filesystem scans cannot see exact plugin aliases or virtual
/// module imports. Returning these physical edges lets preprocessing shadows
/// rewrite the importer and materialise the generated raw wrapper before the
/// later esbuild worker job consumes it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ModuleWorkerRawImportEdge {
    /// Physical project-local module containing the `?raw` import.
    pub importer: PathBuf,
    /// Terminal physical file whose UTF-8 text is requested.
    pub target: PathBuf,
}

/// Plugin-aware preprocessing metadata for one ordinary JS/TS entry graph.
///
/// Unlike the islands filesystem scanner, this walk applies exact plugin
/// aliases and virtual modules. It is used to decide whether a shadow is
/// required even when all `?raw`/Worker syntax lives exclusively behind a
/// plugin-resolved edge.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModulePreprocessingDiscovery {
    /// First-party physical files reached from the entry, including terminal
    /// raw targets. Config files are reported separately.
    pub files: Vec<PathBuf>,
    /// Direct and nested module-worker edges in the complete graph.
    pub worker_edges: Vec<ModuleWorkerEdge>,
    /// Terminal raw edges in the complete graph.
    pub raw_import_edges: Vec<ModuleWorkerRawImportEdge>,
    /// Effective TypeScript-style config inputs plus absent/precedence
    /// candidates that must remain observable for invalidation.
    pub config_dependencies: Vec<PathBuf>,
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
    /// Plugin-aware terminal raw edges that require shadow preprocessing.
    pub raw_import_edges: Vec<ModuleWorkerRawImportEdge>,
    /// Effective tsconfig/jsconfig inputs and watch-only precedence
    /// candidates. These participate in invalidation but are not executable
    /// modules; only effective inputs affect the worker fingerprint.
    pub config_dependencies: Vec<ModuleWorkerDependency>,
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

#[derive(Debug, Clone)]
struct ImportSpecifierOccurrence {
    specifier: String,
    lo: usize,
    hi: usize,
}

fn atom_to_string(value: &swc_core::atoms::Wtf8Atom) -> String {
    value.to_atom_lossy().to_string()
}

fn parse_as_tsx(path: &Path) -> bool {
    !matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("ts" | "mts" | "cts")
    )
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
            tsx: parse_as_tsx(path),
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

fn source_contains_worker_constructor_text(source: &str) -> bool {
    source.contains("new Worker") || source.contains("new SharedWorker")
}

// This guard is intentionally substring-based. A comment or string containing
// `new Worker` is treated as a possible worker site because silently skipping
// a real unparseable worker rewrite would ship a runtime 404.
fn fail_closed_unparseable_worker_source(
    path: &Path,
    source: &str,
    error: anyhow::Error,
) -> Result<()> {
    if source_contains_worker_constructor_text(source) {
        Err(error).with_context(|| {
            format!(
                "zfb bundler: cannot safely skip unparseable module-worker source {} because it contains `new Worker` or `new SharedWorker` text; comments and strings are treated as possible worker syntax",
                path.display()
            )
        })
    } else {
        Ok(())
    }
}

fn empty_rewrite(source: &str) -> ModuleWorkerRewrite {
    ModuleWorkerRewrite {
        expanded_source: source.to_string(),
        worker_edges: Vec::new(),
        dependencies: Vec::new(),
        raw_import_edges: Vec::new(),
        config_dependencies: Vec::new(),
    }
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

pub(crate) fn probe_graph_candidate(candidate: &Path, exact: bool) -> Option<PathBuf> {
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

pub(crate) fn match_tsconfig_pattern(pattern: &str, specifier: &str) -> Option<Option<String>> {
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

pub(crate) fn tsconfig_pattern_specificity(pattern: &str) -> usize {
    pattern
        .find('*')
        .map(|star| star.max(pattern.len().saturating_sub(star + 1)))
        .unwrap_or(pattern.len())
}

pub(crate) fn substitute_tsconfig_target(target: &str, capture: Option<&str>) -> String {
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
    exact: bool,
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
            if let Some(found) = probe_graph_candidate(&candidate, exact) {
                return Ok(Some(Some(found)));
            }
        }
    }
    Ok(Some(None))
}

fn resolve_tsconfig_base_url(
    paths: Option<&TsConfigPaths>,
    specifier: &str,
    exact: bool,
) -> Option<PathBuf> {
    let base_url = paths?.base_url.as_ref()?;
    probe_graph_candidate(&normalize_path_lexical(&base_url.join(specifier)), exact)
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
    plugin_aliases: BTreeMap<String, String>,
    plugin_virtual_modules: BTreeSet<String>,
    allow_unresolved_bare: bool,
}

enum GraphResolution {
    File(PathBuf),
    RawFile(PathBuf),
    Virtual(String),
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

fn nearest_typescript_config(source_path: &Path) -> Option<PathBuf> {
    let mut dir = normalize_path_lexical(source_path).parent()?.to_path_buf();
    loop {
        for filename in ["tsconfig.json", "jsconfig.json"] {
            let candidate = dir.join(filename);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        let parent = dir.parent()?;
        if parent == dir {
            return None;
        }
        dir = parent.to_path_buf();
    }
}

fn typescript_config_watch_candidates(
    source_path: &Path,
    selected_config: Option<&Path>,
) -> Vec<PathBuf> {
    let Some(mut dir) = normalize_path_lexical(source_path)
        .parent()
        .map(Path::to_path_buf)
    else {
        return Vec::new();
    };
    let mut candidates = Vec::new();
    loop {
        // Keep both names even when absent. `tsconfig.json` wins over
        // `jsconfig.json` in the same directory, so creation/deletion of
        // either can change which effective config esbuild consumes.
        candidates.push(dir.join("tsconfig.json"));
        candidates.push(dir.join("jsconfig.json"));
        // Configs above the selected directory cannot participate until the
        // selected file is deleted. That deletion is already watched and its
        // rescan expands this candidate set to the next selected directory.
        if selected_config.and_then(Path::parent) == Some(dir.as_path()) {
            break;
        }
        let Some(parent) = dir.parent() else {
            break;
        };
        if parent == dir {
            break;
        }
        dir = parent.to_path_buf();
    }
    candidates
}

fn config_identity(project_root: &Path, config: &Path) -> String {
    let root = normalize_path_lexical(project_root);
    let config = normalize_path_lexical(config);
    if let Ok(relative) = config.strip_prefix(&root) {
        return format!("project:/{}", relative.to_string_lossy().replace('\\', "/"));
    }

    let components = config.components().collect::<Vec<_>>();
    if let Some(index) = components.iter().rposition(|component| {
        matches!(component, Component::Normal(name) if *name == std::ffi::OsStr::new("node_modules"))
    }) {
        let package_relative = components[index + 1..]
            .iter()
            .filter_map(|component| match component {
                Component::Normal(value) => Some(value.to_string_lossy()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/");
        return format!("package:/{package_relative}");
    }

    format!(
        "external:/{}",
        config
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("config.json"))
            .to_string_lossy()
    )
}

#[derive(Default)]
struct SourceConfigResolution {
    hash_inputs: Vec<TsConfigDependencyInput>,
    watch_paths: Vec<PathBuf>,
}

fn config_resolution_for_source(source_path: &Path) -> Result<SourceConfigResolution> {
    let selected_config = nearest_typescript_config(source_path);
    let mut watch_paths =
        typescript_config_watch_candidates(source_path, selected_config.as_deref());
    let dependency_inputs = match selected_config {
        Some(config) => zfb_plugin_resolver::collect_tsconfig_dependency_inputs(&config)
            .with_context(|| {
                format!(
                    "collect effective TypeScript config chain for module-worker source {}",
                    source_path.display()
                )
            })?,
        None => Vec::new(),
    };
    watch_paths.extend(dependency_inputs.iter().map(|input| input.path.clone()));
    let hash_inputs = dependency_inputs
        .into_iter()
        .filter(|input| input.affects_fingerprint)
        .collect();
    Ok(SourceConfigResolution {
        hash_inputs,
        watch_paths,
    })
}

impl ProjectGraphResolver {
    fn new(
        project_root: &Path,
        worker_entry: &Path,
        context: &ModuleWorkerBuildContext,
        allow_unresolved_bare: bool,
        include_plugin_inputs: bool,
    ) -> Self {
        // `compilerOptions.paths` keeps the first exact key on collision.
        // Preserve that same behavior for duplicate plugin registrations.
        let mut plugin_aliases = BTreeMap::new();
        if include_plugin_inputs {
            for (specifier, target) in &context.plugin_alias_entries {
                plugin_aliases
                    .entry(specifier.clone())
                    .or_insert_with(|| target.clone());
            }
        }
        let selected_config = nearest_typescript_config(worker_entry);
        Self {
            project_root: project_root.to_path_buf(),
            tsconfig_paths: selected_config
                .as_deref()
                .and_then(read_tsconfig_paths_file),
            plugin_aliases,
            plugin_virtual_modules: if include_plugin_inputs {
                context
                    .plugin_virtual_modules
                    .iter()
                    .map(|(specifier, _)| specifier.clone())
                    .collect()
            } else {
                BTreeSet::new()
            },
            allow_unresolved_bare,
        }
    }

    fn user_claims_specifier(&self, specifier: &str) -> bool {
        self.tsconfig_paths.as_ref().is_some_and(|paths| {
            paths
                .aliases
                .iter()
                .any(|alias| match_tsconfig_pattern(&alias.pattern, specifier).is_some())
        })
    }

    fn uses_synthetic_config(&self) -> bool {
        !self.plugin_aliases.is_empty()
            || self
                .plugin_virtual_modules
                .iter()
                .any(|specifier| !self.user_claims_specifier(specifier))
    }

    fn resolve(
        &self,
        importer: &Path,
        specifier: &str,
        kind: GraphSpecifierKind,
    ) -> Result<Option<GraphResolution>> {
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
        } else if Path::new(path_specifier).is_absolute() {
            let candidate = normalize_path_lexical(Path::new(path_specifier));
            let found = probe_graph_candidate(&candidate, raw).ok_or_else(|| {
                anyhow!(
                    "zfb bundler: module-worker dependency {specifier:?} in {} cannot be resolved exactly enough to produce a safe cache key (looked from {})",
                    importer.display(),
                    importer_dir.display()
                )
            })?;
            return self.finish_absolute_file_resolution(specifier, importer, found, raw);
        } else {
            let user_claims_specifier = self.user_claims_specifier(path_specifier);
            // Plugin virtual modules are emitted through esbuild `--alias`
            // unless the user's tsconfig claims the specifier. They have no
            // stable filesystem identity, so their source bytes live in the
            // context cache envelope instead of the watch-path closure.
            if self.plugin_virtual_modules.contains(path_specifier) && !user_claims_specifier {
                return Ok(Some(GraphResolution::Virtual(path_specifier.to_string())));
            }

            let user_has_exact = self.tsconfig_paths.as_ref().is_some_and(|paths| {
                paths
                    .aliases
                    .iter()
                    .any(|alias| alias.pattern == path_specifier)
            });
            // Plugin aliases are exact `paths` entries. An exact user key
            // wins; otherwise the plugin exact key outranks user wildcards,
            // matching esbuild's path-pattern specificity.
            if !user_has_exact {
                if let Some(target) = self.plugin_aliases.get(path_specifier) {
                    let candidate = normalize_path_lexical(Path::new(target));
                    let found = probe_graph_candidate(&candidate, raw).ok_or_else(|| {
                        anyhow!(
                            "zfb bundler: plugin alias {path_specifier:?} imported by {} targets {}, but no module resolved; refusing to produce a stale worker cache key",
                            importer.display(),
                            candidate.display()
                        )
                    })?;
                    return self.finish_file_resolution(found, raw);
                }
            }

            let alias_resolution =
                resolve_tsconfig_graph_alias(self.tsconfig_paths.as_ref(), path_specifier, raw)?;
            if let Some(Some(found)) = alias_resolution.as_ref() {
                found.clone()
            } else if alias_resolution.is_some() {
                bail!(
                    "zfb bundler: tsconfig path alias {specifier:?} imported by {} matched a first-party mapping but no target resolved; refusing to produce a stale worker cache key",
                    importer.display()
                )
            } else if let Some(found) =
                resolve_tsconfig_base_url(self.tsconfig_paths.as_ref(), path_specifier, raw)
            {
                found
            } else if path_specifier.starts_with("node:")
                || installed_package_exists(importer_dir, &self.project_root, path_specifier)
                || self.allow_unresolved_bare
            {
                return Ok(None);
            } else {
                bail!(
                    "zfb bundler: non-relative dependency {specifier:?} imported by {} is neither a resolvable project tsconfig alias/baseUrl file nor an installed package; refusing to omit it from the worker cache key",
                    importer.display()
                )
            }
        };
        self.finish_file_resolution(found, raw)
    }

    fn finish_absolute_file_resolution(
        &self,
        specifier: &str,
        importer: &Path,
        found: PathBuf,
        raw: bool,
    ) -> Result<Option<GraphResolution>> {
        if is_inside_node_modules(&found) {
            return Ok(None);
        }

        let canonical_root = self.project_root.canonicalize().with_context(|| {
            format!("canonicalize project root {}", self.project_root.display())
        })?;
        let canonical = found.canonicalize().with_context(|| {
            format!(
                "canonicalize absolute module-worker dependency {}",
                found.display()
            )
        })?;
        if !canonical.starts_with(&canonical_root) {
            bail!(
                "zfb bundler: absolute module-worker dependency {specifier:?} in {} is outside the project graph contract (canonical target {})",
                importer.display(),
                canonical.display()
            );
        }

        if is_inside_node_modules(&canonical) {
            return Ok(None);
        }

        let relative = canonical.strip_prefix(&canonical_root).with_context(|| {
            format!(
                "map canonical module-worker dependency {} into project root {}",
                canonical.display(),
                canonical_root.display()
            )
        })?;
        let logical = normalize_path_lexical(&self.project_root.join(relative));
        match validate_first_party_path(&logical, &self.project_root, "module-worker dependency") {
            Ok(path) => Ok(Some(if raw {
                GraphResolution::RawFile(path)
            } else {
                GraphResolution::File(path)
            })),
            Err(error) => bail!(
                "zfb bundler: absolute module-worker dependency {specifier:?} in {} is outside the project graph contract: {error:#}",
                importer.display()
            ),
        }
    }

    fn finish_file_resolution(&self, found: PathBuf, raw: bool) -> Result<Option<GraphResolution>> {
        let resolves_under_node_modules = is_inside_node_modules(&found)
            || found
                .canonicalize()
                .ok()
                .is_some_and(|canonical| is_inside_node_modules(&canonical));
        if resolves_under_node_modules {
            return Ok(None);
        }
        match validate_first_party_path(&found, &self.project_root, "module-worker dependency") {
            Ok(path) => Ok(Some(if raw {
                GraphResolution::RawFile(path)
            } else {
                GraphResolution::File(path)
            })),
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

fn collect_import_specifier_occurrences(
    module: &Module,
    base: u32,
    unresolved_ctxt: SyntaxContext,
) -> Vec<ImportSpecifierOccurrence> {
    fn occurrence(value: &swc_core::ecma::ast::Str, base: u32) -> ImportSpecifierOccurrence {
        let span = value.span();
        ImportSpecifierOccurrence {
            specifier: atom_to_string(&value.value),
            lo: (span.lo.0 - base) as usize,
            hi: (span.hi.0 - base) as usize,
        }
    }

    let mut specifiers = Vec::new();
    for item in &module.body {
        let ModuleItem::ModuleDecl(declaration) = item else {
            continue;
        };
        match declaration {
            ModuleDecl::Import(import) if !import.type_only => {
                let runtime = import.specifiers.iter().any(|specifier| {
                    !matches!(specifier, ImportSpecifier::Named(named) if named.is_type_only)
                });
                if runtime || import.specifiers.is_empty() {
                    specifiers.push(occurrence(&import.src, base));
                }
            }
            ModuleDecl::ExportNamed(export) if !export.type_only => {
                if let Some(source) = &export.src {
                    specifiers.push(occurrence(source, base));
                }
            }
            ModuleDecl::ExportAll(export) if !export.type_only => {
                specifiers.push(occurrence(&export.src, base));
            }
            ModuleDecl::TsImportEquals(import_equals) if !import_equals.is_type_only => {
                if let swc_core::ecma::ast::TsModuleRef::TsExternalModuleRef(module_ref) =
                    &import_equals.module_ref
                {
                    specifiers.push(occurrence(&module_ref.expr, base));
                }
            }
            _ => {}
        }
    }

    struct RuntimeCalls {
        base: u32,
        unresolved_ctxt: SyntaxContext,
        specifiers: Vec<ImportSpecifierOccurrence>,
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
                            self.specifiers.push(occurrence(value, self.base));
                        }
                    }
                }
            }
            node.visit_children_with(self);
        }
    }
    let mut runtime_calls = RuntimeCalls {
        base,
        unresolved_ctxt,
        specifiers: Vec::new(),
    };
    module.visit_with(&mut runtime_calls);
    specifiers.extend(runtime_calls.specifiers);
    specifiers
}

fn stable_virtual_module_source(source: &str, project_root: &Path) -> String {
    let virtual_path = project_root.join(".zfb-worker-virtual-module.mjs");
    let Ok((module, base, unresolved_ctxt)) = parse_module(&virtual_path, source) else {
        return source.to_string();
    };
    let mut replacements = collect_import_specifier_occurrences(&module, base, unresolved_ctxt)
        .into_iter()
        .filter_map(|occurrence| {
            let stable = stable_project_virtual_specifier(&occurrence.specifier, project_root)
                .unwrap_or(occurrence.specifier);
            let replacement = serde_json::to_string(&stable).ok()?;
            Some((occurrence.lo, occurrence.hi, replacement))
        })
        .collect::<Vec<_>>();
    if replacements.is_empty() {
        return source.to_string();
    }

    replacements.sort_by_key(|replacement| std::cmp::Reverse(replacement.0));
    let mut stable = source.to_string();
    for (lo, hi, replacement) in replacements {
        stable.replace_range(lo..hi, &replacement);
    }
    stable
}

fn stable_project_virtual_specifier(specifier: &str, project_root: &Path) -> Option<String> {
    if specifier.contains('?') || specifier.contains('#') {
        return None;
    }
    let specifier_path = Path::new(specifier);
    let candidate = if specifier_path.is_absolute() {
        normalize_path_lexical(specifier_path)
    } else if specifier.starts_with("./") || specifier.starts_with("../") {
        normalize_path_lexical(&project_root.join(specifier_path))
    } else {
        return None;
    };
    if is_inside_node_modules(&candidate) {
        return None;
    }
    let found = probe_graph_candidate(&candidate, false)?;
    let canonical_root = project_root.canonicalize().ok()?;
    let canonical = found.canonicalize().ok()?;
    if !canonical.starts_with(&canonical_root) || is_inside_node_modules(&canonical) {
        return None;
    }
    let relative = canonical.strip_prefix(&canonical_root).ok()?;
    let relative = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    Some(format!("./{relative}"))
}

/// Collect statically analyzable runtime import/require specifiers from one
/// JS/TS module without applying first-party path policy. The bundler uses
/// this for a bounded dependency-package closure when an exact target under
/// node_modules must be copied into an isolated resolver root.
pub(crate) fn collect_runtime_import_specifiers_from_file(path: &Path) -> Result<Vec<String>> {
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("read runtime import source {}", path.display()))?;
    if is_css_like(path) {
        let references = collect_css_references(&source, path)?;
        return Ok(references
            .imports
            .into_iter()
            .chain(references.urls)
            .collect());
    }
    let (module, _, unresolved_ctxt) = parse_module(path, &source)?;
    Ok(collect_import_specifiers(&module, unresolved_ctxt))
}

fn validated_virtual_import_specifiers(
    context: &ModuleWorkerBuildContext,
    specifier: &str,
    project_root: &Path,
) -> Result<Vec<String>> {
    let source = context.virtual_module_source(specifier).ok_or_else(|| {
        anyhow!(
            "zfb bundler: module graph resolved virtual module {specifier:?} without source bytes"
        )
    })?;
    let virtual_path = project_root.join(".zfb-worker-virtual-module.mjs");
    let (module, base, unresolved_ctxt) = parse_module(&virtual_path, source)?;
    if !collect_constructor_occurrences(&module, base, unresolved_ctxt).is_empty() {
        bail!(
            "zfb bundler: module workers declared inside plugin virtual module {specifier:?} are unsupported; declare the Worker constructor in a project source file so zfb can emit its companion"
        );
    }
    if source.contains("import.meta.glob")
        && crate::glob_expand::source_contains_import_meta_glob(source).with_context(|| {
            format!("inspect import.meta.glob syntax in plugin virtual module {specifier:?}")
        })?
    {
        bail!(
            "zfb bundler: import.meta.glob(...) inside plugin virtual module {specifier:?} is unsupported because virtual sources cannot be rewritten into the preprocessing shadow; move the glob into a project source file"
        );
    }
    match crate::raw_import_expand::supported_raw_import_specifier_for_path(source, &virtual_path) {
        Ok(Some(raw_specifier)) => {
            bail!(
                "zfb bundler: query-bearing import {raw_specifier:?} inside plugin virtual module {specifier:?} is unsupported because virtual sources cannot be rewritten into the preprocessing shadow; move the import into a project source file"
            );
        }
        Ok(None) => {}
        Err(error) => {
            bail!(
                "zfb bundler: unsupported query syntax inside plugin virtual module {specifier:?}: {error:#}. Virtual sources cannot be rewritten into the preprocessing shadow"
            );
        }
    }
    let dependencies = collect_import_specifiers(&module, unresolved_ctxt);
    Ok(dependencies)
}

struct WorkerGraph {
    hash: String,
    worker_edges: BTreeSet<ModuleWorkerEdge>,
    raw_import_edges: BTreeSet<ModuleWorkerRawImportEdge>,
    files: BTreeSet<PathBuf>,
    config_files: BTreeSet<PathBuf>,
}

fn inspect_worker_graph(
    entry: &Path,
    project_root: &Path,
    context: &ModuleWorkerBuildContext,
    allow_unresolved_bare: bool,
) -> Result<WorkerGraph> {
    let entry = entry.to_path_buf();
    let mut job_resolvers = BTreeMap::from([(
        entry.clone(),
        ProjectGraphResolver::new(project_root, &entry, context, allow_unresolved_bare, true),
    )]);
    let mut native_resolvers = BTreeMap::new();
    let mut visited = BTreeSet::new();
    let mut stack = vec![(entry.clone(), entry.clone())];
    let mut virtual_stack = Vec::new();
    let mut visited_virtual = BTreeSet::new();
    let mut file_bytes: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();
    let mut worker_edges = BTreeSet::new();
    let mut raw_import_edges = BTreeSet::new();
    let initial_config = config_resolution_for_source(&entry)?;
    let mut config_hash_inputs = initial_config
        .hash_inputs
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut config_files = initial_config
        .watch_paths
        .into_iter()
        .collect::<BTreeSet<_>>();

    loop {
        while let Some((job_entry, current)) = stack.pop() {
            if !visited.insert((job_entry.clone(), current.clone())) {
                continue;
            }
            job_resolvers.entry(job_entry.clone()).or_insert_with(|| {
                ProjectGraphResolver::new(
                    project_root,
                    &job_entry,
                    context,
                    allow_unresolved_bare,
                    true,
                )
            });
            let uses_synthetic_config = job_resolvers
                .get(&job_entry)
                .expect("worker job resolver")
                .uses_synthetic_config();
            if !uses_synthetic_config {
                native_resolvers.entry(current.clone()).or_insert_with(|| {
                    ProjectGraphResolver::new(
                        project_root,
                        &current,
                        context,
                        allow_unresolved_bare,
                        false,
                    )
                });
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
                    let resolver = if uses_synthetic_config {
                        job_resolvers.get(&job_entry).expect("worker job resolver")
                    } else {
                        native_resolvers
                            .get(&current)
                            .expect("native module resolver")
                    };
                    if let Some(GraphResolution::File(dependency)) =
                        resolver.resolve(&current, &specifier, GraphSpecifierKind::CssImport)?
                    {
                        if !visited.contains(&(job_entry.clone(), dependency.clone())) {
                            stack.push((job_entry.clone(), dependency));
                        }
                    }
                }
                for specifier in references.urls {
                    let resolver = if uses_synthetic_config {
                        job_resolvers.get(&job_entry).expect("worker job resolver")
                    } else {
                        native_resolvers
                            .get(&current)
                            .expect("native module resolver")
                    };
                    if let Some(GraphResolution::File(dependency)) =
                        resolver.resolve(&current, &specifier, GraphSpecifierKind::CssUrl)?
                    {
                        if !visited.contains(&(job_entry.clone(), dependency.clone())) {
                            stack.push((job_entry.clone(), dependency));
                        }
                    }
                }
                continue;
            }
            if !uses_synthetic_config {
                let config = config_resolution_for_source(&current)?;
                config_hash_inputs.extend(config.hash_inputs);
                config_files.extend(config.watch_paths);
            }
            let (module, base, unresolved_ctxt) = match parse_module(&current, &source) {
                Ok(parsed) => parsed,
                Err(error) => {
                    fail_closed_unparseable_worker_source(&current, &source, error)?;
                    continue;
                }
            };
            for occurrence in collect_constructor_occurrences(&module, base, unresolved_ctxt) {
                if occurrence.kind == ConstructorKind::SharedWorker {
                    bail!(
                    "zfb bundler: unsupported SharedWorker in {} for {:?}. Only module `Worker` constructors are supported.",
                    current.display(),
                    occurrence.specifier
                );
                }
                let nested = resolve_worker_target(&current, &occurrence.specifier, project_root)?;
                job_resolvers.entry(nested.clone()).or_insert_with(|| {
                    ProjectGraphResolver::new(
                        project_root,
                        &nested,
                        context,
                        allow_unresolved_bare,
                        true,
                    )
                });
                if job_resolvers
                    .get(&nested)
                    .expect("nested worker job resolver")
                    .uses_synthetic_config()
                {
                    // Plugin-enabled emission supplies one synthetic
                    // `--tsconfig` selected from each worker ENTRY for that
                    // entire job. Nested workers are separate jobs, so each
                    // contributes its own nearest effective chain; ordinary
                    // transitive modules do not.
                    let config = config_resolution_for_source(&nested)?;
                    config_hash_inputs.extend(config.hash_inputs);
                    config_files.extend(config.watch_paths);
                }
                worker_edges.insert(ModuleWorkerEdge {
                    importer: current.clone(),
                    source_path: nested.clone(),
                });
                if !visited.contains(&(nested.clone(), nested.clone())) {
                    stack.push((nested.clone(), nested));
                }
            }
            for specifier in collect_import_specifiers(&module, unresolved_ctxt) {
                let resolver = if uses_synthetic_config {
                    job_resolvers.get(&job_entry).expect("worker job resolver")
                } else {
                    native_resolvers
                        .get(&current)
                        .expect("native module resolver")
                };
                if let Some(resolution) =
                    resolver.resolve(&current, &specifier, GraphSpecifierKind::JavaScript)?
                {
                    match resolution {
                        GraphResolution::File(dependency) => {
                            if !visited.contains(&(job_entry.clone(), dependency.clone())) {
                                stack.push((job_entry.clone(), dependency));
                            }
                        }
                        GraphResolution::RawFile(target) => {
                            let bytes = std::fs::read(&target).with_context(|| {
                                format!("read raw module-worker dependency {}", target.display())
                            })?;
                            file_bytes.entry(target.clone()).or_insert(bytes);
                            raw_import_edges.insert(ModuleWorkerRawImportEdge {
                                importer: current.clone(),
                                target,
                            });
                        }
                        GraphResolution::Virtual(specifier) => {
                            virtual_stack.push((job_entry.clone(), specifier));
                        }
                    }
                }
            }
        }

        while let Some((job_entry, specifier)) = virtual_stack.pop() {
            if !visited_virtual.insert((job_entry.clone(), specifier.clone())) {
                continue;
            }
            // zfb-plugin-resolver materializes virtual modules directly inside
            // the project working directory. This stable synthetic identity gives
            // relative imports the same parent directory without leaking the
            // random temp filename into the cache key.
            let virtual_path = project_root.join(".zfb-worker-virtual-module.mjs");
            for dependency_specifier in
                validated_virtual_import_specifiers(context, &specifier, project_root)?
            {
                let resolver = job_resolvers.get(&job_entry).expect("worker job resolver");
                if let Some(resolution) = resolver.resolve(
                    &virtual_path,
                    &dependency_specifier,
                    GraphSpecifierKind::JavaScript,
                )? {
                    match resolution {
                        GraphResolution::File(dependency) => {
                            if !visited.contains(&(job_entry.clone(), dependency.clone())) {
                                stack.push((job_entry.clone(), dependency));
                            }
                        }
                        GraphResolution::RawFile(_) => unreachable!(
                            "query-bearing virtual imports are rejected before resolution"
                        ),
                        GraphResolution::Virtual(nested) => {
                            virtual_stack.push((job_entry.clone(), nested));
                        }
                    }
                }
            }
        }
        if stack.is_empty() {
            break;
        }
    }

    // Length-prefix every path and body so concatenation is unambiguous. Paths
    // are project-relative and slash-normalized, making the cache key stable
    // across worktree locations and host operating systems.
    let root = normalize_path_lexical(project_root);
    let mut aggregate = Vec::new();
    context.append_cache_envelope(&mut aggregate, project_root);
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
    config_files.extend(config_hash_inputs.iter().map(|input| input.path.clone()));
    let mut config_inputs = Vec::with_capacity(config_hash_inputs.len());
    for input in &config_hash_inputs {
        let (present, bytes) = match std::fs::read(&input.path) {
            Ok(bytes) => (true, bytes),
            Err(error) if input.missing_allowed && error.kind() == std::io::ErrorKind::NotFound => {
                (false, Vec::new())
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "read module-worker TypeScript config input {}",
                        input.path.display()
                    )
                })
            }
        };
        config_inputs.push((config_identity(project_root, &input.path), present, bytes));
    }
    // External config paths intentionally do not enter the identity. Sorting
    // by identity + bytes keeps equivalent relocated projects stable while
    // still preserving every distinct config body in an extends array.
    config_inputs.sort();
    for (identity, present, bytes) in config_inputs {
        let tagged_identity = format!("config:{identity}");
        aggregate.extend_from_slice(&(tagged_identity.len() as u64).to_le_bytes());
        aggregate.extend_from_slice(tagged_identity.as_bytes());
        aggregate.push(u8::from(present));
        aggregate.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        aggregate.extend_from_slice(&bytes);
    }
    Ok(WorkerGraph {
        hash: module_worker_content_hash(&aggregate),
        worker_edges,
        raw_import_edges,
        files: file_bytes.into_keys().collect(),
        config_files,
    })
}

/// Discover preprocessing requirements from an ordinary project entry using
/// the same plugin-aware resolver and virtual-module policy as worker hashing.
pub fn discover_module_preprocessing_with_context(
    entry: &Path,
    project_root: &Path,
    context: &ModuleWorkerBuildContext,
) -> Result<ModulePreprocessingDiscovery> {
    validate_first_party_path(entry, project_root, "module preprocessing entry")?;
    let graph = inspect_worker_graph(entry, project_root, context, true)?;
    Ok(ModulePreprocessingDiscovery {
        files: graph.files.into_iter().collect(),
        worker_edges: graph.worker_edges.into_iter().collect(),
        raw_import_edges: graph.raw_import_edges.into_iter().collect(),
        config_dependencies: graph.config_files.into_iter().collect(),
    })
}

/// Validate every registered virtual source and discover preprocessing needs
/// in physical project modules imported by those sources.
///
/// Virtual registrations are global resolver inputs, so this pass is global
/// too: unsupported query/Worker/glob syntax fails deterministically even
/// when route discovery happens through MDX, injected entries, or another
/// surface that cannot provide a physical JS root to the Rust scanner.
pub fn discover_registered_virtual_preprocessing_with_context(
    project_root: &Path,
    context: &ModuleWorkerBuildContext,
) -> Result<ModulePreprocessingDiscovery> {
    if context.plugin_virtual_modules.is_empty() {
        return Ok(ModulePreprocessingDiscovery::default());
    }
    let virtual_path = project_root.join(".zfb-worker-virtual-module.mjs");
    let resolver = ProjectGraphResolver::new(project_root, &virtual_path, context, true, true);
    let mut pending = context
        .plugin_virtual_modules
        .iter()
        .map(|(specifier, _)| specifier.clone())
        .filter(|specifier| !resolver.user_claims_specifier(specifier))
        .collect::<BTreeSet<_>>();
    let mut visited = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut worker_edges = BTreeSet::new();
    let mut raw_import_edges = BTreeSet::new();
    let mut config_dependencies = BTreeSet::new();

    while let Some(specifier) = pending.pop_first() {
        if !visited.insert(specifier.clone()) {
            continue;
        }
        for dependency_specifier in
            validated_virtual_import_specifiers(context, &specifier, project_root)?
        {
            let Some(resolution) = resolver.resolve(
                &virtual_path,
                &dependency_specifier,
                GraphSpecifierKind::JavaScript,
            )?
            else {
                continue;
            };
            match resolution {
                GraphResolution::Virtual(nested) => {
                    if !visited.contains(&nested) {
                        pending.insert(nested);
                    }
                }
                GraphResolution::File(dependency) => {
                    let graph = inspect_worker_graph(&dependency, project_root, context, true)?;
                    files.extend(graph.files);
                    worker_edges.extend(graph.worker_edges);
                    raw_import_edges.extend(graph.raw_import_edges);
                    config_dependencies.extend(graph.config_files);
                }
                GraphResolution::RawFile(_) => unreachable!(
                    "query-bearing virtual imports are rejected before physical discovery"
                ),
            }
        }
    }

    Ok(ModulePreprocessingDiscovery {
        files: files.into_iter().collect(),
        worker_edges: worker_edges.into_iter().collect(),
        raw_import_edges: raw_import_edges.into_iter().collect(),
        config_dependencies: config_dependencies.into_iter().collect(),
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
    rewrite_module_worker_urls_with_context(
        source,
        importer,
        project_root,
        &ModuleWorkerBuildContext::default(),
    )
}

/// Context-aware form used by every production bundling pipeline.
pub fn rewrite_module_worker_urls_with_context(
    source: &str,
    importer: &Path,
    project_root: &Path,
    context: &ModuleWorkerBuildContext,
) -> Result<ModuleWorkerRewrite> {
    if !source.contains("Worker") || is_inside_node_modules(&normalize_path_lexical(importer)) {
        return Ok(empty_rewrite(source));
    }
    validate_first_party_path(importer, project_root, "module-worker importer")?;
    let (module, base, unresolved_ctxt) = match parse_module(importer, source) {
        Ok(parsed) => parsed,
        Err(error) => {
            fail_closed_unparseable_worker_source(importer, source, error)?;
            return Ok(empty_rewrite(source));
        }
    };
    let occurrences = collect_constructor_occurrences(&module, base, unresolved_ctxt);
    if occurrences.is_empty() {
        return Ok(empty_rewrite(source));
    }

    let mut replacements = Vec::new();
    let mut worker_edges = BTreeSet::new();
    let mut dependencies = BTreeSet::new();
    let mut raw_import_edges = BTreeSet::new();
    let mut config_dependencies = BTreeSet::new();
    for occurrence in occurrences {
        if occurrence.kind == ConstructorKind::SharedWorker {
            bail!(
                "zfb bundler: unsupported SharedWorker in {} for {:?}. Only `new Worker(new URL(\"./worker.ts\", import.meta.url), {{ type: \"module\" }})` is supported.",
                importer.display(),
                occurrence.specifier
            );
        }
        let worker = resolve_worker_target(importer, &occurrence.specifier, project_root)?;
        let graph = inspect_worker_graph(&worker, project_root, context, false)?;
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
        raw_import_edges.extend(graph.raw_import_edges);
        dependencies.extend(
            graph
                .files
                .into_iter()
                .map(|dependency| ModuleWorkerDependency {
                    importer: importer.to_path_buf(),
                    dependency,
                }),
        );
        config_dependencies.extend(graph.config_files.into_iter().map(|dependency| {
            ModuleWorkerDependency {
                importer: importer.to_path_buf(),
                dependency,
            }
        }));
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
        raw_import_edges: raw_import_edges.into_iter().collect(),
        config_dependencies: config_dependencies.into_iter().collect(),
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
    fn ts_importer_generic_arrow_without_worker_constructor_is_unchanged() {
        let project = tempfile::tempdir().unwrap();
        let importer = project.path().join("src/app.ts");
        write(&importer, "placeholder");
        let source =
            "const WorkerLabel = 'Worker';\nconst o = { m: async <T>() => {} };\nexport { o };\n";
        let rewrite = rewrite_module_worker_urls(source, &importer, project.path()).unwrap();
        assert_eq!(rewrite.expanded_source, source);
        assert!(rewrite.worker_edges.is_empty());
    }

    #[test]
    fn ts_importer_generic_arrow_with_worker_rewrites() {
        let project = tempfile::tempdir().unwrap();
        let importer = project.path().join("src/app.ts");
        let worker = project.path().join("src/worker.ts");
        write(&importer, "placeholder");
        write(&worker, "self.postMessage('ready');");
        let source = "const o = { m: async <T>() => {} };\nnew Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });\n";
        let rewrite = rewrite_module_worker_urls(source, &importer, project.path()).unwrap();
        assert!(rewrite
            .expanded_source
            .contains("new Worker(new URL(\"./worker-src-s-worker-d-ts.js?v="));
        assert_eq!(
            rewrite.worker_edges,
            vec![ModuleWorkerEdge {
                importer,
                source_path: worker,
            }]
        );
    }

    #[test]
    fn tsx_importer_with_jsx_still_rewrites_worker() {
        let project = tempfile::tempdir().unwrap();
        let importer = project.path().join("src/app.tsx");
        let worker = project.path().join("src/worker.ts");
        write(&importer, "placeholder");
        write(&worker, "self.postMessage('ready');");
        let source = "const view = <span />;\nnew Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });\nexport default view;\n";
        let rewrite = rewrite_module_worker_urls(source, &importer, project.path()).unwrap();
        assert!(rewrite.expanded_source.contains("const view = <span />;"));
        assert!(rewrite
            .expanded_source
            .contains("new Worker(new URL(\"./worker-src-s-worker-d-ts.js?v="));
    }

    #[test]
    fn unparseable_without_worker_constructor_is_unchanged() {
        let project = tempfile::tempdir().unwrap();
        let importer = project.path().join("src/app.ts");
        write(&importer, "placeholder");
        let source = "const WorkerLabel = 'Worker';\nconst broken = ;\n";
        let rewrite = rewrite_module_worker_urls(source, &importer, project.path()).unwrap();
        assert_eq!(rewrite.expanded_source, source);
        assert!(rewrite.worker_edges.is_empty());
    }

    #[test]
    fn unparseable_with_worker_constructor_text_fails_closed() {
        let project = tempfile::tempdir().unwrap();
        let importer = project.path().join("src/app.ts");
        write(&importer, "placeholder");
        let source = "const broken = ;\nnew Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });\n";
        let error = rewrite_module_worker_urls(source, &importer, project.path()).unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("cannot safely skip unparseable module-worker source"),
            "{message}"
        );
        assert!(message.contains("failed to parse"), "{message}");
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

    #[test]
    fn transform_only_changes_invalidate_worker_query() {
        let project = tempfile::tempdir().unwrap();
        let importer = project.path().join("src/app.ts");
        let worker = project.path().join("src/worker.ts");
        write(&importer, "placeholder");
        write(
            &worker,
            "self.postMessage(__WORKER_FLAG__ + import.meta.env.DEV);",
        );
        let source = "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });";
        let before = ModuleWorkerBuildContext::new(
            false,
            &BTreeMap::new(),
            &BTreeMap::from([("__WORKER_FLAG__".into(), "1".into())]),
            "preact",
        );
        let after = ModuleWorkerBuildContext::new(
            false,
            &BTreeMap::new(),
            &BTreeMap::from([("__WORKER_FLAG__".into(), "2".into())]),
            "preact",
        );
        let first =
            rewrite_module_worker_urls_with_context(source, &importer, project.path(), &before)
                .unwrap();
        let second =
            rewrite_module_worker_urls_with_context(source, &importer, project.path(), &after)
                .unwrap();
        assert_ne!(first.expanded_source, second.expanded_source);
    }

    #[test]
    fn browser_and_ssr_context_constructors_agree_for_same_config() {
        let loaders = BTreeMap::from([
            (".frag".to_string(), "text".to_string()),
            (".bin".to_string(), "binary".to_string()),
        ]);
        let define = BTreeMap::from([("__FLAG__".to_string(), "true".to_string())]);
        let plugins = vec![("worker:alias".to_string(), "/project/alias.ts".to_string())];
        let virtuals = vec![("virtual:worker".to_string(), "export default 1".to_string())];
        let browser = ModuleWorkerBuildContext::new(true, &loaders, &define, "preact")
            .with_plugins(plugins.clone(), virtuals.clone())
            .with_output_semantics(true, false);
        let loader_args = loaders
            .iter()
            .map(|(extension, loader)| format!("--loader:{extension}={loader}"))
            .collect::<Vec<_>>();
        let ssr = ModuleWorkerBuildContext::from_esbuild_loader_args(
            true,
            &loader_args,
            &define,
            "preact",
        )
        .with_plugins(plugins, virtuals)
        .with_output_semantics(true, false);
        assert_eq!(browser, ssr);
    }

    #[test]
    fn plugin_alias_target_is_watched_hashed_and_root_independent() {
        fn fixture(root: &Path) -> (PathBuf, ModuleWorkerBuildContext) {
            let importer = root.join("src/app.ts");
            let worker = root.join("src/worker.ts");
            let alias_target = root.join("lib/alias-helper.ts");
            write(&importer, "placeholder");
            write(
                &worker,
                "import { value } from 'worker:alias'; self.postMessage(value);",
            );
            write(&alias_target, "export const value = 'one';");
            let context = ModuleWorkerBuildContext::default().with_plugins(
                vec![(
                    "worker:alias".into(),
                    alias_target.to_string_lossy().into_owned(),
                )],
                Vec::new(),
            );
            (importer, context)
        }

        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        let (left_importer, left_context) = fixture(left.path());
        let (right_importer, right_context) = fixture(right.path());
        let source = "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });";
        let first = rewrite_module_worker_urls_with_context(
            source,
            &left_importer,
            left.path(),
            &left_context,
        )
        .unwrap();
        let relocated = rewrite_module_worker_urls_with_context(
            source,
            &right_importer,
            right.path(),
            &right_context,
        )
        .unwrap();
        assert_eq!(first.expanded_source, relocated.expanded_source);
        assert!(first.dependencies.iter().any(|dependency| {
            dependency.dependency == left.path().join("lib/alias-helper.ts")
        }));

        write(
            &left.path().join("lib/alias-helper.ts"),
            "export const value = 'two';",
        );
        let changed = rewrite_module_worker_urls_with_context(
            source,
            &left_importer,
            left.path(),
            &left_context,
        )
        .unwrap();
        assert_ne!(first.expanded_source, changed.expanded_source);
    }

    #[test]
    fn worker_graph_uses_nearest_nested_config_for_alias_closure() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let importer = root.join("src/app.ts");
        let worker = root.join("src/worker.ts");
        let nested_helper = root.join("src/nested/helper.ts");
        let root_helper = root.join("lib/helper.ts");
        write(&importer, "placeholder");
        write(
            &worker,
            "import { value } from '@worker/helper'; self.postMessage(value);",
        );
        write(&nested_helper, "export const value = 'nested-one';");
        write(&root_helper, "export const value = 'root-one';");
        write(
            &root.join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@worker/*":["lib/*"]}}}"#,
        );
        write(
            &root.join("src/tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@worker/*":["nested/*"]}}}"#,
        );
        let source = "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });";
        let context = ModuleWorkerBuildContext::default();
        let first =
            rewrite_module_worker_urls_with_context(source, &importer, root, &context).unwrap();
        assert!(first
            .dependencies
            .iter()
            .any(|dependency| dependency.dependency == nested_helper));
        assert!(!first
            .dependencies
            .iter()
            .any(|dependency| dependency.dependency == root_helper));

        write(&root_helper, "export const value = 'root-two';");
        let root_changed =
            rewrite_module_worker_urls_with_context(source, &importer, root, &context).unwrap();
        assert_eq!(first.expanded_source, root_changed.expanded_source);
        write(&nested_helper, "export const value = 'nested-two';");
        let nested_changed =
            rewrite_module_worker_urls_with_context(source, &importer, root, &context).unwrap();
        assert_ne!(first.expanded_source, nested_changed.expanded_source);
    }

    #[test]
    fn virtual_source_and_its_project_helper_invalidate_worker_query() {
        let project = tempfile::tempdir().unwrap();
        let importer = project.path().join("src/app.ts");
        let worker = project.path().join("src/worker.ts");
        let helper = project.path().join("src/virtual-helper.ts");
        write(&importer, "placeholder");
        write(
            &worker,
            "import { value } from 'virtual:worker-data'; self.postMessage(value);",
        );
        write(&helper, "export const helper = 'one';");
        let source = "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });";
        let context = ModuleWorkerBuildContext::default().with_plugins(
            Vec::new(),
            vec![(
                "virtual:worker-data".into(),
                "import { helper } from './src/virtual-helper.ts'; export const value = helper;"
                    .into(),
            )],
        );
        let first =
            rewrite_module_worker_urls_with_context(source, &importer, project.path(), &context)
                .unwrap();
        assert!(first
            .dependencies
            .iter()
            .any(|dependency| dependency.dependency == helper));

        write(&helper, "export const helper = 'two';");
        let helper_changed =
            rewrite_module_worker_urls_with_context(source, &importer, project.path(), &context)
                .unwrap();
        assert_ne!(first.expanded_source, helper_changed.expanded_source);

        let virtual_changed = ModuleWorkerBuildContext::default().with_plugins(
            Vec::new(),
            vec![(
                "virtual:worker-data".into(),
                "import { helper } from './src/virtual-helper.ts'; export const value = helper + '!';"
                    .into(),
            )],
        );
        let virtual_changed = rewrite_module_worker_urls_with_context(
            source,
            &importer,
            project.path(),
            &virtual_changed,
        )
        .unwrap();
        assert_ne!(
            helper_changed.expanded_source,
            virtual_changed.expanded_source
        );
    }

    #[test]
    fn plugin_alias_entry_discovery_reports_raw_and_nested_worker_edges() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let entry = root.join("src/client.ts");
        let alias = root.join("plugin/entry.ts");
        let payload = root.join("plugin/payload.txt");
        let worker = root.join("plugin/nested.worker.ts");
        write(&entry, "import 'plugin:entry';");
        write(
            &alias,
            "import payload from './payload.txt?raw'; new Worker(new URL('./nested.worker.ts', import.meta.url), { type: 'module' }); export { payload };",
        );
        write(&payload, "plugin raw payload");
        write(&worker, "self.postMessage('nested');");
        let context = ModuleWorkerBuildContext::default().with_plugins(
            vec![("plugin:entry".into(), alias.to_string_lossy().into_owned())],
            Vec::new(),
        );

        let discovery = discover_module_preprocessing_with_context(&entry, root, &context).unwrap();
        assert!(discovery
            .raw_import_edges
            .contains(&ModuleWorkerRawImportEdge {
                importer: alias.clone(),
                target: payload.clone(),
            }));
        assert!(discovery.worker_edges.contains(&ModuleWorkerEdge {
            importer: alias,
            source_path: worker,
        }));
    }

    #[test]
    fn discovery_ts_generic_arrow_with_raw_import_reports_raw_edge() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let entry = root.join("src/client.ts");
        let payload = root.join("src/payload.txt");
        write(
            &entry,
            "import payload from './payload.txt?raw';\nconst o = { m: async <T>() => {} };\nexport { payload, o };\n",
        );
        write(&payload, "payload");

        let discovery = discover_module_preprocessing_with_context(
            &entry,
            root,
            &ModuleWorkerBuildContext::default(),
        )
        .unwrap();
        assert!(discovery
            .raw_import_edges
            .contains(&ModuleWorkerRawImportEdge {
                importer: entry,
                target: payload,
            }));
    }

    #[test]
    fn module_worker_raw_import_resolves_tsconfig_alias_exact_file() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let importer = root.join("src/app.ts");
        let worker = root.join("src/worker.ts");
        let payload = root.join("src/payload.txt");
        write(
            &root.join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/*"]}}}"#,
        );
        write(&importer, "placeholder");
        write(
            &worker,
            "import payload from '@/payload.txt?raw'; self.postMessage(payload);",
        );
        write(&payload, "aliased worker raw");

        let rewrite = rewrite_module_worker_urls(
            "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });",
            &importer,
            root,
        )
        .unwrap();

        assert!(rewrite
            .raw_import_edges
            .contains(&ModuleWorkerRawImportEdge {
                importer: worker,
                target: payload.clone(),
            }));
        assert!(rewrite
            .dependencies
            .iter()
            .any(|edge| edge.dependency == payload));
    }

    #[test]
    fn module_worker_raw_alias_does_not_probe_extensions() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let importer = root.join("src/app.ts");
        let worker = root.join("src/worker.ts");
        write(
            &root.join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/*"]}}}"#,
        );
        write(&importer, "placeholder");
        write(
            &worker,
            "import payload from '@/payload?raw'; self.postMessage(payload);",
        );
        write(&root.join("src/payload.txt"), "must not be probed");

        let error = rewrite_module_worker_urls(
            "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });",
            &importer,
            root,
        )
        .unwrap_err()
        .to_string();

        assert!(
            error.contains("matched a first-party mapping but no target resolved"),
            "{error}"
        );
    }

    #[test]
    fn discovery_unparseable_dependency_without_worker_constructor_is_tolerated() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let entry = root.join("src/client.ts");
        let dependency = root.join("src/broken.ts");
        write(&entry, "import './broken';\nexport const ok = true;\n");
        write(
            &dependency,
            "const WorkerLabel = 'Worker';\nconst broken = ;\n",
        );

        let discovery = discover_module_preprocessing_with_context(
            &entry,
            root,
            &ModuleWorkerBuildContext::default(),
        )
        .unwrap();
        assert!(discovery.files.contains(&dependency));
        assert!(discovery.worker_edges.is_empty());
    }

    #[test]
    fn discovery_unparseable_dependency_with_worker_constructor_fails_closed() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let entry = root.join("src/client.ts");
        let dependency = root.join("src/broken.ts");
        write(&entry, "import './broken';\nexport const ok = true;\n");
        write(
            &dependency,
            "const broken = ;\nnew SharedWorker(new URL('./worker.ts', import.meta.url));\n",
        );

        let error = discover_module_preprocessing_with_context(
            &entry,
            root,
            &ModuleWorkerBuildContext::default(),
        )
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("cannot safely skip unparseable module-worker source"),
            "{message}"
        );
        assert!(message.contains("new Worker") || message.contains("new SharedWorker"));
    }

    #[test]
    fn virtual_module_query_preprocessing_fails_before_worker_hashing() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let importer = root.join("src/app.ts");
        let worker = root.join("src/worker.ts");
        write(&importer, "placeholder");
        write(
            &worker,
            "import value from 'virtual:worker'; self.postMessage(value);",
        );
        let context = ModuleWorkerBuildContext::default().with_plugins(
            Vec::new(),
            vec![(
                "virtual:worker".into(),
                "import value from './payload.txt?raw'; export default value;".into(),
            )],
        );
        let source = "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });";

        let error =
            rewrite_module_worker_urls_with_context(source, &importer, root, &context).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("query-bearing import \"./payload.txt?raw\" inside plugin virtual module \"virtual:worker\" is unsupported"),
            "{error:#}"
        );
    }

    #[test]
    fn global_virtual_validation_rejects_type_only_query_imports() {
        let project = tempfile::tempdir().unwrap();
        let context = ModuleWorkerBuildContext::default().with_plugins(
            Vec::new(),
            vec![(
                "virtual:type-query".into(),
                "import type Payload from './payload.txt?raw'; export default 1;".into(),
            )],
        );

        let error =
            discover_registered_virtual_preprocessing_with_context(project.path(), &context)
                .unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains(
                "unsupported query syntax inside plugin virtual module \"virtual:type-query\""
            ),
            "{message}"
        );
        assert!(
            message.contains("is not a single static default import"),
            "{message}"
        );
    }

    #[test]
    fn global_virtual_validation_rejects_computed_query_imports() {
        let project = tempfile::tempdir().unwrap();
        let context = ModuleWorkerBuildContext::default().with_plugins(
            Vec::new(),
            vec![(
                "virtual:computed-query".into(),
                "const suffix = 'raw'; export default import(`./payload.txt?${suffix}`);".into(),
            )],
        );

        let error =
            discover_registered_virtual_preprocessing_with_context(project.path(), &context)
                .unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains(
                "unsupported query syntax inside plugin virtual module \"virtual:computed-query\""
            ),
            "{message}"
        );
        assert!(message.contains("dynamic import"), "{message}");
    }

    #[test]
    fn global_virtual_validation_rejects_unparseable_query_sources() {
        let project = tempfile::tempdir().unwrap();
        let context = ModuleWorkerBuildContext::default().with_plugins(
            Vec::new(),
            vec![(
                "virtual:broken-query".into(),
                "const broken = ;\n// import value from './payload.txt?raw';".into(),
            )],
        );

        let error =
            discover_registered_virtual_preprocessing_with_context(project.path(), &context)
                .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("failed to parse"), "{message}");
        assert!(
            message.contains(".zfb-worker-virtual-module.mjs"),
            "{message}"
        );
    }

    #[test]
    fn effective_config_chain_is_hashed_watched_and_root_independent() {
        fn fixture(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
            let importer = root.join("src/app.ts");
            let worker = root.join("src/worker.ts");
            let package = root.join("node_modules/@scope/worker-config");
            write(&importer, "placeholder");
            write(
                &worker,
                "class Box { value = 1 } self.postMessage(new Box().value);",
            );
            write(
                &package.join("package.json"),
                r#"{"name":"@scope/worker-config","tsconfig":"base.json"}"#,
            );
            write(
                &package.join("base.json"),
                r#"{"compilerOptions":{"useDefineForClassFields":false}}"#,
            );
            write(
                &root.join("config/shared.json"),
                r#"{"compilerOptions":{"jsx":"automatic"}}"#,
            );
            write(
                &root.join("src/jsconfig.json"),
                r#"{"extends":["@scope/worker-config","../config/shared.json"]}"#,
            );
            (importer, worker, package.join("base.json"))
        }

        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        let (left_importer, _, left_package_config) = fixture(left.path());
        let (right_importer, _, _) = fixture(right.path());
        let source = "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });";
        let first = rewrite_module_worker_urls(source, &left_importer, left.path()).unwrap();
        let relocated = rewrite_module_worker_urls(source, &right_importer, right.path()).unwrap();
        assert_eq!(first.expanded_source, relocated.expanded_source);
        for config in [
            left.path().join("src/jsconfig.json"),
            left.path().join("config/shared.json"),
            left.path()
                .join("node_modules/@scope/worker-config/package.json"),
            left_package_config.clone(),
        ] {
            assert!(first
                .config_dependencies
                .iter()
                .any(|dependency| dependency.dependency == config));
        }

        write(
            &left_package_config,
            r#"{"compilerOptions":{"useDefineForClassFields":true}}"#,
        );
        let changed = rewrite_module_worker_urls(source, &left_importer, left.path()).unwrap();
        assert_ne!(first.expanded_source, changed.expanded_source);
    }

    #[test]
    fn ancestor_only_config_is_hashed_and_returned_for_invalidation() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().join("project");
        let importer = root.join("src/app.ts");
        let worker = root.join("src/worker.ts");
        let config = workspace.path().join("jsconfig.json");
        write(&importer, "placeholder");
        write(
            &worker,
            "class Box { value = 1 } self.postMessage(new Box().value);",
        );
        write(
            &config,
            r#"{"compilerOptions":{"useDefineForClassFields":false}}"#,
        );
        let source = "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });";

        let first = rewrite_module_worker_urls(source, &importer, &root).unwrap();
        assert!(first
            .config_dependencies
            .iter()
            .any(|dependency| dependency.dependency == config));
        write(
            &config,
            r#"{"compilerOptions":{"useDefineForClassFields":true}}"#,
        );
        let changed = rewrite_module_worker_urls(source, &importer, &root).unwrap();
        assert_ne!(first.expanded_source, changed.expanded_source);
    }

    #[test]
    fn nearest_config_precedence_candidate_survives_create_delete_recreate() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let importer = root.join("src/app.ts");
        let worker = root.join("src/worker.ts");
        let jsconfig = root.join("src/jsconfig.json");
        let tsconfig = root.join("src/tsconfig.json");
        write(&importer, "placeholder");
        write(
            &worker,
            "class Box { value = 1 } self.postMessage(new Box().value);",
        );
        write(
            &jsconfig,
            r#"{"compilerOptions":{"useDefineForClassFields":false}}"#,
        );
        let source = "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });";

        let fallback = rewrite_module_worker_urls(source, &importer, root).unwrap();
        assert!(fallback
            .config_dependencies
            .iter()
            .any(|dependency| dependency.dependency == tsconfig));

        write(
            &tsconfig,
            r#"{"compilerOptions":{"useDefineForClassFields":true}}"#,
        );
        let preferred = rewrite_module_worker_urls(source, &importer, root).unwrap();
        assert_ne!(fallback.expanded_source, preferred.expanded_source);

        std::fs::remove_file(&tsconfig).unwrap();
        let deleted = rewrite_module_worker_urls(source, &importer, root).unwrap();
        assert_eq!(fallback.expanded_source, deleted.expanded_source);
        assert!(deleted
            .config_dependencies
            .iter()
            .any(|dependency| dependency.dependency == tsconfig));

        write(
            &tsconfig,
            r#"{"compilerOptions":{"useDefineForClassFields":true}}"#,
        );
        let recreated = rewrite_module_worker_urls(source, &importer, root).unwrap();
        assert_eq!(preferred.expanded_source, recreated.expanded_source);
    }

    #[test]
    fn extensionless_extends_precedence_probe_survives_create_delete_recreate() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let importer = root.join("src/app.ts");
        let worker = root.join("src/worker.ts");
        let raw_config = root.join("src/worker-base");
        let directory_config = raw_config.join("tsconfig.json");
        let fallback_config = root.join("src/worker-base.json");
        write(&importer, "placeholder");
        write(
            &worker,
            "class Box { value = 1 } self.postMessage(new Box().value);",
        );
        write(
            &root.join("src/jsconfig.json"),
            r#"{"extends":"./worker-base"}"#,
        );
        write(
            &fallback_config,
            r#"{"compilerOptions":{"useDefineForClassFields":false}}"#,
        );
        let source = "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });";

        let fallback = rewrite_module_worker_urls(source, &importer, root).unwrap();
        for expected in [&raw_config, &fallback_config, &directory_config] {
            assert!(fallback
                .config_dependencies
                .iter()
                .any(|dependency| dependency.dependency == *expected));
        }

        write(
            &raw_config,
            r#"{"compilerOptions":{"useDefineForClassFields":true}}"#,
        );
        let preferred = rewrite_module_worker_urls(source, &importer, root).unwrap();
        assert_ne!(fallback.expanded_source, preferred.expanded_source);

        std::fs::remove_file(&raw_config).unwrap();
        let deleted = rewrite_module_worker_urls(source, &importer, root).unwrap();
        assert_eq!(fallback.expanded_source, deleted.expanded_source);
        assert!(deleted
            .config_dependencies
            .iter()
            .any(|dependency| dependency.dependency == raw_config));

        write(
            &raw_config,
            r#"{"compilerOptions":{"useDefineForClassFields":true}}"#,
        );
        let recreated = rewrite_module_worker_urls(source, &importer, root).unwrap();
        assert_eq!(preferred.expanded_source, recreated.expanded_source);
    }

    #[test]
    fn package_config_metadata_survives_delete_fallback_and_recreate() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let importer = root.join("src/app.ts");
        let worker = root.join("src/worker.ts");
        let package = root.join("node_modules/worker-config");
        let package_json = package.join("package.json");
        let fallback_config = package.join("tsconfig.json");
        let config_a = package.join("a.json");
        let config_b = package.join("b.json");
        write(&importer, "placeholder");
        write(
            &worker,
            "class Box { value = 1 } self.postMessage(new Box().value);",
        );
        write(
            &root.join("src/jsconfig.json"),
            r#"{"extends":"worker-config"}"#,
        );
        write(
            &config_a,
            r#"{"compilerOptions":{"useDefineForClassFields":false}}"#,
        );
        write(
            &config_b,
            r#"{"compilerOptions":{"useDefineForClassFields":true}}"#,
        );
        write(
            &fallback_config,
            r#"{"compilerOptions":{"target":"es2020"}}"#,
        );
        write(&package_json, r#"{"tsconfig":"a.json"}"#);
        let source = "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });";

        let redirected_a = rewrite_module_worker_urls(source, &importer, root).unwrap();
        std::fs::remove_file(&package_json).unwrap();
        let fallback = rewrite_module_worker_urls(source, &importer, root).unwrap();
        assert_ne!(redirected_a.expanded_source, fallback.expanded_source);
        for expected in [&package_json, &fallback_config] {
            assert!(fallback
                .config_dependencies
                .iter()
                .any(|dependency| dependency.dependency == *expected));
        }

        write(&package_json, r#"{"tsconfig":"b.json"}"#);
        let redirected_b = rewrite_module_worker_urls(source, &importer, root).unwrap();
        assert_ne!(fallback.expanded_source, redirected_b.expanded_source);
        assert_ne!(redirected_a.expanded_source, redirected_b.expanded_source);
        for expected in [&package_json, &config_b] {
            assert!(redirected_b
                .config_dependencies
                .iter()
                .any(|dependency| dependency.dependency == *expected));
        }
    }

    #[test]
    fn user_claimed_virtual_keeps_native_transitive_config_hashing() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let importer = root.join("src/app.ts");
        let worker = root.join("src/worker.ts");
        let claimed = root.join("src/claimed.ts");
        let helper = root.join("src/nested/helper.ts");
        let nested_config = root.join("src/nested/tsconfig.json");
        write(&importer, "placeholder");
        write(
            &root.join("src/tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"virtual:claimed":["./claimed.ts"]}}}"#,
        );
        write(&claimed, "export const claimed = 'USER_WINS';");
        write(
            &worker,
            "import { claimed } from 'virtual:claimed'; import { value } from './nested/helper.ts'; self.postMessage(claimed + value);",
        );
        write(
            &helper,
            "class Box { value = 1 } export const value = new Box().value;",
        );
        write(
            &nested_config,
            r#"{"compilerOptions":{"useDefineForClassFields":false}}"#,
        );
        let context = ModuleWorkerBuildContext::default().with_plugins(
            Vec::new(),
            vec![(
                "virtual:claimed".into(),
                "export const claimed = 'LOSING_PLUGIN';".into(),
            )],
        );
        let source = "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });";

        let first =
            rewrite_module_worker_urls_with_context(source, &importer, root, &context).unwrap();
        assert!(first
            .config_dependencies
            .iter()
            .any(|dependency| dependency.dependency == nested_config));

        write(
            &nested_config,
            r#"{"compilerOptions":{"useDefineForClassFields":true}}"#,
        );
        let changed =
            rewrite_module_worker_urls_with_context(source, &importer, root, &context).unwrap();
        assert_ne!(first.expanded_source, changed.expanded_source);
    }
}
