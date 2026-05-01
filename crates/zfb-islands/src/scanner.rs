//! `"use client"` AST scanner.
//!
//! Given a list of page entry paths and a [`Resolver`], [`scan_islands`]
//! walks every component imported (transitively) from every page and
//! returns the deterministic, sorted islands set.
//!
//! ## Public-API contract
//!
//! - **Input**:
//!   - `pages: &[PathBuf]` — entry paths (typically absolute) the caller
//!     considers page roots. Whatever path representation the caller
//!     hands in is the same representation that comes back inside
//!     [`crate::Island::source_path`].
//!   - `resolver: &impl Resolver` — abstracts both module resolution and
//!     source reading. The default [`FsResolver`] hits the file system;
//!     tests use [`InMemoryResolver`].
//! - **Output**: an [`IslandsSet`] (`Vec<Island>`), sorted by
//!   `(source_path, component_name)`. The order is byte-stable across runs
//!   for a given input — downstream hashing (Sub 2) relies on this.
//!
//! ## "use client" detection
//!
//! A source file is an islands entry if and only if its leading directive
//! prologue contains a string-literal expression statement whose value
//! equals `"use client"` — same rule Next.js uses. The rules:
//!
//! - The prologue is the run of expression-statement string literals at
//!   the top of the module. As soon as a non-string-literal-expr-stmt
//!   appears (a real statement, an import, anything else), the prologue
//!   ends.
//! - Both single and double quotes are tolerated; the AST stores the
//!   parsed value, not the source token, so quote style is invisible at
//!   this level.
//! - Other directives such as `"use strict"` may appear before, after, or
//!   alongside `"use client"` in the prologue. We accept `"use client"` if
//!   it appears anywhere in the prologue.
//! - Comments before the directive are ignored — SWC's parser leaves them
//!   out of the module body.
//!
//! ## Component identity
//!
//! Each `"use client"` file contributes one [`crate::Island`] per *exported
//! binding name*. Specifically:
//!
//! - `export function Foo() {}` → `"Foo"`
//! - `export class Foo {}` → `"Foo"`
//! - `export const Foo = …` (and `let` / `var`) → `"Foo"`
//! - `export default …` → the literal string `"default"`
//! - `export { A, B as C }` → `"A"`, `"C"`
//! - `export * as ns from "./mod"` → `"ns"`
//! - `export d from "./mod"` (rare) → `"d"`
//!
//! The pair `(source_path, component_name)` is the stable identity used
//! for dedup and ordering. Re-exports without a local declaration of the
//! same component (e.g. `export { X } from "./other"`) do contribute their
//! exported name; the bundler downstream sees the same file path and
//! resolves the actual implementation through its own module resolver.
//!
//! ## Cycles and dedup
//!
//! The scanner tracks a visited set keyed by the resolved path the
//! resolver returned, so cyclic imports terminate. Dedup is performed on
//! the `(source_path, component_name)` pair via a `BTreeMap`, which also
//! gives us the sort order on output for free.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use swc_core::atoms::Wtf8Atom;
use swc_core::common::sync::Lrc;
use swc_core::common::{FileName, SourceMap};
use swc_core::ecma::ast::{
    Decl, EsVersion, Expr, ExportSpecifier, Lit, Module, ModuleDecl, ModuleExportName, ModuleItem,
    Pat, Stmt,
};
use swc_core::ecma::parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};
use thiserror::Error;

use crate::bundler::Island;

/// Errors raised by [`scan_islands`].
#[derive(Debug, Error)]
pub enum ScanError {
    /// The resolver returned an error reading the given path.
    #[error("resolver failed for {path}: {message}")]
    Resolver {
        /// The path the scanner asked the resolver for.
        path: PathBuf,
        /// Stringified error message returned by the resolver.
        message: String,
    },
    /// SWC could not parse the source as TS/TSX.
    #[error("parse failed for {path}: {message}")]
    Parse {
        /// Path of the source that failed to parse.
        path: PathBuf,
        /// Diagnostic message from SWC.
        message: String,
    },
}

/// Convenience alias for scanner results.
pub type ScanResult<T> = std::result::Result<T, ScanError>;

/// The result of [`scan_islands`].
pub type IslandsSet = Vec<Island>;

/// Abstraction over module resolution + source reading.
///
/// The scanner doesn't know how a project lays out its files (real FS,
/// virtual FS, an in-memory test harness) — it asks the resolver. Two
/// methods, deliberately split:
///
/// - [`Resolver::resolve`] turns a `(importer_dir, specifier)` pair into a
///   resolved path the scanner will use as both the visited-set key and
///   the [`Island::source_path`].
/// - [`Resolver::read`] reads a previously-resolved path back as a string.
pub trait Resolver {
    /// Resolve a relative `specifier` (`./foo`, `../bar/baz`) against the
    /// importer's directory. Return `None` for bare specifiers (e.g.
    /// `preact`, `react`, `zfb`) or unresolvable paths — the scanner will
    /// then skip them.
    fn resolve(&self, importer_dir: &Path, specifier: &str) -> Option<PathBuf>;

    /// Read the source content for a previously-resolved path.
    ///
    /// The error type is `String` (rather than `std::io::Error` or a
    /// trait-specific type) so test resolvers and FS resolvers can both
    /// produce the same shape without lossy conversions.
    fn read(&self, path: &Path) -> std::result::Result<String, String>;
}

/// Lexically normalise a path by collapsing `.` and `..` components
/// without consulting the filesystem.
///
/// `..` is only popped when the previous component is a normal name; a
/// `..` after a root or after another preserved `..` is left in place
/// (so we never escape above a relative root). Useful both for the
/// in-memory test resolver — where `/proj/pages/../components/x` must
/// match the same key as `/proj/components/x` — and as a public helper
/// for downstream code that wants the same dedup behaviour.
pub fn normalize_path_lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => {
                out.push(comp.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                let last_is_normal = out
                    .components()
                    .next_back()
                    .map(|c| matches!(c, Component::Normal(_)))
                    .unwrap_or(false);
                if last_is_normal {
                    out.pop();
                } else {
                    out.push(comp.as_os_str());
                }
            }
            Component::Normal(name) => out.push(name),
        }
    }
    out
}

/// Whether a specifier is bare (no `./`, `../`, or `/` prefix).
///
/// Bare specifiers are runtime-provided by the framework adapter (see
/// `zfb_render::loader`) and do not point at files on disk; the scanner
/// must skip them to avoid spurious resolver errors.
pub fn is_bare_specifier(specifier: &str) -> bool {
    !(specifier.starts_with("./") || specifier.starts_with("../") || specifier.starts_with('/'))
}

/// Filesystem-backed [`Resolver`].
///
/// Probes the same extensions as `zfb_render`'s loader: `.tsx`, `.ts`,
/// `.jsx`, `.js`, plus `index.<ext>` inside a directory.
///
/// In addition to relative-path resolution, this resolver understands
/// pnpm-workspace consumer layouts: when handed a bare specifier (e.g.
/// `@scope/pkg` or `pkg/sub`) it walks up from `importer_dir` looking
/// for `node_modules/<specifier>` and probes the symlinked package's
/// `package.json` (`source` → `module` → `main` → `index`) for a
/// scannable `.tsx` / `.ts` entry. This is what lets a pnpm-workspace
/// consumer's page module `import "@takazudo/zfb-blog-islands"` route
/// the scanner into the workspace package's source `.tsx` files so any
/// `"use client"` islands actually emit a production bundle.
#[derive(Debug, Clone)]
pub struct FsResolver {
    /// File extensions probed for relative imports without an extension.
    pub probe_exts: Vec<String>,
    /// Whether bare specifiers are routed through the pnpm-workspace
    /// `node_modules/<specifier>` probe. Defaults to `true`; tests that
    /// want the legacy "skip every bare specifier" shape can flip it
    /// off via [`FsResolver::without_workspace_probe`].
    pub workspace_probe_enabled: bool,
}

impl Default for FsResolver {
    fn default() -> Self {
        Self {
            probe_exts: vec![
                ".tsx".to_string(),
                ".ts".to_string(),
                ".jsx".to_string(),
                ".js".to_string(),
            ],
            workspace_probe_enabled: true,
        }
    }
}

impl FsResolver {
    /// Construct with the default extension probe order and
    /// pnpm-workspace probe enabled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct with the pnpm-workspace probe explicitly disabled —
    /// matches the pre-#122 behaviour where bare specifiers always
    /// resolved to `None`.
    pub fn without_workspace_probe() -> Self {
        Self {
            workspace_probe_enabled: false,
            ..Self::default()
        }
    }

    /// Walk up from `start_dir` looking for the first ancestor that
    /// contains `node_modules/<specifier>` and return that path. The
    /// path is *not* canonicalised here — callers do that after
    /// probing for an actual entry file inside.
    fn locate_node_modules_pkg(start_dir: &Path, specifier: &str) -> Option<PathBuf> {
        let mut dir: Option<&Path> = Some(start_dir);
        while let Some(d) = dir {
            let candidate = d.join("node_modules").join(specifier);
            if candidate.exists() {
                return Some(candidate);
            }
            dir = d.parent();
        }
        None
    }

    /// Split a bare specifier into `(package, subpath)` where the
    /// package portion is the name pnpm resolves under `node_modules/`
    /// (`pkg` or `@scope/pkg`) and the subpath is the rest of the
    /// specifier (empty when none).
    fn split_bare_specifier(specifier: &str) -> (String, String) {
        if let Some(rest) = specifier.strip_prefix('@') {
            // Scoped: @scope/pkg[/sub]
            let mut parts = rest.splitn(3, '/');
            let scope = parts.next().unwrap_or("");
            let pkg = parts.next().unwrap_or("");
            let pkg_name = format!("@{scope}/{pkg}");
            let sub = parts.next().unwrap_or("");
            (pkg_name, sub.to_string())
        } else {
            // Plain: pkg[/sub]
            let mut parts = specifier.splitn(2, '/');
            let pkg = parts.next().unwrap_or("").to_string();
            let sub = parts.next().unwrap_or("").to_string();
            (pkg, sub)
        }
    }

    /// Probe an installed package directory for the specifier's actual
    /// source entry point.
    ///
    /// Subpath imports (`@scope/pkg/components/foo`) probe directly
    /// against `<pkg_dir>/<subpath>` plus the standard extension list.
    /// Bare-package imports (no subpath) read `package.json` and try
    /// `source` (the convention pnpm-workspace TypeScript packages use
    /// for un-built sources), then `module`, then `main`. If
    /// `package.json` is missing or doesn't point at a scannable file,
    /// fall back to probing `src/index.<ext>` and `index.<ext>` inside
    /// the package root — both common shapes for un-built workspace
    /// packages.
    fn probe_package_entry(&self, pkg_dir: &Path, subpath: &str) -> Option<PathBuf> {
        if !subpath.is_empty() {
            let candidate = pkg_dir.join(subpath);
            return Self::probe_with_extensions(&candidate, &self.probe_exts);
        }

        // 1) package.json hints — read once, try fields in priority order.
        let pkg_json_path = pkg_dir.join("package.json");
        if let Ok(text) = std::fs::read_to_string(&pkg_json_path) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                for field in ["source", "module", "main"] {
                    if let Some(rel) = value.get(field).and_then(|v| v.as_str()) {
                        let candidate = pkg_dir.join(rel);
                        if let Some(found) = Self::probe_with_extensions(&candidate, &self.probe_exts)
                        {
                            return Some(found);
                        }
                    }
                }
            }
        }

        // 2) Conventional un-built workspace shapes.
        for prefix in ["src/index", "index"] {
            let candidate = pkg_dir.join(prefix);
            if let Some(found) = Self::probe_with_extensions(&candidate, &self.probe_exts) {
                return Some(found);
            }
        }
        None
    }

    /// Try `path` as-is, then with each extension appended; return
    /// the first hit that is a regular file.
    fn probe_with_extensions(path: &Path, exts: &[String]) -> Option<PathBuf> {
        if path.is_file() {
            return Some(path.to_path_buf());
        }
        for ext in exts {
            let mut probe = path.to_path_buf();
            let new_name = format!(
                "{}{}",
                probe.file_name().and_then(|s| s.to_str()).unwrap_or(""),
                ext
            );
            probe.set_file_name(new_name);
            if probe.is_file() {
                return Some(probe);
            }
        }
        None
    }
}

impl Resolver for FsResolver {
    fn resolve(&self, importer_dir: &Path, specifier: &str) -> Option<PathBuf> {
        // Helper: turn a found path into the canonical (symlink-resolved,
        // `..`-collapsed) form so the visited set and Island::source_path
        // are stable regardless of how many times the same file is reached
        // through different specifiers.
        let canonicalize = |p: PathBuf| p.canonicalize().unwrap_or(p);

        if is_bare_specifier(specifier) {
            if !self.workspace_probe_enabled {
                return None;
            }
            // Ignore obviously-not-on-disk specifiers that surface in
            // every project (the framework-provided ones zfb_render's
            // loader fakes). Walking node_modules for these is wasted
            // work; failing the probe also means we skip them silently.
            let (pkg_name, subpath) = Self::split_bare_specifier(specifier);
            let pkg_dir = Self::locate_node_modules_pkg(importer_dir, &pkg_name)?;
            return self.probe_package_entry(&pkg_dir, &subpath).map(canonicalize);
        }

        let candidate = importer_dir.join(specifier);

        // 1) Exact match.
        if candidate.is_file() {
            return Some(canonicalize(candidate));
        }

        // 2) Probe extensions.
        for ext in &self.probe_exts {
            let mut probe = candidate.clone();
            let new_name = format!(
                "{}{}",
                probe.file_name().and_then(|s| s.to_str()).unwrap_or(""),
                ext
            );
            probe.set_file_name(new_name);
            if probe.is_file() {
                return Some(canonicalize(probe));
            }
        }

        // 3) `index.<ext>` inside a directory.
        if candidate.is_dir() {
            for ext in &self.probe_exts {
                let probe = candidate.join(format!("index{ext}"));
                if probe.is_file() {
                    return Some(canonicalize(probe));
                }
            }
        }

        None
    }

    fn read(&self, path: &Path) -> std::result::Result<String, String> {
        std::fs::read_to_string(path).map_err(|e| e.to_string())
    }
}

/// In-memory [`Resolver`] for tests and integration harnesses.
///
/// Resolution probes the same extension list as [`FsResolver`], but
/// against the in-memory `files` map instead of the file system.
///
/// Note on path normalization: where [`FsResolver`] uses
/// `std::fs::canonicalize` (which also resolves symlinks), this resolver
/// only collapses `.` / `..` lexically via [`normalize_path_lexical`].
/// That's enough for the tests' synthetic paths but is intentionally
/// less powerful than the real-FS path; production code should always
/// use [`FsResolver`].
#[derive(Debug, Clone, Default)]
pub struct InMemoryResolver {
    /// Map of resolved-path → source text.
    pub files: HashMap<PathBuf, String>,
    /// Probe extensions; mirrors [`FsResolver::probe_exts`].
    pub probe_exts: Vec<String>,
}

impl InMemoryResolver {
    /// Construct a fresh resolver with the standard probe extensions.
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
            probe_exts: vec![
                ".tsx".to_string(),
                ".ts".to_string(),
                ".jsx".to_string(),
                ".js".to_string(),
            ],
        }
    }

    /// Insert a source file into the map (chainable).
    pub fn with_file(mut self, path: impl Into<PathBuf>, source: impl Into<String>) -> Self {
        self.files.insert(path.into(), source.into());
        self
    }

    /// Insert a source file (mutating).
    pub fn insert(&mut self, path: impl Into<PathBuf>, source: impl Into<String>) {
        self.files.insert(path.into(), source.into());
    }
}

impl Resolver for InMemoryResolver {
    fn resolve(&self, importer_dir: &Path, specifier: &str) -> Option<PathBuf> {
        if is_bare_specifier(specifier) {
            return None;
        }
        // Lexically normalise the candidate so `pages/../components/x`
        // matches the same key the test wrote with `components/x`.
        let candidate = normalize_path_lexical(&importer_dir.join(specifier));

        if self.files.contains_key(&candidate) {
            return Some(candidate);
        }

        for ext in &self.probe_exts {
            let mut probe = candidate.clone();
            let new_name = format!(
                "{}{}",
                probe.file_name().and_then(|s| s.to_str()).unwrap_or(""),
                ext
            );
            probe.set_file_name(new_name);
            if self.files.contains_key(&probe) {
                return Some(probe);
            }
        }

        // `index.<ext>` probe (treat any path with a matching `index.*`
        // child as a directory entry).
        for ext in &self.probe_exts {
            let probe = candidate.join(format!("index{ext}"));
            if self.files.contains_key(&probe) {
                return Some(probe);
            }
        }

        None
    }

    fn read(&self, path: &Path) -> std::result::Result<String, String> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| format!("not found in InMemoryResolver: {}", path.display()))
    }
}

/// Walk every page, resolve imports recursively, and collect every
/// `"use client"` component reachable from any page.
///
/// `pages` should be the path representation the caller wants reflected
/// back in [`Island::source_path`]; whatever the resolver returns from
/// [`Resolver::resolve`] is what subsequently appears as the source path
/// of any island found in or beyond that file.
///
/// The returned vector is sorted by `(source_path, component_name)` and
/// deduped — duplicate entries (same path + name reachable through
/// multiple chains) collapse to one.
pub fn scan_islands<R: Resolver>(pages: &[PathBuf], resolver: &R) -> ScanResult<IslandsSet> {
    // BTreeMap keyed by (path, name) gives us natural sort order on output
    // and dedup for free.
    let mut found: BTreeMap<(PathBuf, String), Island> = BTreeMap::new();
    // Visited file set so cyclic imports terminate.
    let mut visited: HashSet<PathBuf> = HashSet::new();
    // DFS stack.
    let mut stack: Vec<PathBuf> = pages.to_vec();

    while let Some(current) = stack.pop() {
        if !visited.insert(current.clone()) {
            continue;
        }

        let source = resolver.read(&current).map_err(|message| ScanError::Resolver {
            path: current.clone(),
            message,
        })?;

        let module = parse_module(&current, &source)?;

        if has_use_client_directive(&module) {
            for name in exported_binding_names(&module) {
                let key = (current.clone(), name.clone());
                found
                    .entry(key)
                    .or_insert_with(|| Island::new(name, current.clone()));
            }
        }

        // Walk imports → push resolved paths onto the stack.
        //
        // Bare specifiers are handed to the resolver too. Today's
        // [`FsResolver`] uses them to walk `node_modules/` for
        // pnpm-workspace consumer packages whose source `.tsx` files
        // may carry `"use client"` islands; resolvers that don't care
        // (e.g. [`InMemoryResolver`], or [`FsResolver`] with the
        // workspace probe disabled) return `None` and the specifier is
        // silently skipped, matching the pre-#122 behaviour for
        // genuinely runtime-only specifiers like `preact/hooks`.
        let importer_dir = current
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        for specifier in collect_import_specifiers(&module) {
            if let Some(resolved) = resolver.resolve(&importer_dir, &specifier) {
                if !visited.contains(&resolved) {
                    stack.push(resolved);
                }
            }
        }
    }

    Ok(found.into_values().collect())
}

/// Parse a single TS/TSX source string into an SWC [`Module`].
fn parse_module(path: &Path, source: &str) -> ScanResult<Module> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(
        FileName::Real(path.to_path_buf()).into(),
        source.to_string(),
    );
    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax {
            tsx: true,
            decorators: false,
            dts: false,
            no_early_errors: false,
            disallow_ambiguous_jsx_like: false,
        }),
        EsVersion::Es2022,
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    parser.parse_module().map_err(|e| ScanError::Parse {
        path: path.to_path_buf(),
        message: format!("{e:?}"),
    })
}

/// Return true iff the module's leading directive prologue contains a
/// `"use client"` string-literal expression statement.
///
/// The prologue is scanned greedily: as soon as a non-string-literal
/// expression statement appears, we stop. Other prologue directives (e.g.
/// `"use strict"`) are tolerated.
fn has_use_client_directive(module: &Module) -> bool {
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Expr(expr_stmt)) = item else {
            // First non-stmt item ends the prologue.
            return false;
        };
        let Expr::Lit(Lit::Str(s)) = &*expr_stmt.expr else {
            // First non-string-literal expression statement ends the
            // prologue.
            return false;
        };
        if s.value == *"use client" {
            return true;
        }
        // Some other directive (e.g. "use strict") — keep looking through
        // the prologue.
    }
    false
}

/// Convert a [`Wtf8Atom`] (the SWC AST string type) to a plain [`String`].
///
/// Module specifiers, identifier names, and named-export labels are all
/// real source-text strings that round-trip cleanly through UTF-8; the
/// "lossy" path is only taken for the WTF-8 corner case (unpaired
/// surrogates in JS string literals), which we never see in practice.
fn atom_to_string(value: &Wtf8Atom) -> String {
    value.to_atom_lossy().to_string()
}

/// Collect import-style specifiers (anything that pulls another module
/// into the graph): plain `import`, side-effect `import "./x"`, named
/// re-exports `export { x } from "./y"`, and `export * from "./z"`.
fn collect_import_specifiers(module: &Module) -> Vec<String> {
    let mut out = Vec::new();
    for item in &module.body {
        let ModuleItem::ModuleDecl(decl) = item else {
            continue;
        };
        match decl {
            ModuleDecl::Import(import_decl) => {
                out.push(atom_to_string(&import_decl.src.value));
            }
            ModuleDecl::ExportNamed(named) => {
                if let Some(src) = &named.src {
                    out.push(atom_to_string(&src.value));
                }
            }
            ModuleDecl::ExportAll(all) => {
                out.push(atom_to_string(&all.src.value));
            }
            _ => {}
        }
    }
    out
}

/// Collect the exported binding names produced by this module.
///
/// See the module-level "Component identity" docs for the full mapping.
fn exported_binding_names(module: &Module) -> Vec<String> {
    let mut out = Vec::new();
    for item in &module.body {
        let ModuleItem::ModuleDecl(decl) = item else {
            continue;
        };
        match decl {
            ModuleDecl::ExportDecl(ed) => match &ed.decl {
                Decl::Class(c) => out.push(c.ident.sym.to_string()),
                Decl::Fn(f) => out.push(f.ident.sym.to_string()),
                Decl::Var(v) => {
                    for d in &v.decls {
                        if let Pat::Ident(bi) = &d.name {
                            out.push(bi.id.sym.to_string());
                        }
                    }
                }
                _ => {}
            },
            ModuleDecl::ExportDefaultDecl(_) | ModuleDecl::ExportDefaultExpr(_) => {
                out.push("default".to_string());
            }
            ModuleDecl::ExportNamed(named) => {
                for spec in &named.specifiers {
                    match spec {
                        ExportSpecifier::Named(n) => {
                            let pick = n.exported.as_ref().unwrap_or(&n.orig);
                            match pick {
                                ModuleExportName::Ident(id) => out.push(id.sym.to_string()),
                                ModuleExportName::Str(s) => out.push(atom_to_string(&s.value)),
                            }
                        }
                        ExportSpecifier::Default(d) => {
                            out.push(d.exported.sym.to_string());
                        }
                        ExportSpecifier::Namespace(n) => match &n.name {
                            ModuleExportName::Ident(id) => out.push(id.sym.to_string()),
                            ModuleExportName::Str(s) => out.push(atom_to_string(&s.value)),
                        },
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from("/proj")
    }

    fn paths(islands: &IslandsSet) -> Vec<(String, String)> {
        islands
            .iter()
            .map(|i| {
                (
                    i.source_path.to_string_lossy().to_string(),
                    i.component_name.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn detects_use_client_with_double_quotes() {
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { Counter } from "../components/counter";
                export default function Home() { return <Counter/>; }
                "#,
            )
            .with_file(
                root().join("components/counter.tsx"),
                r#""use client";
                import { useState } from "preact/hooks";
                export function Counter() { return null; }
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(
            paths(&islands),
            vec![(
                "/proj/components/counter.tsx".to_string(),
                "Counter".to_string()
            )]
        );
    }

    #[test]
    fn detects_use_client_with_single_quotes() {
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { Counter } from "../components/counter";
                export default function Home() {}
                "#,
            )
            .with_file(
                root().join("components/counter.tsx"),
                r#"'use client';
                export function Counter() {}
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].component_name, "Counter");
    }

    #[test]
    fn page_with_no_use_client_imports_yields_empty_set() {
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { ServerOnly } from "../components/server-only";
                export default function Home() { return <ServerOnly/>; }
                "#,
            )
            .with_file(
                root().join("components/server-only.tsx"),
                r#"export function ServerOnly() { return <div/>; }
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(islands.is_empty(), "got {:?}", islands);
    }

    #[test]
    fn transitive_imports_reach_use_client() {
        // page → layout → island. The middle hop is server-only, but the
        // scanner must traverse it.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { Layout } from "../layouts/main";
                export default function Home() { return <Layout/>; }
                "#,
            )
            .with_file(
                root().join("layouts/main.tsx"),
                r#"import { Counter } from "../components/counter";
                export function Layout() { return <Counter/>; }
                "#,
            )
            .with_file(
                root().join("components/counter.tsx"),
                r#""use client";
                export function Counter() {}
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(
            paths(&islands),
            vec![(
                "/proj/components/counter.tsx".to_string(),
                "Counter".to_string()
            )]
        );
    }

    #[test]
    fn use_client_module_with_multiple_exports_emits_one_island_per_name() {
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { Foo, Bar, RenamedBaz } from "../components/cluster";
                export default function Home() {}
                "#,
            )
            .with_file(
                root().join("components/cluster.tsx"),
                r#""use client";
                export function Foo() {}
                export class Bar {}
                const Baz = () => null;
                export { Baz as RenamedBaz };
                export const Qux = 1;
                export default function Defaulted() {}
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        let names: Vec<String> = islands.iter().map(|i| i.component_name.clone()).collect();
        // Sorted lexicographically by component name for the same source
        // path: Bar, Foo, Qux, RenamedBaz, default.
        assert_eq!(
            names,
            vec![
                "Bar".to_string(),
                "Foo".to_string(),
                "Qux".to_string(),
                "RenamedBaz".to_string(),
                "default".to_string(),
            ]
        );
        // All point at the same source path.
        for island in &islands {
            assert_eq!(island.source_path, root().join("components/cluster.tsx"));
        }
    }

    #[test]
    fn use_client_after_use_strict_in_prologue_is_accepted() {
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { Counter } from "../components/counter";
                export default function Home() {}
                "#,
            )
            .with_file(
                root().join("components/counter.tsx"),
                r#""use strict";
                "use client";
                export function Counter() {}
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1);
    }

    #[test]
    fn directive_after_a_real_statement_is_not_a_directive() {
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { Counter } from "../components/counter";
                export default function Home() {}
                "#,
            )
            .with_file(
                root().join("components/counter.tsx"),
                r#"const _ = 1;
                "use client";
                export function Counter() {}
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(islands.is_empty(), "got {:?}", islands);
    }

    #[test]
    fn comments_above_use_client_are_ignored() {
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { Counter } from "../components/counter";
                export default function Home() {}
                "#,
            )
            .with_file(
                root().join("components/counter.tsx"),
                r#"// SPDX-License-Identifier: MIT
                /* This file runs in the browser. */
                "use client";
                export function Counter() {}
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].component_name, "Counter");
    }

    #[test]
    fn output_is_deterministically_sorted() {
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { A } from "../components/zeta";
                import { B } from "../components/alpha";
                export default function Home() {}
                "#,
            )
            .with_file(
                root().join("components/zeta.tsx"),
                r#""use client";
                export function A() {}
                "#,
            )
            .with_file(
                root().join("components/alpha.tsx"),
                r#""use client";
                export function B() {}
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(
            paths(&islands),
            vec![
                (
                    "/proj/components/alpha.tsx".to_string(),
                    "B".to_string()
                ),
                ("/proj/components/zeta.tsx".to_string(), "A".to_string()),
            ]
        );
    }

    #[test]
    fn cyclic_imports_terminate() {
        // a imports b, b imports a — neither uses "use client".
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { A } from "../components/a";
                export default function Home() {}
                "#,
            )
            .with_file(
                root().join("components/a.tsx"),
                r#"import { B } from "./b";
                export const A = () => B;
                "#,
            )
            .with_file(
                root().join("components/b.tsx"),
                r#"import { A } from "./a";
                export const B = () => A;
                "#,
            );

        // Should terminate (and yield no islands).
        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(islands.is_empty());
    }

    #[test]
    fn dedups_islands_reachable_through_multiple_chains() {
        // page imports both A and B; both pull in the same client island.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { A } from "../components/a";
                import { B } from "../components/b";
                export default function Home() {}
                "#,
            )
            .with_file(
                root().join("components/a.tsx"),
                r#"import { Counter } from "./counter";
                export function A() {}
                "#,
            )
            .with_file(
                root().join("components/b.tsx"),
                r#"import { Counter } from "./counter";
                export function B() {}
                "#,
            )
            .with_file(
                root().join("components/counter.tsx"),
                r#""use client";
                export function Counter() {}
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1, "got {:?}", islands);
        assert_eq!(islands[0].component_name, "Counter");
    }

    #[test]
    fn bare_specifiers_are_skipped_silently() {
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { useState } from "preact/hooks";
                import { Counter } from "../components/counter";
                export default function Home() {}
                "#,
            )
            .with_file(
                root().join("components/counter.tsx"),
                r#""use client";
                export function Counter() {}
                "#,
            );

        // Must not fail trying to resolve "preact/hooks".
        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1);
    }

    #[test]
    fn re_export_from_another_module_is_followed_and_attributed() {
        // A barrel file re-exports a "use client" component. The barrel
        // itself is not a "use client" file — but the scanner must follow
        // the re-export to discover the underlying island.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { Counter } from "../components/index";
                export default function Home() {}
                "#,
            )
            .with_file(
                root().join("components/index.tsx"),
                r#"export { Counter } from "./counter";
                "#,
            )
            .with_file(
                root().join("components/counter.tsx"),
                r#""use client";
                export function Counter() {}
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(
            paths(&islands),
            vec![(
                "/proj/components/counter.tsx".to_string(),
                "Counter".to_string()
            )]
        );
    }

    #[test]
    fn fs_resolver_works_against_a_real_tempdir() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let pages = dir.path().join("pages");
        let components = dir.path().join("components");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&components).unwrap();

        fs::write(
            pages.join("home.tsx"),
            r#"import { Counter } from "../components/counter";
            export default function Home() {}
            "#,
        )
        .unwrap();
        fs::write(
            components.join("counter.tsx"),
            r#""use client";
            export function Counter() {}
            "#,
        )
        .unwrap();

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].component_name, "Counter");
        // FsResolver canonicalises (resolves symlinks like macOS's
        // `/var` → `/private/var`), so compare against the canonical form
        // of the expected path rather than the join-produced one.
        let expected = components
            .join("counter.tsx")
            .canonicalize()
            .expect("canonicalize");
        assert_eq!(islands[0].source_path, expected);
    }

    #[test]
    fn parse_error_surfaces_with_path_context() {
        let resolver = InMemoryResolver::new().with_file(
            root().join("pages/broken.tsx"),
            // Stray `}`: real syntax error.
            r#"export default function() }"#,
        );
        let err = scan_islands(&[root().join("pages/broken.tsx")], &resolver).unwrap_err();
        match err {
            ScanError::Parse { path, .. } => {
                assert_eq!(path, root().join("pages/broken.tsx"));
            }
            other => unreachable!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn is_bare_specifier_recognises_relative_and_absolute_paths() {
        assert!(is_bare_specifier("preact"));
        assert!(is_bare_specifier("preact/hooks"));
        assert!(is_bare_specifier("zfb"));
        assert!(!is_bare_specifier("./layout"));
        assert!(!is_bare_specifier("../components/counter"));
        assert!(!is_bare_specifier("/abs/path"));
    }

    /// pnpm-workspace consumer shape (#122): a page imports a workspace
    /// package by its scoped name. The package's source `.tsx` carries
    /// `"use client"`. The fixture wires up the same shape pnpm
    /// produces under `node_modules/@scope/pkg/` (a real directory in
    /// the test rather than a symlink to keep the fixture portable).
    /// FsResolver must walk node_modules + read package.json.source to
    /// reach the island.
    #[test]
    fn fs_resolver_resolves_pnpm_workspace_scoped_package() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let consumer = dir.path().join("consumer");
        let pages = consumer.join("pages");
        let pkg = consumer
            .join("node_modules")
            .join("@takazudo")
            .join("zfb-blog-islands");
        let pkg_src = pkg.join("src");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&pkg_src).unwrap();

        fs::write(
            pages.join("home.tsx"),
            r#"import { Counter } from "@takazudo/zfb-blog-islands";
            export default function Home() {}
            "#,
        )
        .unwrap();
        fs::write(
            pkg.join("package.json"),
            r#"{ "name": "@takazudo/zfb-blog-islands", "source": "src/index.tsx" }"#,
        )
        .unwrap();
        fs::write(
            pkg_src.join("index.tsx"),
            r#""use client";
            export function Counter() {}
            "#,
        )
        .unwrap();

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1, "got {islands:?}");
        assert_eq!(islands[0].component_name, "Counter");
        let expected = pkg_src.join("index.tsx").canonicalize().expect("canon");
        assert_eq!(islands[0].source_path, expected);
    }

    /// Same shape as above but with a subpath specifier
    /// (`@scope/pkg/components/foo`). The `node_modules/@scope/pkg/`
    /// directory hosts a `components/foo.tsx` that the resolver
    /// reaches without consulting `package.json` exports.
    #[test]
    fn fs_resolver_resolves_pnpm_workspace_subpath_specifier() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let consumer = dir.path().join("consumer");
        let pages = consumer.join("pages");
        let components = consumer
            .join("node_modules")
            .join("@takazudo")
            .join("zfb-blog-islands")
            .join("components");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&components).unwrap();

        fs::write(
            pages.join("home.tsx"),
            r#"import { Counter } from "@takazudo/zfb-blog-islands/components/counter";
            export default function Home() {}
            "#,
        )
        .unwrap();
        fs::write(
            components.join("counter.tsx"),
            r#""use client";
            export function Counter() {}
            "#,
        )
        .unwrap();

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1, "got {islands:?}");
        assert_eq!(islands[0].component_name, "Counter");
    }

    /// Walks ancestors when `node_modules/<pkg>` lives several
    /// directories above the importer (mirrors a deep page importer +
    /// hoisted workspace package).
    #[test]
    fn fs_resolver_walks_ancestors_for_node_modules() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let pages = dir.path().join("apps/site/pages/blog");
        let pkg = dir
            .path()
            .join("node_modules")
            .join("@takazudo")
            .join("zfb-blog-islands");
        let pkg_src = pkg.join("src");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&pkg_src).unwrap();

        fs::write(
            pages.join("post.tsx"),
            r#"import { Counter } from "@takazudo/zfb-blog-islands";
            export default function Post() {}
            "#,
        )
        .unwrap();
        fs::write(
            pkg.join("package.json"),
            r#"{ "name": "@takazudo/zfb-blog-islands", "source": "src/index.tsx" }"#,
        )
        .unwrap();
        fs::write(
            pkg_src.join("index.tsx"),
            r#""use client";
            export function Counter() {}
            "#,
        )
        .unwrap();

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("post.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1);
    }

    /// Falls back to `src/index.<ext>` when no package.json field
    /// points at a real source file — common for un-built workspace
    /// packages that don't bother with a manifest entry.
    #[test]
    fn fs_resolver_falls_back_to_src_index_without_package_json_hint() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let pages = dir.path().join("pages");
        let pkg = dir.path().join("node_modules").join("ws-pkg");
        let pkg_src = pkg.join("src");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&pkg_src).unwrap();

        fs::write(
            pages.join("home.tsx"),
            r#"import { Counter } from "ws-pkg";
            export default function Home() {}
            "#,
        )
        .unwrap();
        fs::write(pkg.join("package.json"), r#"{ "name": "ws-pkg" }"#).unwrap();
        fs::write(
            pkg_src.join("index.tsx"),
            r#""use client";
            export function Counter() {}
            "#,
        )
        .unwrap();

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1);
    }

    /// Bare specifier pointing at a package that does NOT exist on
    /// disk (the framework-supplied case: `preact/hooks`, `zfb`, etc.)
    /// must return `None` from the resolver — `scan_islands` then
    /// silently skips it without erroring.
    #[test]
    fn fs_resolver_returns_none_for_unknown_bare_specifier() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let pages = dir.path().join("pages");
        fs::create_dir_all(&pages).unwrap();
        fs::write(
            pages.join("home.tsx"),
            r#"import { useState } from "preact/hooks";
            export default function Home() {}
            "#,
        )
        .unwrap();

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        assert!(islands.is_empty(), "got {islands:?}");
    }

    /// `FsResolver::without_workspace_probe()` opts out of the
    /// pnpm-workspace probe — bare specifiers always return `None`,
    /// matching the pre-#122 behaviour. Useful for the (unlikely) case
    /// where a future caller wants to suppress the probe entirely.
    #[test]
    fn fs_resolver_without_workspace_probe_skips_node_modules_walk() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let pages = dir.path().join("pages");
        let pkg_src = dir
            .path()
            .join("node_modules")
            .join("ws-pkg")
            .join("src");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&pkg_src).unwrap();

        fs::write(
            pages.join("home.tsx"),
            r#"import { Counter } from "ws-pkg";
            export default function Home() {}
            "#,
        )
        .unwrap();
        fs::write(
            pkg_src.join("index.tsx"),
            r#""use client";
            export function Counter() {}
            "#,
        )
        .unwrap();

        let resolver = FsResolver::without_workspace_probe();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        assert!(islands.is_empty(), "got {islands:?}");
    }

    #[test]
    fn split_bare_specifier_handles_scoped_and_unscoped() {
        let scoped = FsResolver::split_bare_specifier("@takazudo/zfb-blog-islands");
        assert_eq!(scoped, ("@takazudo/zfb-blog-islands".to_string(), String::new()));

        let scoped_sub =
            FsResolver::split_bare_specifier("@takazudo/zfb-blog-islands/components/foo");
        assert_eq!(
            scoped_sub,
            (
                "@takazudo/zfb-blog-islands".to_string(),
                "components/foo".to_string()
            )
        );

        let plain = FsResolver::split_bare_specifier("preact");
        assert_eq!(plain, ("preact".to_string(), String::new()));

        let plain_sub = FsResolver::split_bare_specifier("preact/hooks");
        assert_eq!(plain_sub, ("preact".to_string(), "hooks".to_string()));
    }
}
