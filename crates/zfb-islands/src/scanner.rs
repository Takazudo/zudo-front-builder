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
//! - `export const Foo = …` (and `let` / `var`) → `"Foo"` when the
//!   initialiser is component-plausible (function/arrow/call/tagged
//!   template/identifier/…); a clearly-non-component literal value is
//!   dropped (issue #998).
//! - `export default …` → the literal string `"default"` (a
//!   clearly-non-component literal default is dropped, issue #998)
//! - `export { A, B as C }` → `"A"`, `"C"` (a local re-export resolving
//!   to a clearly-non-component binding is dropped, issue #998)
//! - `export d from "./mod"` (rare) → `"d"`
//!
//! `export * as ns from "./mod"` is NOT registered: a module-namespace
//! object is never a mountable component (issue #998).
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
use std::sync::{Arc, Mutex};

use swc_core::atoms::Wtf8Atom;
use swc_core::common::sync::Lrc;
use swc_core::common::{FileName, SourceMap};
use swc_core::ecma::ast::{
    BlockStmt, Callee, Decl, DefaultDecl, EsVersion, ExportSpecifier, Expr, ImportSpecifier, Lit,
    Module, ModuleDecl, ModuleExportName, ModuleItem, Pat, Stmt, VarDeclarator,
};
use swc_core::ecma::parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};
use thiserror::Error;
use zfb_plugin_resolver::{read_tsconfig_paths, TsConfigPaths, TsPathAlias};
use zfb_types::normalize_path_lexical;

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

/// Side-channel metadata gathered during a scan that is not itself an
/// [`Island`].
///
/// Returned alongside the [`IslandsSet`] by [`scan_islands_with_meta`].
/// Kept separate from [`IslandsSet`] so the long-standing
/// `scan_islands -> Vec<Island>` contract (and every caller / test that
/// depends on it) is unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanMeta {
    /// True when some file reachable from a page opts the project into
    /// `<ClientRouter />` (issue #289). Detected from import / re-export
    /// declarations — see [`collect_client_router_facts`] and
    /// [`resolve_client_router_usage`]:
    ///
    /// - a named `ClientRouter` import from the `@takazudo/zfb-runtime`
    ///   barrel (covers both the `<ClientRouter />` JSX tag and the
    ///   function-call `{ClientRouter() as ...}` form), or
    /// - a direct import / re-export of the
    ///   `@takazudo/zfb-runtime/client-router` subpath, or
    /// - a named `ClientRouter` import / re-export from a *local* barrel
    ///   that transitively re-exports `@takazudo/zfb-runtime` through one
    ///   or more `export *` hops (issue #307).
    ///
    /// When set, the islands bundler injects a side-effect
    /// `import "@takazudo/zfb-runtime/client-router";` into the synthetic
    /// entry so the View Transitions runtime (`init()`) ships in
    /// `assets/islands.js` with no `"use client"` boilerplate on the
    /// consumer's side.
    ///
    /// Detection keys off the *import* of `ClientRouter`, not its actual
    /// JSX/call use: distinguishing an unused import would require
    /// whole-module identifier-usage analysis the scanner does not do.
    /// An unused `ClientRouter` import is a rare authoring mistake whose
    /// only cost is a few KB; treating the import as the signal keeps the
    /// scanner cheap.
    pub uses_client_router: bool,

    /// Count of scanned modules that *look like* they intended to be a
    /// `"use client"` island but did not register one — i.e. the source
    /// text contains the literal substring `use client`, yet the module
    /// contributed zero islands (issue #822).
    ///
    /// This is a cheap "near-miss" heuristic, not a precise diagnostic.
    /// It flags the two authoring mistakes the empty-islands warning is
    /// actually written for:
    ///
    /// - a misplaced directive — `"use client"` appears, but not as the
    ///   first statement of the module prologue, so
    ///   [`has_use_client_directive`] rejects it; and
    /// - a misspelled / mis-cased directive (e.g. `"use  client"`,
    ///   `'use client '`) that still contains the `use client` substring.
    ///
    /// It deliberately does *not* try to detect islands that are simply
    /// unreachable from `pages/` (the scanner never reads those files, so
    /// it has nothing to inspect) — that case is left to documentation.
    ///
    /// The caller uses this to decide *how* to report an empty island
    /// set: when `> 0` there is a plausible authoring mistake, so the
    /// loud warning + verify-hint is justified; when `0` the project is
    /// island-free on purpose, so a quiet info note suffices and the
    /// verify-hint would be pure noise.
    pub near_miss_candidates: usize,
}

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
///
/// The resolver also honours TypeScript path aliases declared in the
/// project's `tsconfig.json` (`compilerOptions.paths` +
/// `compilerOptions.baseUrl`). The nearest `tsconfig.json` is
/// auto-discovered by walking up from each importer's directory the
/// first time it's seen; the parsed alias table is cached on the
/// resolver so subsequent imports from the same project re-use it
/// without re-reading disk. Wildcard patterns of the form
/// `"@/*": ["src/*"]` are supported (the common case downstream
/// projects use); literal patterns also work. Conditional priorities
/// (`browser`, `node`, …) and the full Node.js `paths` spec are out of
/// scope for now — see issue #139 for the upstream tracker.
#[derive(Debug, Clone)]
pub struct FsResolver {
    /// File extensions probed for relative imports without an extension.
    pub probe_exts: Vec<String>,
    /// Whether bare specifiers are routed through the pnpm-workspace
    /// `node_modules/<specifier>` probe. Defaults to `true`; tests that
    /// want the legacy "skip every bare specifier" shape can flip it
    /// off via [`FsResolver::without_workspace_probe`].
    pub workspace_probe_enabled: bool,
    /// Cache of `tsconfig_dir → parsed paths` for the nearest
    /// `tsconfig.json` discovered above each importer. Populated on
    /// first lookup; subsequent imports from the same project hit the
    /// in-memory entry instead of re-reading disk. `Arc<Mutex<…>>` so
    /// `Clone` is cheap and the cache is shared across clones (which
    /// the scanner makes when running in parallel test harnesses).
    tsconfig_cache: Arc<Mutex<HashMap<PathBuf, Option<TsConfigPaths>>>>,
}

// TsConfigPaths and TsPathAlias are imported from zfb_plugin_resolver above.

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
            tsconfig_cache: Arc::new(Mutex::new(HashMap::new())),
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

    /// True when `pkg_dir` (a `node_modules/<pkg>` entry) looks like a
    /// pnpm-workspace package — i.e. the entry is a symlink whose
    /// target lives OUTSIDE any `node_modules/` directory.
    ///
    /// pnpm's layout: workspace members are linked from
    /// `node_modules/<pkg>` directly to the source directory in the
    /// workspace tree (e.g. `packages/<pkg>/`). A regular installed
    /// dependency is linked from `node_modules/<pkg>` into the pnpm
    /// content-addressable store under `node_modules/.pnpm/<pkg>@<ver>/…`,
    /// so its canonical path still contains a `node_modules/` segment.
    ///
    /// This heuristic is intentionally conservative: it only descends
    /// into bare specifiers when there's clear evidence of a workspace
    /// package, so framework imports like `preact/hooks` (real
    /// `node_modules/preact/dist/preact.js` after canonicalisation)
    /// stop at the resolver and never feed the scanner a transitive
    /// node_modules walk on every build. See codex-review on PR #125.
    fn is_workspace_package(pkg_dir: &Path) -> bool {
        // Symlink check: pnpm always places workspace members behind a
        // symlink at `node_modules/<pkg>`. A real `node_modules/<pkg>/`
        // directory (npm/yarn classic, or a flat layout) is NOT a
        // workspace package.
        let meta = match std::fs::symlink_metadata(pkg_dir) {
            Ok(m) => m,
            Err(_) => return false,
        };
        if !meta.file_type().is_symlink() {
            return false;
        }
        // Canonicalise the symlink target. If the canonical path
        // contains any `node_modules/` segment, it's the regular
        // dependency layout (`node_modules/.pnpm/<pkg>@<ver>/node_modules/<pkg>`)
        // — NOT a workspace package.
        let canon = match pkg_dir.canonicalize() {
            Ok(p) => p,
            Err(_) => return false,
        };
        for comp in canon.components() {
            if let Component::Normal(name) = comp {
                if name == std::ffi::OsStr::new("node_modules") {
                    return false;
                }
            }
        }
        true
    }

    /// True when `dir` has any `node_modules` path component — i.e. the
    /// importer is itself inside an installed package's tree.
    ///
    /// Used to scope the regular-npm-package descent (issue #999): a bare
    /// import made from inside a package's dist must NOT crawl into OTHER
    /// packages (no framework dependency-graph walk), so only project-source
    /// imports (importer outside `node_modules`) are allowed to enter a
    /// regular npm package. Mirrors the `node_modules`-segment heuristic
    /// [`Self::is_workspace_package`] already uses.
    fn is_inside_node_modules(dir: &Path) -> bool {
        dir.components().any(|comp| {
            matches!(comp, Component::Normal(name) if name == std::ffi::OsStr::new("node_modules"))
        })
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
    /// Subpath imports (`@scope/pkg/components/foo`) probe
    /// `<pkg_dir>/<subpath>` exactly first (with extensions appended,
    /// and finally as a directory containing `index.<ext>` — same shape
    /// the relative-path branch uses, so a workspace-package directory
    /// import behaves the same as a project-internal one). When that
    /// literal-subpath probe misses, fall back to `package.json`
    /// `exports["./<subpath>"]` for packages that redirect subpath
    /// imports into a different on-disk shape (e.g. `src/<sub>/index.ts`)
    /// — the common pattern for un-built TypeScript workspace packages.
    /// Conditional shapes (`{ "source": "...", "default": "..." }`) are
    /// flattened in priority `source` → `default` → `import`; pattern
    /// entries (`*` wildcards) are intentionally not handled here.
    ///
    /// Bare-package imports (no subpath) read `package.json` and try
    /// `source` (the convention pnpm-workspace TypeScript packages use
    /// for un-built sources), then `module`, then `main`. If
    /// `package.json` is missing or doesn't point at a scannable file,
    /// fall back to probing `src/index.<ext>` and `index.<ext>` inside
    /// the package root — both common shapes for un-built workspace
    /// packages.
    ///
    /// All resolved paths are clamped to live inside `pkg_dir` (after
    /// canonicalisation): a malicious `package.json` with
    /// `"source": "../../etc/passwd"` cannot punch out of the package
    /// directory.
    fn probe_package_entry(&self, pkg_dir: &Path, subpath: &str) -> Option<PathBuf> {
        // Canonicalise once for clamp comparisons; if the package dir
        // can't be canonicalised (e.g. doesn't exist), fall back to the
        // raw path — the probe's `is_file()` checks will then fail
        // naturally on anything that doesn't actually live on disk.
        let pkg_root = pkg_dir
            .canonicalize()
            .unwrap_or_else(|_| pkg_dir.to_path_buf());

        let probe_dir_or_file = |candidate: PathBuf| -> Option<PathBuf> {
            self.probe_path_with_index(&candidate)
                .filter(|found| Self::is_inside(&pkg_root, found))
        };

        if !subpath.is_empty() {
            // 1) Literal-subpath probe — preserved as the fallback for
            //    the legacy package shape where on-disk layout matches
            //    the import shape (`<pkg_dir>/<subpath>`).
            let direct = pkg_dir.join(subpath);
            if let Some(found) = probe_dir_or_file(direct) {
                return Some(found);
            }

            // 2) `package.json` `exports["./<subpath>"]` — workspace
            //    TypeScript packages overwhelmingly redirect subpath
            //    imports into `src/<sub>/index.ts` via this map.
            let pkg_json_path = pkg_dir.join("package.json");
            if let Ok(text) = std::fs::read_to_string(&pkg_json_path) {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                    if let Some(target) = resolve_exports_subpath(&value, subpath) {
                        let candidate = pkg_dir.join(&target);
                        if let Some(found) = probe_dir_or_file(candidate) {
                            return Some(found);
                        }
                    }
                }
            }
            return None;
        }

        // 1) package.json hints — read once, try in priority order.
        let pkg_json_path = pkg_dir.join("package.json");
        if let Ok(text) = std::fs::read_to_string(&pkg_json_path) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                // `exports["."]` first: modern packages (e.g. a published
                // `"type": "module"` dist with no `main`/`module`) expose
                // their entry only through the `exports` map, and Node
                // ignores `main` entirely once `exports` is present. This
                // is the shape `@takazudo/zudo-doc` ships (issue #999).
                if let Some(target) = resolve_exports_dot(&value) {
                    let candidate = pkg_dir.join(&target);
                    if let Some(found) = probe_dir_or_file(candidate) {
                        return Some(found);
                    }
                }
                for field in ["source", "module", "main"] {
                    if let Some(rel) = value.get(field).and_then(|v| v.as_str()) {
                        let candidate = pkg_dir.join(rel);
                        if let Some(found) = probe_dir_or_file(candidate) {
                            return Some(found);
                        }
                    }
                }
            }
        }

        // 2) Conventional un-built workspace shapes.
        for prefix in ["src/index", "index"] {
            let candidate = pkg_dir.join(prefix);
            if let Some(found) = probe_dir_or_file(candidate) {
                return Some(found);
            }
        }
        None
    }

    /// Probe a candidate path the same way the relative-import branch
    /// of [`Resolver::resolve`] does: try the path as a file, then with
    /// each extension appended, and finally as a directory containing
    /// `index.<ext>`. Returns the first regular file that exists.
    fn probe_path_with_index(&self, candidate: &Path) -> Option<PathBuf> {
        if let Some(found) = Self::probe_with_extensions(candidate, &self.probe_exts) {
            return Some(found);
        }
        if candidate.is_dir() {
            for ext in &self.probe_exts {
                let probe = candidate.join(format!("index{ext}"));
                if probe.is_file() {
                    return Some(probe);
                }
            }
        }
        None
    }

    /// Try `path` as-is, then with TS-family swap candidates for any
    /// `.js`/`.jsx`/`.mjs`/`.cjs` specifier (the
    /// `moduleResolution: "Bundler"` / NodeNext convention where source
    /// is authored as `.ts`/`.tsx` but imports use the post-emit
    /// `.js`/`.jsx`/… extension), then with each `exts` entry appended.
    /// Returns the first hit that is a regular file.
    ///
    /// The TS-swap pass exists because TypeScript with
    /// `moduleResolution: "Bundler"` (or `"NodeNext"`) idiomatically has
    /// source files write `import "./foo.js"` even though the file on
    /// disk is `foo.tsx` — esbuild and tsc both transparently swap the
    /// extension when resolving. The islands scanner needs to do the
    /// same so it can follow imports through workspace TypeScript
    /// packages that ship un-built source. See issue #143 (the symptom
    /// was a downstream consumer with `*.tsx` workspace packages whose
    /// `import "./foo.js"` chain stopped at the first such specifier,
    /// missing every transitively-imported `"use client"` island).
    fn probe_with_extensions(path: &Path, exts: &[String]) -> Option<PathBuf> {
        // TS-family swap runs first so a stale `.js` artifact next to the
        // live `.tsx` doesn't shadow the source the bundler uses.
        for swapped in ts_swap_candidates(path) {
            if swapped.is_file() {
                return Some(swapped);
            }
        }
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

    /// Whether `path`, after canonicalisation, lives at or below
    /// `root`. Used to clamp resolved package entries inside their
    /// `node_modules/<pkg>/` directory so a hostile `package.json`
    /// cannot redirect the scanner outside the package.
    fn is_inside(root: &Path, path: &Path) -> bool {
        let canon = match path.canonicalize() {
            Ok(p) => p,
            Err(_) => return false,
        };
        canon.starts_with(root)
    }

    /// Walk up from `start` looking for the nearest `tsconfig.json`.
    /// Returns the directory containing it, not the file path itself
    /// (callers join `tsconfig.json` themselves where needed). The
    /// shape mirrors [`Self::locate_node_modules_pkg`] — same
    /// "ancestor walk" pattern, no canonicalisation (callers handle
    /// that on the resolved file).
    fn locate_tsconfig_dir(start: &Path) -> Option<PathBuf> {
        let mut dir: Option<&Path> = Some(start);
        while let Some(d) = dir {
            if d.join("tsconfig.json").is_file() {
                return Some(d.to_path_buf());
            }
            dir = d.parent();
        }
        None
    }

    /// Try to resolve `specifier` against `tsconfig.json`
    /// `compilerOptions.paths`. Returns the first probe hit, or
    /// `None` when:
    ///
    /// - There is no `tsconfig.json` above `importer_dir`.
    /// - The tsconfig has no `compilerOptions.paths` entries.
    /// - No alias pattern matches the specifier.
    /// - A pattern matches but every substitution target's probe
    ///   (extension + index walk) misses.
    fn try_resolve_tsconfig_alias(&self, importer_dir: &Path, specifier: &str) -> Option<PathBuf> {
        let tsconfig_dir = Self::locate_tsconfig_dir(importer_dir)?;
        let entry = self.load_tsconfig_paths(&tsconfig_dir)?;
        // Collect every alias whose pattern matches the specifier,
        // then iterate in TypeScript's documented priority order:
        // most-specific (longest non-wildcard prefix) wins. With the
        // overlapping shape `{"@/*": [...], "@/components/*": [...]}`,
        // resolving `@/components/Button` must try `@/components/*`
        // first; only fall through to `@/*` when the more-specific
        // probe misses. Map order in the source `paths` object is
        // **not** stable across JSON parsers, so we cannot rely on
        // declaration order — we must rank by pattern shape.
        let mut matches: Vec<(&TsPathAlias, Option<String>, usize)> = entry
            .aliases
            .iter()
            .filter_map(|alias| {
                let captured = match_alias_pattern(&alias.pattern, specifier)?;
                Some((alias, captured, pattern_specificity(&alias.pattern)))
            })
            .collect();
        // Higher specificity score first. `sort_by_key` is stable,
        // so ties (e.g. two literal patterns of the same length)
        // keep declaration order and remain deterministic.
        matches.sort_by_key(|m| std::cmp::Reverse(m.2));
        for (alias, captured, _score) in matches {
            for target in &alias.targets {
                let substituted = substitute_alias_target(target, captured.as_deref());
                let candidate = entry.base_dir.join(&substituted);
                if let Some(found) = self.probe_path_with_index(&candidate) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// Look up — and lazily populate — the parsed `paths` table for the
    /// `tsconfig.json` at `tsconfig_dir`. Cached on the resolver so a
    /// project's tsconfig is read at most once per resolver instance,
    /// and shared across `Clone`s.
    ///
    /// Delegates to [`zfb_plugin_resolver::read_tsconfig_paths`], which adds
    /// JSONC comment stripping (fixing a latent bug where a tsconfig with `//`
    /// comments silently broke alias resolution) and file-based `extends`
    /// resolution (supporting any tsconfig filename, e.g. `tsconfig.base.json`).
    fn load_tsconfig_paths(&self, tsconfig_dir: &Path) -> Option<TsConfigPaths> {
        let key = tsconfig_dir.to_path_buf();
        // Check cache first.
        {
            let cache = self.tsconfig_cache.lock().unwrap_or_else(|p| {
                tracing::warn!(
                    site = "FsResolver::tsconfig_cache",
                    "mutex poisoned, recovered"
                );
                p.into_inner()
            });
            if let Some(cached) = cache.get(&key) {
                return cached.clone();
            }
        }

        // Cache miss — parse from disk via the shared helper.
        let parsed = read_tsconfig_paths(&key);
        let mut cache = self.tsconfig_cache.lock().unwrap_or_else(|p| {
            tracing::warn!(
                site = "FsResolver::tsconfig_cache",
                "mutex poisoned, recovered"
            );
            p.into_inner()
        });
        cache.insert(key, parsed.clone());
        parsed
    }
}

/// Score `pattern`'s specificity for the
/// most-specific-pattern-wins rule TypeScript applies when more than
/// one `compilerOptions.paths` key matches the specifier.
///
/// The score is the length of the longest non-wildcard run in the
/// pattern. Literal patterns (no `*`) score their full length, so
/// they always beat any wildcard form of the same prefix; wildcards
/// score the longer of the prefix-before-`*` and suffix-after-`*`
/// runs. Two patterns with identical scores keep their declaration
/// order (the caller uses a stable sort).
///
/// This deliberately mirrors the TypeScript Compiler's own
/// `getBestPathPattern` shape: the longest fixed prefix is the
/// dominant tiebreaker; an exact match wins by sheer length.
fn pattern_specificity(pattern: &str) -> usize {
    if let Some(star_idx) = pattern.find('*') {
        let prefix_len = star_idx;
        let suffix_len = pattern.len().saturating_sub(star_idx + 1);
        prefix_len.max(suffix_len)
    } else {
        pattern.len()
    }
}

/// Match `specifier` against an alias `pattern` from
/// `compilerOptions.paths`.
///
/// Returns:
///
/// - `Some(None)` — pattern is a literal (no `*`) and matches the
///   specifier exactly.
/// - `Some(Some(rest))` — pattern is a wildcard (`prefix*` / `prefix*suffix`)
///   and matches; `rest` is the substring that fills the `*`.
/// - `None` — no match.
///
/// At most one `*` is honoured (the TypeScript spec). Patterns with
/// multiple `*`s are skipped (`None`) rather than treated as literals
/// — silently matching nothing is safer than half-supporting the
/// shape.
fn match_alias_pattern(pattern: &str, specifier: &str) -> Option<Option<String>> {
    let star_count = pattern.matches('*').count();
    match star_count {
        0 => {
            if pattern == specifier {
                Some(None)
            } else {
                None
            }
        }
        1 => {
            let star_idx = pattern.find('*')?;
            let prefix = &pattern[..star_idx];
            let suffix = &pattern[star_idx + 1..];
            if specifier.starts_with(prefix) && specifier.ends_with(suffix) {
                let captured =
                    &specifier[prefix.len()..specifier.len().saturating_sub(suffix.len())];
                Some(Some(captured.to_string()))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Substitute the captured wildcard fragment back into a
/// `compilerOptions.paths` target.
///
/// `target` mirrors the alias-pattern shape: zero `*` means the target
/// is a literal substitution (used as-is); one `*` means we splice
/// `rest` (the captured fragment from [`match_alias_pattern`]) into
/// the wildcard slot. Unbalanced shapes (literal pattern paired with
/// wildcard target, or vice versa) are tolerated by treating the
/// missing `rest` as the empty string.
fn substitute_alias_target(target: &str, rest: Option<&str>) -> String {
    let captured = rest.unwrap_or("");
    if let Some(star_idx) = target.find('*') {
        let mut out = String::with_capacity(target.len() + captured.len().saturating_sub(1));
        out.push_str(&target[..star_idx]);
        out.push_str(captured);
        out.push_str(&target[star_idx + 1..]);
        out
    } else {
        target.to_string()
    }
}

/// Look up `exports["./<subpath>"]` in a parsed `package.json` and
/// return the redirected on-disk relative path, if any.
///
/// Supports the two shapes workspace TypeScript packages use:
///
/// - Single string: `"./sub": "./src/sub/index.ts"` — returned as-is.
/// - Conditional object: `"./sub": { "source": "...", "default": "..." }`
///   — flattened in priority `source` → `default` → `import` to prefer
///   un-built source over compiled output.
///
/// **Out of scope** (intentional limits):
///
/// - Subpath patterns with `*` wildcards (`"./components/*": "..."`).
///   The downstream consumers that motivated this fix all use literal
///   keys; pattern support is left for a follow-up if needed.
/// - Full Node.js conditional priority (`browser`, `node`, etc.). The
///   `source` / `default` / `import` walk is intentionally minimal —
///   workspace packages overwhelmingly use one of those three.
/// - Top-level `exports` shorthand (a bare string or array at the root
///   of `exports`, with no `./<sub>` keys). That shape is the
///   "no-subpath" case which the bare-package branch already handles
///   via `module` / `main`.
fn resolve_exports_subpath(pkg_json: &serde_json::Value, subpath: &str) -> Option<String> {
    let exports = pkg_json.get("exports")?;
    let key = format!("./{subpath}");
    let entry = exports.get(&key)?;
    flatten_exports_entry(entry)
}

/// Resolve the top-level package entry (`import "@scope/pkg"`, no subpath)
/// from a parsed `package.json` `exports` field. Returns the redirected
/// on-disk relative path, if any.
///
/// Handles the three `exports` shapes a top-level entry can take:
///
/// - **Shorthand string / array** — `"exports": "./index.js"` or
///   `["./a.js", "./b.js"]` — flattened directly (first usable string).
/// - **Subpath map** — `{ ".": <entry>, "./sub": … }` — the `"."` entry is
///   flattened. A subpath map with no explicit `"."` key has no top-level
///   entry, so `None` is returned (the caller falls back to
///   `module`/`main`).
/// - **Dot-condition object** — `{ "import": "…", "default": "…" }` with no
///   `"./"`-prefixed keys — the whole object IS the dot export's condition
///   set, flattened in the usual `source → default → import` priority.
///
/// Conditional flattening (and the `module`/`require`/`types` conditions it
/// intentionally skips) is shared with [`flatten_exports_entry`]; see its
/// docs. This is the `"."` analogue of [`resolve_exports_subpath`], added
/// for issue #999 (npm dist packages that expose their entry only through
/// `exports`, with no `main`/`module`).
fn resolve_exports_dot(pkg_json: &serde_json::Value) -> Option<String> {
    let exports = pkg_json.get("exports")?;
    match exports {
        serde_json::Value::String(_) | serde_json::Value::Array(_) => {
            flatten_exports_entry(exports)
        }
        serde_json::Value::Object(map) => {
            if let Some(dot) = map.get(".") {
                flatten_exports_entry(dot)
            } else if map.keys().any(|k| k.starts_with("./")) {
                // A subpath map without an explicit "." entry: no
                // top-level export. Fall back to `module`/`main`.
                None
            } else {
                // No "./"-prefixed keys: the object is the dot export's
                // condition set (e.g. `{ "import": …, "default": … }`).
                flatten_exports_entry(exports)
            }
        }
        _ => None,
    }
}

/// Recursively pick a target string out of an `exports` entry value.
///
/// Plain strings are returned verbatim; conditional objects are
/// walked in priority `source` → `default` → `import`. The first key
/// that resolves to a string (after recursion through nested
/// conditional shapes) wins. Arrays of fallbacks (`["./a", "./b"]`)
/// take the first string entry.
fn flatten_exports_entry(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(_) => {
            // Priority: `source` (un-built TS workspace source) → `module`
            // / `default` (ESM dist) → `import` (ESM entry condition).
            // `require`/`types`/`node` are intentionally skipped — we only
            // scan ESM source. `default` is kept ahead of `import` to
            // preserve the pre-#999 contract (see the
            // `resolve_exports_subpath_prefers_*` tests).
            for cond in ["source", "module", "default", "import"] {
                if let Some(inner) = value.get(cond) {
                    if let Some(found) = flatten_exports_entry(inner) {
                        return Some(found);
                    }
                }
            }
            None
        }
        serde_json::Value::Array(arr) => {
            for inner in arr {
                if let Some(found) = flatten_exports_entry(inner) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

/// Generate TS-family swap candidate paths for a candidate that uses
/// a JS-family extension (`.js`, `.jsx`, `.mjs`, `.cjs`).
///
/// `moduleResolution: "Bundler"` and `"NodeNext"` accept import
/// specifiers that name the post-emit JS file (`./foo.js`) even when
/// the on-disk source is `./foo.tsx`. esbuild and `tsc` both swap the
/// extension transparently; the islands scanner needs the same logic
/// so it can follow imports through workspace TypeScript packages
/// that ship un-built source.
///
/// Mapping (matches TypeScript's `getSuffixedSubstitutions` ordering):
///
/// - `.js`  → `.ts`,  `.tsx`
/// - `.jsx` → `.tsx`
/// - `.mjs` → `.mts`
/// - `.cjs` → `.cts`
///
/// Returns an empty vector for any other extension (or no extension)
/// so callers fall straight through to the legacy "append extension"
/// probe loop. Match is ASCII-case-insensitive for filesystems that
/// case-fold (macOS default, Windows).
fn ts_swap_candidates(path: &Path) -> Vec<PathBuf> {
    let Some(ext_os) = path.extension() else {
        return Vec::new();
    };
    let Some(ext) = ext_os.to_str() else {
        return Vec::new();
    };
    let lower = ext.to_ascii_lowercase();
    let swaps: &[&str] = match lower.as_str() {
        "js" => &["ts", "tsx"],
        "jsx" => &["tsx"],
        "mjs" => &["mts"],
        "cjs" => &["cts"],
        _ => return Vec::new(),
    };
    swaps
        .iter()
        .map(|new_ext| path.with_extension(new_ext))
        .collect()
}

impl Resolver for FsResolver {
    fn resolve(&self, importer_dir: &Path, specifier: &str) -> Option<PathBuf> {
        // Helper: turn a found path into the canonical (symlink-resolved,
        // `..`-collapsed) form so the visited set and Island::source_path
        // are stable regardless of how many times the same file is reached
        // through different specifiers.
        let canonicalize = |p: PathBuf| p.canonicalize().unwrap_or(p);

        if is_bare_specifier(specifier) {
            // 1) tsconfig path aliases (issue #139). Bare specifiers
            //    like `@/components/foo` are not on the
            //    relative-import path, so they would otherwise fall
            //    through to the node_modules / workspace probe. Walk
            //    up from the importer to find the nearest
            //    `tsconfig.json`, parse `compilerOptions.paths`, and
            //    if the specifier matches an alias, probe the
            //    substitution targets like a relative path.
            if let Some(found) = self.try_resolve_tsconfig_alias(importer_dir, specifier) {
                return Some(canonicalize(found));
            }

            if !self.workspace_probe_enabled {
                return None;
            }
            // Ignore obviously-not-on-disk specifiers that surface in
            // every project (the framework-provided ones zfb_render's
            // loader fakes). Walking node_modules for these is wasted
            // work; failing the probe also means we skip them silently.
            let (pkg_name, subpath) = Self::split_bare_specifier(specifier);
            let pkg_dir = Self::locate_node_modules_pkg(importer_dir, &pkg_name)?;
            // Descent policy for bare specifiers (issue #999):
            //
            // - **pnpm-workspace packages** (a symlink at
            //   `node_modules/<pkg>` whose canonical target lives OUTSIDE
            //   `node_modules/`) are always entered — un-built TS source
            //   carries `"use client"` islands.
            // - **Regular npm packages** (a published `@scope/pkg`
            //   consumed from npm; the symlink resolves into
            //   `node_modules/.pnpm/…`) are entered when the importer is
            //   NOT itself inside `node_modules`. This is the #999 npm-dist
            //   islands path: a page/component importing a dist-shipped
            //   `"use client"` module (directly, or through the package's
            //   own barrels via relative imports) must register its
            //   islands.
            //
            // The single hard invariant (the part of the PR #125 guarantee
            // that still holds unconditionally): a bare import made from
            // INSIDE `node_modules` — i.e. one regular package's dist
            // reaching for `preact`, `@takazudo/zfb-runtime`, another
            // `@scope/pkg`, … — is NEVER followed. So the scan never walks
            // the *transitive* framework dependency graph; once inside a
            // regular package only its own RELATIVE-import closure is
            // traversed.
            //
            // What #999 DOES relax vs PR #125: a regular package's directly
            // imported entry/subpath is now scanned (one parse, plus its
            // relative closure), even for framework packages like `preact`.
            // PR #125 skipped these outright. The cost is bounded — no
            // transitive crawl — and real framework dist carries no
            // `"use client"`, so no spurious islands are emitted; the
            // trade is accepting those extra entry parses in exchange for
            // surfacing islands genuinely published in npm dist. Note this
            // relaxation also applies when the importer is a *workspace
            // package's* source (which lives outside `node_modules`), not
            // only top-level project source — distinguishing the two would
            // require threading project-root provenance through the
            // resolver, which is out of scope here.
            if !Self::is_workspace_package(&pkg_dir)
                && Self::is_inside_node_modules(importer_dir)
            {
                return None;
            }
            return self
                .probe_package_entry(&pkg_dir, &subpath)
                .map(canonicalize);
        }

        let candidate = importer_dir.join(specifier);

        // 1) TS-family swap (`.js` → `.ts`/`.tsx`, `.jsx` → `.tsx`,
        //    `.mjs` → `.mts`, `.cjs` → `.cts`) — the
        //    `moduleResolution: "Bundler"` / NodeNext convention where
        //    sources author `import "./foo.js"` against an on-disk
        //    `foo.tsx`. The TS sibling wins over a same-named `.js` so a
        //    stale build artifact next to live source can't shadow the
        //    real source the bundler will use. See
        //    [`Self::probe_with_extensions`] for the motivation and
        //    issue reference.
        for swapped in ts_swap_candidates(&candidate) {
            if swapped.is_file() {
                return Some(canonicalize(swapped));
            }
        }

        // 2) Exact match.
        if candidate.is_file() {
            return Some(canonicalize(candidate));
        }

        // 3) Probe extensions.
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

        // 4) `index.<ext>` inside a directory.
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

        // TS-family swap runs first so a stale `.js` doesn't shadow the
        // live `.tsx`. See [`ts_swap_candidates`] for rationale.
        for swapped in ts_swap_candidates(&candidate) {
            if self.files.contains_key(&swapped) {
                return Some(swapped);
            }
        }

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
///
/// This is a thin wrapper over [`scan_islands_with_meta`] that drops the
/// [`ScanMeta`] side-channel — kept so the long-standing
/// `scan_islands -> Vec<Island>` contract and every caller / test built
/// on it stay unchanged.
pub fn scan_islands<R: Resolver>(pages: &[PathBuf], resolver: &R) -> ScanResult<IslandsSet> {
    scan_islands_with_meta(pages, resolver).map(|(islands, _meta)| islands)
}

/// Like [`scan_islands`], but also returns [`ScanMeta`] — side-channel
/// facts gathered during the same single DFS pass that are not
/// themselves islands (today: whether the project uses `<ClientRouter />`,
/// issue #289).
///
/// The DFS is shared with [`scan_islands`]; this is the function that
/// actually does the work.
pub fn scan_islands_with_meta<R: Resolver>(
    pages: &[PathBuf],
    resolver: &R,
) -> ScanResult<(IslandsSet, ScanMeta)> {
    // BTreeMap keyed by (path, name) gives us natural sort order on output
    // and dedup for free.
    let mut found: BTreeMap<(PathBuf, String), Island> = BTreeMap::new();
    // Visited file set so cyclic imports terminate.
    let mut visited: HashSet<PathBuf> = HashSet::new();
    // DFS stack.
    let mut stack: Vec<PathBuf> = pages.to_vec();
    // Per-module `<ClientRouter />` opt-in facts (#289, #307), keyed by
    // resolved path — the same key space as `visited`. The cross-module
    // verdict (following `export *` re-export edges) is computed from
    // these once the walk completes.
    let mut client_router_facts: HashMap<PathBuf, ClientRouterFacts> = HashMap::new();
    // Issue #822: count modules that contain a `"use client"`-like
    // substring but contributed no island — likely a misplaced /
    // misspelled directive the empty-islands warning should point at.
    let mut near_miss_candidates: usize = 0;

    while let Some(current) = stack.pop() {
        if !visited.insert(current.clone()) {
            continue;
        }

        // Skip files whose extension can't carry a `"use client"` directive
        // the scanner needs to follow. After #139 the resolver follows
        // tsconfig path aliases, so a specifier like `"#data"` can resolve
        // to a `.json` (or `.css`, `.svg`, image/font binary, etc.) — none
        // of those parse as TS/TSX. Treat them as terminal leaves; their
        // imports are not walked further. See issue #141.
        if !is_scannable_source(&current) {
            continue;
        }

        let source = resolver
            .read(&current)
            .map_err(|message| ScanError::Resolver {
                path: current.clone(),
                message,
            })?;

        let module = parse_module(&current, &source)?;

        // Directory of the importer — used to resolve this module's own
        // specifiers, both for `<ClientRouter />` fact collection below
        // and for the import walk further down.
        let importer_dir = current
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        // `<ClientRouter />` opt-in fact collection (#289, #307).
        // Gathered for every reachable module, not just `"use client"`
        // files — the import typically lives in an SSR-only layout that
        // carries no directive.
        let cr_facts =
            collect_client_router_facts(&module, |spec| resolver.resolve(&importer_dir, spec));

        if has_use_client_directive(&module) {
            let records = exported_island_records(&module);
            if records.is_empty() {
                // Issue #822: a valid `"use client"` module that exports
                // nothing is still a near-miss — the author flagged it as
                // an island but gave the bundler nothing to ship. Keep the
                // verify-hint pointing at it.
                near_miss_candidates += 1;
            }
            for record in records {
                let key = (current.clone(), record.component_name.clone());
                let path = current.clone();
                found.entry(key).or_insert_with(|| {
                    Island::with_marker_name(record.component_name, path, record.marker_name)
                });
            }
        } else if has_near_miss_use_client_directive(&module) {
            // Issue #822: the module didn't register a valid directive
            // (so `has_use_client_directive` is false) yet a top-level
            // string-literal statement normalizes to `use client` — a
            // misplaced, mis-cased, or whitespace-malformed directive is
            // the most likely cause. Count it so the caller can keep the
            // loud verify-hint for this genuine near-miss instead of
            // nagging an intentionally island-free project. AST-based, so
            // a `// use client` comment never trips it.
            near_miss_candidates += 1;
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
        for specifier in collect_import_specifiers(&module) {
            if let Some(resolved) = resolver.resolve(&importer_dir, &specifier) {
                if !visited.contains(&resolved) {
                    stack.push(resolved);
                }
            }
        }

        client_router_facts.insert(current, cr_facts);
    }

    Ok((
        found.into_values().collect(),
        ScanMeta {
            uses_client_router: resolve_client_router_usage(&client_router_facts),
            near_miss_candidates,
        },
    ))
}

/// Bare specifier of the `@takazudo/zfb-runtime` barrel package.
const ZFB_RUNTIME_SPECIFIER: &str = "@takazudo/zfb-runtime";

/// Bare specifier of the client-router subpath. Importing this for its
/// side effect is what registers the View Transitions runtime (`init()`).
const ZFB_RUNTIME_CLIENT_ROUTER_SPECIFIER: &str = "@takazudo/zfb-runtime/client-router";

/// The named export the `<ClientRouter />` API is published under.
const CLIENT_ROUTER_EXPORT: &str = "ClientRouter";

/// Per-module facts the `<ClientRouter />` opt-in detection needs
/// (issues #289, #307).
///
/// Gathered once per reachable module by [`collect_client_router_facts`]
/// during the shared DFS in [`scan_islands_with_meta`]; the project-wide
/// verdict — which has to follow `export *` re-export edges across
/// modules — is computed afterwards by [`resolve_client_router_usage`].
#[derive(Default)]
struct ClientRouterFacts {
    /// The module on its own conclusively opts the project into
    /// `<ClientRouter />`: a named `ClientRouter` import / re-export from
    /// the `@takazudo/zfb-runtime` barrel, or any pull-in of the
    /// `@takazudo/zfb-runtime/client-router` subpath.
    direct_hit: bool,
    /// The module has a (non-type) `export * from "@takazudo/zfb-runtime"`
    /// — it re-exports the runtime *barrel*, so it is itself a
    /// "runtime-barrel proxy".
    reexports_runtime_barrel: bool,
    /// Resolved paths of every (non-type) `export * from "<local>"`.
    /// These are the edges followed transitively to grow the proxy set:
    /// a module that `export *`s a proxy is itself a proxy.
    star_reexport_targets: Vec<PathBuf>,
    /// Resolved paths from which this module pulls a (non-type) named
    /// `ClientRouter` binding — `import { ClientRouter [as X] } from
    /// "<local>"` or `export { ClientRouter [as X] } from "<local>"`.
    /// If any such path is a runtime-barrel proxy, the project uses
    /// `<ClientRouter />` (issue #307).
    client_router_pulls: Vec<PathBuf>,
}

/// Gather the per-module [`ClientRouterFacts`] for one module.
///
/// `resolve` maps a module specifier to the same resolved [`PathBuf`] key
/// the DFS `visited` set uses, so the local proxy modules and the paths
/// recorded here share a single key space.
///
/// **Direct opt-in** (`direct_hit`) — conclusive on its own — covers:
///
/// - Anything pulling in the `@takazudo/zfb-runtime/client-router`
///   subpath directly: a plain `import`, a side-effect `import "…"`, a
///   named re-export `export { … } from "…"`, or `export * from "…"`.
/// - A named `ClientRouter` binding imported from — or re-exported out
///   of — the `@takazudo/zfb-runtime` barrel. Renamed forms
///   (`import { ClientRouter as CR }`) match on the *imported* name, not
///   the local binding.
///
/// **Cross-module opt-in** — issue #307. A project can reach
/// `<ClientRouter />` through a local barrel: `src/lib/runtime.ts` does
/// `export * from "@takazudo/zfb-runtime"`, and app code then does
/// `import { ClientRouter } from "../lib/runtime"`. Neither module is a
/// direct hit, so this function records the structural facts
/// (`reexports_runtime_barrel`, `star_reexport_targets`,
/// `client_router_pulls`) and [`resolve_client_router_usage`] computes
/// the transitive verdict after the DFS finishes.
///
/// A bare `import * as rt from "…"` namespace import — and its
/// export-side mirror `export * as rt from "…"` — is intentionally NOT a
/// trigger and NOT a proxy edge: it cannot be tied to `ClientRouter`
/// without member-expression analysis, and treating it as one would risk
/// shipping the runtime to projects that never touch `<ClientRouter />`
/// (issue #289 acceptance criterion 2 — zero new bytes for non-users).
///
/// TypeScript type-only forms are skipped entirely — `import type { … }`,
/// per-specifier `{ type ClientRouter }`, `export type { … }`, and a
/// type-only `export type * from "…"`. They are compile-time-erased (zero
/// runtime code), so counting them would ship the client-router runtime
/// to a consumer that only references `ClientRouter` as a type, again
/// breaking acceptance criterion 2.
///
/// The `@takazudo/zfb-runtime/client-router` subpath is a side-effect
/// entrypoint: importing it runs `init()` at module-eval time (see issue
/// #289 Background). Any import of that subpath — even for a non-router
/// helper like `navigate`/`prefetch` — opts the project into the runtime,
/// so detection on the subpath is deliberately not narrowed to
/// `ClientRouter`.
///
/// Accepted over-approximation: if a runtime-barrel proxy *also* declares
/// its own export named `ClientRouter` (a local `export const/function`
/// or a named re-export from a non-runtime module), an explicit export
/// shadows the `export *` re-export under ES module semantics — so a
/// `ClientRouter` pull from that barrel would not bind the runtime's
/// `ClientRouter`. This detection does not track that shadowing and would
/// still treat the pull as a trigger. The miss is benign: the only cost
/// is shipping a few unused KB of runtime to a project that named its own
/// component `ClientRouter` inside a barrel re-exporting the zfb runtime —
/// an extraordinarily unlikely shape — never a broken build. Contrast the
/// issue #307 false *negative* this function fixes, which left a genuinely
/// used runtime un-bundled. Same trade-off the unused-import note on
/// [`ScanMeta::uses_client_router`] already accepts.
fn collect_client_router_facts(
    module: &Module,
    mut resolve: impl FnMut(&str) -> Option<PathBuf>,
) -> ClientRouterFacts {
    let mut facts = ClientRouterFacts::default();
    for item in &module.body {
        let ModuleItem::ModuleDecl(decl) = item else {
            continue;
        };
        match decl {
            ModuleDecl::Import(import_decl) => {
                // `import type { … }` is erased at compile time — no
                // runtime code, so it can never opt into the runtime.
                if import_decl.type_only {
                    continue;
                }
                let src = &import_decl.src.value;
                if *src == *ZFB_RUNTIME_CLIENT_ROUTER_SPECIFIER {
                    facts.direct_hit = true;
                    continue;
                }
                let is_runtime_barrel = *src == *ZFB_RUNTIME_SPECIFIER;
                for spec in &import_decl.specifiers {
                    // Namespace (`import * as rt`) and default imports
                    // cannot be tied to `ClientRouter` — skip them.
                    let ImportSpecifier::Named(named) = spec else {
                        continue;
                    };
                    // Per-specifier `{ type ClientRouter }` is erased.
                    if named.is_type_only {
                        continue;
                    }
                    // `imported` is `Some` for renamed imports
                    // (`{ ClientRouter as CR }`); `None` means the local
                    // binding IS the imported name.
                    let imported = match &named.imported {
                        Some(name) => module_export_name(name),
                        None => named.local.sym.to_string(),
                    };
                    if imported != CLIENT_ROUTER_EXPORT {
                        continue;
                    }
                    if is_runtime_barrel {
                        facts.direct_hit = true;
                    } else if let Some(path) = resolve(&atom_to_string(src)) {
                        // `ClientRouter` pulled from a non-runtime
                        // specifier — a trigger only if that module turns
                        // out to be a runtime-barrel proxy (#307).
                        facts.client_router_pulls.push(path);
                    }
                }
            }
            ModuleDecl::ExportNamed(named) => {
                // `export type { … }` is erased at compile time.
                if named.type_only {
                    continue;
                }
                let Some(src) = &named.src else {
                    // `export { … }` with no `from` is a local re-export,
                    // not a module pull-in.
                    continue;
                };
                let src = &src.value;
                if *src == *ZFB_RUNTIME_CLIENT_ROUTER_SPECIFIER {
                    facts.direct_hit = true;
                    continue;
                }
                let is_runtime_barrel = *src == *ZFB_RUNTIME_SPECIFIER;
                for spec in &named.specifiers {
                    // `export * as ns from "…"` is a namespace re-export
                    // (the export-side mirror of `import * as`) — not a
                    // `ClientRouter` pull. Default re-exports likewise.
                    let ExportSpecifier::Named(export_named) = spec else {
                        continue;
                    };
                    // Per-specifier `{ type ClientRouter }` is erased.
                    if export_named.is_type_only {
                        continue;
                    }
                    // `orig` is the name in the source module.
                    if module_export_name(&export_named.orig) != CLIENT_ROUTER_EXPORT {
                        continue;
                    }
                    if is_runtime_barrel {
                        facts.direct_hit = true;
                    } else if let Some(path) = resolve(&atom_to_string(src)) {
                        facts.client_router_pulls.push(path);
                    }
                }
            }
            ModuleDecl::ExportAll(all) => {
                // `export type * from "…"` is compile-time-erased — never
                // a runtime pull, never a proxy edge.
                if all.type_only {
                    continue;
                }
                let src = &all.src.value;
                if *src == *ZFB_RUNTIME_CLIENT_ROUTER_SPECIFIER {
                    facts.direct_hit = true;
                } else if *src == *ZFB_RUNTIME_SPECIFIER {
                    // `export * from "@takazudo/zfb-runtime"` — this
                    // module re-exports the runtime barrel: it is a proxy.
                    facts.reexports_runtime_barrel = true;
                } else if let Some(path) = resolve(&atom_to_string(src)) {
                    // `export * from "<local>"` — a proxy-set edge,
                    // resolved transitively after the DFS.
                    facts.star_reexport_targets.push(path);
                }
            }
            _ => {}
        }
        // A direct hit is conclusive for this module — and the project
        // verdict short-circuits on any direct hit — so the remaining
        // declarations (and their resolver calls) cannot change the
        // outcome. Stop scanning, matching the pre-#307 early return.
        if facts.direct_hit {
            break;
        }
    }
    facts
}

/// Compute the project-wide `<ClientRouter />` verdict from the per-module
/// [`ClientRouterFacts`] gathered during the scan (issues #289, #307).
///
/// A `direct_hit` on any reachable module is conclusive. Otherwise the
/// project still opts in when a `ClientRouter` name is pulled from a
/// *runtime-barrel proxy* — a module that transitively re-exports
/// `@takazudo/zfb-runtime` through one or more `export *` hops.
///
/// The proxy set is grown to a fixpoint with a worklist that re-scans
/// every module each pass and only ever *adds* modules. That terminates
/// even when barrels `export *` each other cyclically: a pass either
/// flips at least one new module into the set or makes no change and the
/// loop ends.
fn resolve_client_router_usage(facts: &HashMap<PathBuf, ClientRouterFacts>) -> bool {
    if facts.values().any(|f| f.direct_hit) {
        return true;
    }
    // Seed with the modules that directly re-export the runtime barrel,
    // then follow `export *` edges until the set stops growing.
    let mut proxies: HashSet<PathBuf> = facts
        .iter()
        .filter(|(_, f)| f.reexports_runtime_barrel)
        .map(|(path, _)| path.clone())
        .collect();
    loop {
        let mut changed = false;
        for (path, f) in facts {
            if proxies.contains(path) {
                continue;
            }
            if f.star_reexport_targets
                .iter()
                .any(|target| proxies.contains(target))
            {
                proxies.insert(path.clone());
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    facts.values().any(|f| {
        f.client_router_pulls
            .iter()
            .any(|path| proxies.contains(path))
    })
}

/// Extract the textual name from a [`ModuleExportName`] (an identifier or
/// a string-literal export name).
fn module_export_name(name: &ModuleExportName) -> String {
    match name {
        ModuleExportName::Ident(id) => id.sym.to_string(),
        ModuleExportName::Str(s) => atom_to_string(&s.value),
    }
}

/// Return `true` if `path` has an extension that can plausibly carry a
/// `"use client"` directive that the scanner needs to follow.
///
/// Conservative whitelist: only TS/JS family source extensions. Anything
/// else — `.json`, `.css`, `.svg`, image/font binaries, `.md`/`.mdx`, etc.
/// — is a terminal leaf as far as the islands scanner is concerned, and
/// must not be passed to SWC (which would parse it as TS and crash; see
/// issue #141).
///
/// Files with no extension at all are also skipped: real source files in
/// this codebase always carry an extension, and an unknown leaf is safer
/// to skip than to feed to SWC.
///
/// The match is ASCII-case-insensitive so that on case-insensitive
/// filesystems (macOS default, Windows) a `Foo.TSX` file is still
/// recognised — pre-#141 the scanner accepted any extension, so case
/// folding here preserves parity for legitimate source files.
fn is_scannable_source(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts"
    )
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

/// Issue #822: return true iff the module looks like a *near-miss*
/// `"use client"` island — a directive the author clearly intended but
/// that [`has_use_client_directive`] rejected.
///
/// This is the signal the empty-islands report uses to keep its loud
/// warning + verify-hint instead of demoting to a quiet "island-free on
/// purpose" note. Callers should only consult it once
/// [`has_use_client_directive`] has already returned false.
///
/// We scan *every* top-level expression-statement string literal in the
/// module body — not just the leading prologue — and compare a normalized
/// value (lowercased, trimmed, internal whitespace collapsed to a single
/// space) against `use client`. That catches the realistic authoring
/// mistakes:
///
/// - a misplaced directive — `"use client"` after an `import`, so it is
///   no longer in the prologue (`has_use_client_directive` stops at the
///   first non-string statement);
/// - a mis-cased directive — `"Use client"`, `"USE CLIENT"`;
/// - a whitespace-malformed directive — `"use  client"`, `"use client "`.
///
/// Crucially, it works on the parsed AST, not the raw file text, so a
/// `// use client` comment or a `"...use client..."` substring inside an
/// ordinary expression is *not* a false positive — neither is a top-level
/// `ExprStmt(Str)`. False positives matter more than false negatives here:
/// a false positive resurrects exactly the warning #822 is silencing on
/// an intentionally island-free site.
fn has_near_miss_use_client_directive(module: &Module) -> bool {
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Expr(expr_stmt)) = item else {
            continue;
        };
        let Expr::Lit(Lit::Str(s)) = &*expr_stmt.expr else {
            continue;
        };
        if normalize_directive(&s.value) == "use client" {
            return true;
        }
    }
    false
}

/// Lowercase, trim, and collapse internal whitespace runs to a single
/// space — the normalization used to match malformed `"use client"`
/// directives (issue #822). Returns an owned [`String`]; only ever called
/// on the short directive literals in a module prologue, so the
/// allocation is negligible.
fn normalize_directive(value: &Wtf8Atom) -> String {
    atom_to_string(value)
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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
                // `import type { … }` is erased at compile time — the module
                // is never loaded at runtime, so it must not become a DFS edge.
                if import_decl.type_only {
                    continue;
                }
                out.push(atom_to_string(&import_decl.src.value));
            }
            ModuleDecl::ExportNamed(named) => {
                // `export type { … } from "…"` is compile-time-erased — not a
                // runtime pull, so it must not become a DFS edge.
                if named.type_only {
                    continue;
                }
                if let Some(src) = &named.src {
                    out.push(atom_to_string(&src.value));
                }
            }
            ModuleDecl::ExportAll(all) => {
                // `export type * from "…"` is compile-time-erased — not a
                // runtime pull, so it must not become a DFS edge.
                if all.type_only {
                    continue;
                }
                out.push(atom_to_string(&all.src.value));
            }
            _ => {}
        }
    }
    out
}

/// One exported binding paired with its SSR-marker name.
///
/// Returned by [`exported_island_records`] so the scanner can hand the
/// bundler both the export-side identity (`component_name`, used as the
/// dedup key) and the SSR-side identity (`marker_name`, used as the
/// hydration-manifest key in the production shared bundle).
#[derive(Debug, Clone, PartialEq, Eq)]
struct IslandRecord {
    component_name: String,
    marker_name: String,
}

/// Return `true` when an initialiser expression is *clearly* a
/// non-component value that must NOT be registered as a mountable island
/// (issue #998).
///
/// The rule is deliberately CONSERVATIVE — it drops only shapes that can
/// never be a component:
///
/// - primitive/literal values (`'foo'`, `42`, `true`, `null`, `/re/`, `1n`)
/// - untagged template strings (`` `foo${x}` ``)
/// - array literals
/// - object literals
///
/// Everything else is treated as component-plausible and KEPT: function
/// expressions, arrow functions, ANY call expression (`memo(...)`,
/// `forwardRef(...)`, `lazy(...)`, `styled(...)`, `connect(...)(...)` — no
/// callee whitelist), tagged templates (`` styled.div`…` `` parse as
/// `Expr::TaggedTpl`, distinct from `Expr::Tpl`), bare identifiers,
/// conditional expressions, `as`-casts / `satisfies`, member access, and
/// anything else. When in doubt, keep.
///
/// `export const SIDEBAR_STORAGE_KEY = 'zudo-doc-sidebar-visible'` — the
/// repro from #998 — is a string literal and gets dropped here.
fn init_is_clearly_non_component(init: &Expr) -> bool {
    match init {
        Expr::Lit(_) | Expr::Tpl(_) | Expr::Array(_) | Expr::Object(_) => true,
        // Peel a redundant wrapping paren and re-test. TS casts
        // (`x as T`, `x satisfies T`, `x!`) are intentionally NOT peeled:
        // the spec treats them as ambiguous and keeps them.
        Expr::Paren(p) => init_is_clearly_non_component(&p.expr),
        _ => false,
    }
}

/// `true` when a single `const`/`let`/`var` declarator's initialiser is a
/// clearly-non-component value. A declarator with no initialiser
/// (`let Foo;`) is ambiguous → kept.
fn var_declarator_is_clearly_non_component(d: &VarDeclarator) -> bool {
    match d.init.as_ref() {
        Some(init) => init_is_clearly_non_component(init),
        None => false,
    }
}

/// Collect the names of module-local `const`/`let`/`var` bindings whose
/// initialiser is a clearly-non-component value (see
/// [`init_is_clearly_non_component`]).
///
/// Used to drop *local* named re-exports — `const KEY = 'x'; export { KEY }`
/// — that resolve to a non-component binding (issue #998). Bindings that
/// are functions, classes, component-shaped initialisers, OR imported from
/// another module are absent from this set, so a re-export of such a
/// binding stays registered (resolving cross-module re-exports is out of
/// scope; those are kept permissively).
fn collect_non_component_local_bindings(module: &Module) -> HashSet<String> {
    let mut out: HashSet<String> = HashSet::new();
    for item in &module.body {
        let decl = match item {
            ModuleItem::Stmt(Stmt::Decl(decl)) => decl,
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ed)) => &ed.decl,
            _ => continue,
        };
        if let Decl::Var(v) = decl {
            for d in &v.decls {
                if let Pat::Ident(bi) = &d.name {
                    if var_declarator_is_clearly_non_component(d) {
                        out.insert(bi.id.sym.to_string());
                    }
                }
            }
        }
    }
    out
}

/// Walk the module's exports and pair each one with its SSR-marker name.
///
/// The marker name is what the SSR side will write into the
/// `data-zfb-island` / `data-zfb-island-skip-ssr` attribute (see
/// `packages/zfb/src/island.ts::captureComponentName`). The shared-bundle
/// hydration manifest is keyed on this name; using the export-side name
/// directly would mis-key every host-shape default-export island as
/// `"default"` (issue #149 Gap B) and every SSR-skip wrapper as the
/// wrapper's identifier instead of its placeholder marker (Gap A).
///
/// Resolution rules:
///
/// - `export default function Foo()` / `export default class Foo` →
///   `component_name = "default"`, `marker_name = "Foo"` (the identifier
///   name; matches `function.name` at SSR time before minification).
/// - `export default function ()` / `export default <anon-expr>` →
///   `component_name = "default"`, `marker_name = "default"` (no
///   recoverable identity; the bundler still registers the entry, but
///   the SSR side will need [`ANONYMOUS_COMPONENT_NAME`] to match).
/// - `export function Foo()` / `export class Foo` /
///   `export const Foo = …` → `component_name = "Foo"`,
///   `marker_name = "Foo"`.
/// - `export { A, B as C }` → `(A, A)`, `(C, C)` — re-exports don't
///   carry an inline body, so the wrapper detection below does not apply.
///
/// Then each function body is scanned for a top-level
/// `renderSsrSkipPlaceholder("X", …)` call (the host-side SSR-skip
/// wrapper convention, see
/// `packages/zudo-doc-v2/src/ssr-skip/types.ts::renderSsrSkipPlaceholder`).
/// When found, the literal first-argument string overrides the
/// rule-based `marker_name` so the manifest key matches
/// `data-zfb-island-skip-ssr="X"` rather than the wrapper's identifier.
///
/// ## Component filtering (issue #998)
///
/// Clearly-non-component exports are dropped so they never become
/// mountable-island registry entries — a string/number/object/array
/// literal export of a `"use client"` module (e.g.
/// `export const SIDEBAR_STORAGE_KEY = 'zudo-doc-sidebar-visible'`) is not
/// a component. The filter is conservative (see
/// [`init_is_clearly_non_component`]): only literal-shaped values, untagged
/// template strings, `export * as ns from "…"` namespace re-exports, and
/// local re-exports resolving to such values are dropped; everything
/// ambiguous is kept. Re-exports from another module
/// (`export { Foo } from "./x"`) are kept permissively.
fn exported_island_records(module: &Module) -> Vec<IslandRecord> {
    // 1. Build a side-table of `local_ident -> renderSsrSkipPlaceholder
    //    first-arg literal` for every function/variable declaration in
    //    the module body. Used below to override marker_name on the
    //    matching exported declaration.
    let body_markers = collect_body_markers(module);

    // Names of module-local bindings whose value is clearly a
    // non-component (issue #998) so local named re-exports of them can be
    // dropped below.
    let non_component_locals = collect_non_component_local_bindings(module);

    let mut out: Vec<IslandRecord> = Vec::new();
    for item in &module.body {
        let ModuleItem::ModuleDecl(decl) = item else {
            continue;
        };
        match decl {
            ModuleDecl::ExportDecl(ed) => match &ed.decl {
                Decl::Class(c) => {
                    let name = c.ident.sym.to_string();
                    let marker = body_markers
                        .get(&name)
                        .cloned()
                        .unwrap_or_else(|| name.clone());
                    out.push(IslandRecord {
                        component_name: name,
                        marker_name: marker,
                    });
                }
                Decl::Fn(f) => {
                    let name = f.ident.sym.to_string();
                    let marker_from_body = marker_from_function_body(f.function.body.as_ref());
                    let marker = marker_from_body
                        .or_else(|| body_markers.get(&name).cloned())
                        .unwrap_or_else(|| name.clone());
                    out.push(IslandRecord {
                        component_name: name,
                        marker_name: marker,
                    });
                }
                Decl::Var(v) => {
                    for d in &v.decls {
                        if let Pat::Ident(bi) = &d.name {
                            // Issue #998: drop clearly-non-component exports
                            // (`export const KEY = 'x'`) so a string / number /
                            // object literal never becomes a mountable island.
                            if var_declarator_is_clearly_non_component(d) {
                                continue;
                            }
                            let name = bi.id.sym.to_string();
                            let marker_from_init = marker_from_var_initialiser(d);
                            let marker = marker_from_init
                                .or_else(|| body_markers.get(&name).cloned())
                                .unwrap_or_else(|| name.clone());
                            out.push(IslandRecord {
                                component_name: name,
                                marker_name: marker,
                            });
                        }
                    }
                }
                _ => {}
            },
            ModuleDecl::ExportDefaultDecl(ed) => match &ed.decl {
                DefaultDecl::Fn(fexp) => {
                    let ident_name = fexp.ident.as_ref().map(|i| i.sym.to_string());
                    let marker = marker_from_function_body(fexp.function.body.as_ref())
                        .or_else(|| ident_name.clone())
                        .unwrap_or_else(|| "default".to_string());
                    out.push(IslandRecord {
                        component_name: "default".to_string(),
                        marker_name: marker,
                    });
                }
                DefaultDecl::Class(cexp) => {
                    let ident_name = cexp.ident.as_ref().map(|i| i.sym.to_string());
                    let marker = ident_name.unwrap_or_else(|| "default".to_string());
                    out.push(IslandRecord {
                        component_name: "default".to_string(),
                        marker_name: marker,
                    });
                }
                DefaultDecl::TsInterfaceDecl(_) => {
                    // Type-only export — no runtime value, but keep the
                    // historical "default" emission so dedup behaviour is
                    // unchanged. The bundler will see no usable component
                    // and skip it at register time.
                    out.push(IslandRecord {
                        component_name: "default".to_string(),
                        marker_name: "default".to_string(),
                    });
                }
            },
            ModuleDecl::ExportDefaultExpr(ee) => {
                // Issue #998: `export default 'foo'` / `export default { … }`
                // — a clearly-non-component default is not a mountable island.
                if init_is_clearly_non_component(&ee.expr) {
                    continue;
                }
                // `export default <expr>` — try to recover an identifier
                // from common shapes (`export default Foo`,
                // `export default forwardRef(Foo)`). Otherwise fall back
                // to "default".
                let marker =
                    marker_from_default_expr(&ee.expr).unwrap_or_else(|| "default".to_string());
                out.push(IslandRecord {
                    component_name: "default".to_string(),
                    marker_name: marker,
                });
            }
            ModuleDecl::ExportNamed(named) => {
                // `export type { … }` is compile-time-erased — skip the
                // entire declaration (mirrors collect_client_router_facts).
                if named.type_only {
                    continue;
                }
                for spec in &named.specifiers {
                    match spec {
                        ExportSpecifier::Named(n) => {
                            // Per-specifier `{ type Foo, Bar }` — Foo is
                            // erased; skip it (mirrors collect_client_router_facts).
                            if n.is_type_only {
                                continue;
                            }
                            let pick = n.exported.as_ref().unwrap_or(&n.orig);
                            let name = module_export_name(pick);
                            // For named re-exports the local ident is
                            // `n.orig` — that's what the body-marker
                            // table is keyed on.
                            let local = module_export_name(&n.orig);
                            // Issue #998: a *local* named re-export whose
                            // binding resolves to a clearly-non-component
                            // value (`const KEY = 'x'; export { KEY }`) must
                            // not be registered. Re-exports from another
                            // module (`export { Foo } from './x'`) carry a
                            // `src` and are kept permissively — resolving the
                            // other module is out of scope.
                            if named.src.is_none() && non_component_locals.contains(&local) {
                                continue;
                            }
                            let marker = body_markers.get(&local).cloned().unwrap_or_else(|| {
                                // `export { Foo as default }` — the exported alias
                                // is "default" but the SSR side uses
                                // `displayName ?? name`, which resolves to the
                                // local identifier ("Foo").  Fall back to `local`
                                // so the manifest key matches the runtime marker.
                                if name == "default" {
                                    local.clone()
                                } else {
                                    name.clone()
                                }
                            });
                            out.push(IslandRecord {
                                component_name: name,
                                marker_name: marker,
                            });
                        }
                        ExportSpecifier::Default(d) => {
                            let name = d.exported.sym.to_string();
                            out.push(IslandRecord {
                                component_name: name.clone(),
                                marker_name: name,
                            });
                        }
                        ExportSpecifier::Namespace(_n) => {
                            // Issue #998: `export * as ns from "…"` binds a
                            // module-namespace object, which is never a
                            // mountable component — drop it.
                            continue;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Walk every top-level function and variable declaration in the module
/// body and record the `renderSsrSkipPlaceholder("X", …)` literal each
/// one carries (if any). Returned as `local_ident -> marker_name` so the
/// caller can override the marker name on the matching exported
/// declaration.
///
/// This catches the host-shape pattern where a wrapper is declared at
/// module level and exported separately, e.g.
///
/// ```ignore
/// function AiChatModalIsland(props) {
///   return renderSsrSkipPlaceholder("AiChatModal", …);
/// }
/// export { AiChatModalIsland };
/// ```
///
/// The inline-export case (`export function AiChatModalIsland(…) { … }`)
/// is handled directly by [`exported_island_records`] reading the
/// function body of the matched export.
fn collect_body_markers(module: &Module) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(decl)) => match decl {
                Decl::Fn(f) => {
                    if let Some(marker) = marker_from_function_body(f.function.body.as_ref()) {
                        out.insert(f.ident.sym.to_string(), marker);
                    }
                }
                Decl::Var(v) => {
                    for d in &v.decls {
                        if let Pat::Ident(bi) = &d.name {
                            if let Some(marker) = marker_from_var_initialiser(d) {
                                out.insert(bi.id.sym.to_string(), marker);
                            }
                        }
                    }
                }
                _ => {}
            },
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ed)) => match &ed.decl {
                // `export function Foo(…) { … }` declarations also
                // populate the table so `Foo` resolves correctly when
                // later re-exported via `export { Foo as default }`.
                Decl::Fn(f) => {
                    if let Some(marker) = marker_from_function_body(f.function.body.as_ref()) {
                        out.insert(f.ident.sym.to_string(), marker);
                    }
                }
                Decl::Var(v) => {
                    for d in &v.decls {
                        if let Pat::Ident(bi) = &d.name {
                            if let Some(marker) = marker_from_var_initialiser(d) {
                                out.insert(bi.id.sym.to_string(), marker);
                            }
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    out
}

/// If `init` of a variable declarator is a function expression / arrow
/// function whose body contains a top-level
/// `renderSsrSkipPlaceholder("X", …)` call, return `"X"`.
fn marker_from_var_initialiser(d: &VarDeclarator) -> Option<String> {
    let init = d.init.as_ref()?;
    match init.as_ref() {
        Expr::Fn(f) => marker_from_function_body(f.function.body.as_ref()),
        Expr::Arrow(a) => match &*a.body {
            swc_core::ecma::ast::BlockStmtOrExpr::BlockStmt(b) => marker_from_block(b),
            swc_core::ecma::ast::BlockStmtOrExpr::Expr(e) => marker_from_expr(e),
        },
        _ => None,
    }
}

/// Pull a marker literal out of a function body if it contains a
/// top-level `renderSsrSkipPlaceholder("X", …)` call.
fn marker_from_function_body(body: Option<&BlockStmt>) -> Option<String> {
    marker_from_block(body?)
}

/// Walk a `BlockStmt`'s top-level statements looking for a
/// `return <expr>;` or `<expr>;` that is a
/// `renderSsrSkipPlaceholder("X", …)` call; return `"X"`.
fn marker_from_block(block: &BlockStmt) -> Option<String> {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Return(r) => {
                if let Some(arg) = r.arg.as_ref() {
                    if let Some(s) = marker_from_expr(arg) {
                        return Some(s);
                    }
                }
            }
            Stmt::Expr(es) => {
                if let Some(s) = marker_from_expr(&es.expr) {
                    return Some(s);
                }
            }
            _ => {}
        }
    }
    None
}

/// If `expr` is a `renderSsrSkipPlaceholder("X", …)` call (or a
/// parenthesised / type-asserted version of one), return `"X"`.
fn marker_from_expr(expr: &Expr) -> Option<String> {
    let inner = unwrap_expr(expr);
    let call = match inner {
        Expr::Call(c) => c,
        _ => return None,
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let callee_inner = unwrap_expr(callee);
    let is_target = match callee_inner {
        Expr::Ident(id) => id.sym == *RENDER_SSR_SKIP_PLACEHOLDER_NAME,
        // Member expressions like `helpers.renderSsrSkipPlaceholder(…)` —
        // unusual, but conservative to support.
        Expr::Member(m) => match &m.prop {
            swc_core::ecma::ast::MemberProp::Ident(id) => {
                id.sym == *RENDER_SSR_SKIP_PLACEHOLDER_NAME
            }
            _ => false,
        },
        _ => false,
    };
    if !is_target {
        return None;
    }
    let first = call.args.first()?;
    if first.spread.is_some() {
        return None;
    }
    let lit_expr = unwrap_expr(&first.expr);
    let Expr::Lit(Lit::Str(s)) = lit_expr else {
        return None;
    };
    Some(atom_to_string(&s.value))
}

/// `export default <expr>` recovery. Handles `export default Foo` and
/// `export default forwardRef(Foo)` shapes.
fn marker_from_default_expr(expr: &Expr) -> Option<String> {
    match unwrap_expr(expr) {
        Expr::Ident(id) => Some(id.sym.to_string()),
        Expr::Call(c) => {
            // `forwardRef(Foo)` / `memo(Foo)` style — first ident arg.
            for arg in &c.args {
                if arg.spread.is_some() {
                    continue;
                }
                if let Expr::Ident(id) = unwrap_expr(&arg.expr) {
                    return Some(id.sym.to_string());
                }
            }
            None
        }
        _ => None,
    }
}

/// Strip wrapping `(expr)` and TS `expr as T` / `<T>expr` /
/// `expr satisfies T` so identifier matching is robust.
fn unwrap_expr(expr: &Expr) -> &Expr {
    let mut e = expr;
    loop {
        match e {
            Expr::Paren(p) => e = &p.expr,
            Expr::TsAs(a) => e = &a.expr,
            Expr::TsTypeAssertion(a) => e = &a.expr,
            Expr::TsConstAssertion(a) => e = &a.expr,
            Expr::TsSatisfies(s) => e = &s.expr,
            Expr::TsNonNull(n) => e = &n.expr,
            _ => return e,
        }
    }
}

/// Name of the host-side SSR-skip placeholder helper that the scanner
/// recognises as a marker-name source. See the doc on
/// [`exported_island_records`] for the contract.
///
/// Defined as a `'static` slice so future scanner extensions can share
/// the recognition table (e.g. accept additional names) without making
/// each call-site re-spell the string.
const RENDER_SSR_SKIP_PLACEHOLDER_NAME: &str = "renderSsrSkipPlaceholder";

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

    /// Create a pnpm-workspace-style symlink at `link` pointing to
    /// `target`. pnpm links workspace members into `node_modules/<pkg>`
    /// as symlinks whose target lives in the workspace tree (outside
    /// `node_modules/`); the scanner uses that symlink-vs-real-dir
    /// distinction to tell workspace packages apart from regular npm
    /// dependencies. Tests that exercise the workspace-probe path
    /// must build the same shape so `is_workspace_package` returns
    /// `true`.
    ///
    /// Unix-only (the suite already runs on Unix in CI; the workspace
    /// symlink shape is what pnpm produces in real consumer projects).
    #[cfg(unix)]
    fn make_workspace_link(target: &Path, link: &Path) {
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent).expect("create link parent");
        }
        std::os::unix::fs::symlink(target, link).expect("symlink workspace package");
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

    // --- `<ClientRouter />` detection (issue #289) ----------------------

    #[test]
    fn client_router_detected_via_named_import_from_runtime_barrel() {
        // The realistic shape: an SSR-only layout imports `ClientRouter`
        // from the `@takazudo/zfb-runtime` barrel and renders it in
        // `<head>`. The layout carries no `"use client"` directive, so
        // detection must work on a plain reachable module.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { Layout } from "../layouts/main";
                export default function Home() { return <Layout/>; }
                "#,
            )
            .with_file(
                root().join("layouts/main.tsx"),
                r#"import { ClientRouter } from "@takazudo/zfb-runtime";
                export function Layout() { return <head><ClientRouter/></head>; }
                "#,
            );

        let (islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(islands.is_empty(), "no islands expected: {islands:?}");
        assert!(
            meta.uses_client_router,
            "named ClientRouter import from the runtime barrel must be detected"
        );
    }

    #[test]
    fn client_router_detected_via_direct_subpath_import() {
        // The legacy workaround (and anyone importing the subpath
        // directly) — a side-effect import of the client-router subpath.
        let resolver = InMemoryResolver::new().with_file(
            root().join("pages/home.tsx"),
            r#"import "@takazudo/zfb-runtime/client-router";
            export default function Home() {}
            "#,
        );

        let (_islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            meta.uses_client_router,
            "direct client-router subpath import must be detected"
        );
    }

    #[test]
    fn client_router_detected_via_renamed_named_import() {
        // `import { ClientRouter as CR }` — detection keys off the
        // imported name, not the local binding.
        let resolver = InMemoryResolver::new().with_file(
            root().join("pages/home.tsx"),
            r#"import { ClientRouter as CR } from "@takazudo/zfb-runtime";
            export default function Home() { return <CR/>; }
            "#,
        );

        let (_islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            meta.uses_client_router,
            "renamed ClientRouter import must be detected on the imported name"
        );
    }

    #[test]
    fn client_router_detected_via_barrel_reexport() {
        // A consumer barrel that re-exports `ClientRouter` out of the
        // runtime — `export { ClientRouter } from "@takazudo/zfb-runtime"`.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { ClientRouter } from "../lib/barrel";
                export default function Home() { return <ClientRouter/>; }
                "#,
            )
            .with_file(
                root().join("lib/barrel.tsx"),
                r#"export { ClientRouter } from "@takazudo/zfb-runtime";
                "#,
            );

        let (_islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            meta.uses_client_router,
            "ClientRouter re-export from the runtime barrel must be detected"
        );
    }

    #[test]
    fn client_router_not_detected_when_not_imported() {
        // A project that never imports `ClientRouter` at all must report
        // `uses_client_router = false` (acceptance criterion 2 — the
        // bundler stays byte-identical).
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
                export function Counter() {}
                "#,
            );

        let (_islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            !meta.uses_client_router,
            "no ClientRouter usage → uses_client_router must be false"
        );
    }

    #[test]
    fn client_router_not_detected_for_namespace_import_of_barrel() {
        // `import * as rt from "@takazudo/zfb-runtime"` is intentionally
        // NOT a trigger — it cannot be tied to `ClientRouter` without
        // member-expression analysis, and treating it as one would risk
        // shipping the runtime to non-users.
        let resolver = InMemoryResolver::new().with_file(
            root().join("pages/home.tsx"),
            r#"import * as rt from "@takazudo/zfb-runtime";
            export default function Home() {}
            "#,
        );

        let (_islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            !meta.uses_client_router,
            "bare namespace import of the runtime barrel must not trigger detection"
        );
    }

    #[test]
    fn client_router_not_detected_for_other_named_import() {
        // Importing some OTHER named export from the runtime barrel must
        // not trigger detection.
        let resolver = InMemoryResolver::new().with_file(
            root().join("pages/home.tsx"),
            r#"import { someOtherHelper } from "@takazudo/zfb-runtime";
            export default function Home() {}
            "#,
        );

        let (_islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            !meta.uses_client_router,
            "a non-ClientRouter named import must not trigger detection"
        );
    }

    #[test]
    fn client_router_not_detected_for_type_only_import_declaration() {
        // `import type { ClientRouter } from "…"` is compile-time-erased —
        // it emits zero runtime code, so it must NOT ship the runtime
        // (acceptance criterion 2).
        let resolver = InMemoryResolver::new().with_file(
            root().join("pages/home.tsx"),
            r#"import type { ClientRouter } from "@takazudo/zfb-runtime";
            export default function Home() {}
            "#,
        );

        let (_islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            !meta.uses_client_router,
            "a type-only `import type` of ClientRouter must not trigger detection"
        );
    }

    #[test]
    fn client_router_not_detected_for_inline_type_only_specifier() {
        // `import { type ClientRouter }` — the per-specifier `type` marker
        // erases just this binding; still zero runtime code.
        let resolver = InMemoryResolver::new().with_file(
            root().join("pages/home.tsx"),
            r#"import { type ClientRouter } from "@takazudo/zfb-runtime";
            export default function Home() {}
            "#,
        );

        let (_islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            !meta.uses_client_router,
            "a per-specifier `type ClientRouter` import must not trigger detection"
        );
    }

    #[test]
    fn client_router_not_detected_for_type_only_reexport() {
        // `export type { ClientRouter } from "…"` is compile-time-erased.
        let resolver = InMemoryResolver::new().with_file(
            root().join("pages/home.tsx"),
            r#"export type { ClientRouter } from "@takazudo/zfb-runtime";
            export default function Home() {}
            "#,
        );

        let (_islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            !meta.uses_client_router,
            "a type-only `export type` re-export of ClientRouter must not trigger detection"
        );
    }

    // --- cross-module barrel re-export indirection (issue #307) ---------

    #[test]
    fn client_router_detected_via_local_barrel_reexporting_runtime() {
        // Issue #307 literal scenario: a local barrel `export *`s the
        // whole runtime barrel, and app code imports `ClientRouter` from
        // that local barrel. Neither module is a direct hit on its own.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { ClientRouter } from "../lib/runtime";
                export default function Home() { return <ClientRouter/>; }
                "#,
            )
            .with_file(
                root().join("lib/runtime.ts"),
                r#"export * from "@takazudo/zfb-runtime";
                "#,
            );

        let (_islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            meta.uses_client_router,
            "ClientRouter imported through a local barrel that `export *`s \
             the runtime barrel must be detected"
        );
    }

    #[test]
    fn client_router_not_detected_when_local_barrel_used_only_for_non_router_helper() {
        // Acceptance criterion 2 (issue #289): a project that re-exports
        // the runtime barrel through a local barrel but only imports a
        // NON-router helper from it must still ship zero new bytes.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { navigate } from "../lib/runtime";
                export default function Home() { navigate("/x"); }
                "#,
            )
            .with_file(
                root().join("lib/runtime.ts"),
                r#"export * from "@takazudo/zfb-runtime";
                "#,
            );

        let (_islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            !meta.uses_client_router,
            "importing only a non-router helper through a runtime-barrel \
             proxy must not trigger detection"
        );
    }

    #[test]
    fn client_router_detected_via_transitive_barrel_chain() {
        // A multi-hop `export *` chain: app → barrel-a → barrel-b →
        // runtime. The proxy fixpoint must follow every hop.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { ClientRouter } from "../lib/a";
                export default function Home() { return <ClientRouter/>; }
                "#,
            )
            .with_file(
                root().join("lib/a.ts"),
                r#"export * from "./b";
                "#,
            )
            .with_file(
                root().join("lib/b.ts"),
                r#"export * from "@takazudo/zfb-runtime";
                "#,
            );

        let (_islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            meta.uses_client_router,
            "ClientRouter imported through a transitive `export *` barrel \
             chain must be detected"
        );
    }

    #[test]
    fn client_router_detected_via_renamed_import_through_local_barrel() {
        // `import { ClientRouter as CR }` through a proxy — matched on the
        // imported name, not the local binding.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { ClientRouter as CR } from "../lib/runtime";
                export default function Home() { return <CR/>; }
                "#,
            )
            .with_file(
                root().join("lib/runtime.ts"),
                r#"export * from "@takazudo/zfb-runtime";
                "#,
            );

        let (_islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            meta.uses_client_router,
            "a renamed ClientRouter import through a runtime-barrel proxy \
             must be detected"
        );
    }

    #[test]
    fn client_router_detected_via_named_reexport_through_local_barrel() {
        // `export { ClientRouter } from "<proxy>"` is the export-side
        // mirror of the import form and must trigger symmetrically.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"export { ClientRouter } from "../lib/runtime";
                export default function Home() {}
                "#,
            )
            .with_file(
                root().join("lib/runtime.ts"),
                r#"export * from "@takazudo/zfb-runtime";
                "#,
            );

        let (_islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            meta.uses_client_router,
            "a named ClientRouter re-export through a runtime-barrel proxy \
             must be detected"
        );
    }

    #[test]
    fn client_router_not_detected_for_namespace_import_through_local_barrel() {
        // `import * as rt from "<proxy>"` cannot be tied to `ClientRouter`
        // — it stays a non-trigger even when the source is a proxy.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import * as rt from "../lib/runtime";
                export default function Home() {}
                "#,
            )
            .with_file(
                root().join("lib/runtime.ts"),
                r#"export * from "@takazudo/zfb-runtime";
                "#,
            );

        let (_islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            !meta.uses_client_router,
            "a namespace import of a runtime-barrel proxy must not trigger \
             detection"
        );
    }

    #[test]
    fn client_router_not_detected_for_type_only_import_through_local_barrel() {
        // `import type { ClientRouter } from "<proxy>"` is erased — zero
        // runtime code, so it must not ship the runtime.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import type { ClientRouter } from "../lib/runtime";
                export default function Home() {}
                "#,
            )
            .with_file(
                root().join("lib/runtime.ts"),
                r#"export * from "@takazudo/zfb-runtime";
                "#,
            );

        let (_islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            !meta.uses_client_router,
            "a type-only ClientRouter import through a proxy must not \
             trigger detection"
        );
    }

    #[test]
    fn client_router_not_detected_for_type_only_star_reexport_of_runtime() {
        // `export type * from "@takazudo/zfb-runtime"` is compile-time
        // erased — the local barrel is NOT a runtime-barrel proxy.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { ClientRouter } from "../lib/runtime";
                export default function Home() { return <ClientRouter/>; }
                "#,
            )
            .with_file(
                root().join("lib/runtime.ts"),
                r#"export type * from "@takazudo/zfb-runtime";
                "#,
            );

        let (_islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            !meta.uses_client_router,
            "a type-only `export type *` of the runtime barrel must not \
             make the module a proxy"
        );
    }

    #[test]
    fn client_router_detection_terminates_on_cyclic_barrels() {
        // Two barrels `export *` each other; one also re-exports the
        // runtime barrel. The proxy fixpoint must terminate on the cycle
        // and still reach the correct verdict.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { ClientRouter } from "../lib/a";
                export default function Home() { return <ClientRouter/>; }
                "#,
            )
            .with_file(
                root().join("lib/a.ts"),
                r#"export * from "./b";
                export * from "@takazudo/zfb-runtime";
                "#,
            )
            .with_file(
                root().join("lib/b.ts"),
                r#"export * from "./a";
                "#,
            );

        let (_islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            meta.uses_client_router,
            "cyclic `export *` barrels must terminate and still detect \
             ClientRouter usage"
        );
    }

    #[test]
    fn client_router_not_detected_for_local_module_with_own_client_router() {
        // A local module that declares its OWN `ClientRouter` and never
        // reaches the runtime barrel is not a proxy — importing that
        // unrelated `ClientRouter` must not ship the runtime.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { ClientRouter } from "../lib/widgets";
                export default function Home() { return <ClientRouter/>; }
                "#,
            )
            .with_file(
                root().join("lib/widgets.ts"),
                r#"export function ClientRouter() { return null; }
                "#,
            );

        let (_islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            !meta.uses_client_router,
            "importing a same-named `ClientRouter` from a module unrelated \
             to the runtime barrel must not trigger detection"
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
        // path: Bar, Foo, RenamedBaz, default.
        //
        // Issue #998: `export const Qux = 1` is a number-literal export —
        // never a component — and is now dropped from the registry rather
        // than registered as a mountable island.
        assert_eq!(
            names,
            vec![
                "Bar".to_string(),
                "Foo".to_string(),
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
                ("/proj/components/alpha.tsx".to_string(), "B".to_string()),
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

    /// `moduleResolution: "Bundler"` / NodeNext shape: import specifier
    /// uses `.js` even though the on-disk source is `.tsx`. Without the
    /// TS-family swap the scanner stops at the first such hop and every
    /// transitive `"use client"` island goes missing. See issue #143
    /// (downstream zudo-doc#1355 wave 4).
    #[test]
    fn fs_resolver_swaps_js_specifier_to_tsx_source() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let pages = dir.path().join("pages");
        let layouts = dir.path().join("layouts");
        let components = dir.path().join("components");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&layouts).unwrap();
        fs::create_dir_all(&components).unwrap();

        // Page → layout via `.js` specifier (file is `.tsx`).
        fs::write(
            pages.join("home.tsx"),
            r#"import { Layout } from "../layouts/main.js";
            export default function Home() {}
            "#,
        )
        .unwrap();
        // Layout → island via `.js` specifier (file is `.tsx`).
        fs::write(
            layouts.join("main.tsx"),
            r#"import { Counter } from "../components/counter.js";
            export function Layout() {}
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
        assert_eq!(islands.len(), 1, "got {:?}", islands);
        assert_eq!(islands[0].component_name, "Counter");
        let expected = components
            .join("counter.tsx")
            .canonicalize()
            .expect("canonicalize");
        assert_eq!(islands[0].source_path, expected);
    }

    /// Companion to the `.js` swap test above: the same TS-bundler shape
    /// also writes `.jsx`, `.mjs`, `.cjs` specifiers against `.tsx` /
    /// `.mts` / `.cts` sources. Cover each so a future refactor can't
    /// silently regress one shape.
    #[test]
    fn fs_resolver_swaps_jsx_mjs_cjs_specifiers() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let pages = dir.path().join("pages");
        let comps = dir.path().join("components");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&comps).unwrap();

        fs::write(
            pages.join("home.tsx"),
            r#"import { A } from "../components/a.jsx";
            import { B } from "../components/b.mjs";
            import { C } from "../components/c.cjs";
            export default function Home() {}
            "#,
        )
        .unwrap();
        fs::write(
            comps.join("a.tsx"),
            r#""use client"; export function A() {}"#,
        )
        .unwrap();
        fs::write(
            comps.join("b.mts"),
            r#""use client"; export function B() {}"#,
        )
        .unwrap();
        fs::write(
            comps.join("c.cts"),
            r#""use client"; export function C() {}"#,
        )
        .unwrap();

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        let names: Vec<String> = islands.iter().map(|i| i.component_name.clone()).collect();
        assert_eq!(
            names,
            vec!["A".to_string(), "B".to_string(), "C".to_string()]
        );
    }

    /// In-memory resolver mirror of [`fs_resolver_swaps_js_specifier_to_tsx_source`].
    /// Without the swap, the layout hop returns `None` and the
    /// transitive island is unreachable.
    #[test]
    fn in_memory_resolver_swaps_js_specifier_to_tsx_source() {
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { Layout } from "../layouts/main.js";
                export default function Home() {}
                "#,
            )
            .with_file(
                root().join("layouts/main.tsx"),
                r#"import { Counter } from "../components/counter.js";
                export function Layout() {}
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

    /// Specifier with a JS extension that does NOT have a TS sibling
    /// must still resolve as the literal `.js` file when present, so the
    /// swap pass cannot accidentally hide a real `.js` source.
    #[test]
    fn fs_resolver_keeps_literal_js_when_no_ts_sibling() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let pages = dir.path().join("pages");
        let comps = dir.path().join("components");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&comps).unwrap();

        fs::write(
            pages.join("home.tsx"),
            r#"import { Counter } from "../components/counter.js";
            export default function Home() {}
            "#,
        )
        .unwrap();
        fs::write(
            comps.join("counter.js"),
            r#""use client"; export function Counter() {}"#,
        )
        .unwrap();
        // No counter.ts / counter.tsx — must fall back to the literal .js.

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].component_name, "Counter");
        let expected = comps
            .join("counter.js")
            .canonicalize()
            .expect("canonicalize");
        assert_eq!(islands[0].source_path, expected);
    }

    /// TS sibling wins over an existing `.js` when both are present —
    /// matches TypeScript's lookup order. Authoring scenario: a stale
    /// build artifact left a `foo.js` next to the live `foo.tsx`; the
    /// scanner must follow the source, not the artifact.
    #[test]
    fn fs_resolver_prefers_tsx_over_js_when_both_exist() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let pages = dir.path().join("pages");
        let comps = dir.path().join("components");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&comps).unwrap();

        fs::write(
            pages.join("home.tsx"),
            r#"import { Counter } from "../components/counter.js";
            export default function Home() {}
            "#,
        )
        .unwrap();
        // Both siblings exist; the .tsx is the live source.
        fs::write(
            comps.join("counter.tsx"),
            r#""use client"; export function Counter() {}"#,
        )
        .unwrap();
        fs::write(
            comps.join("counter.js"),
            // Stale artifact — no "use client", different export name.
            r#"export function StaleArtifact() {}"#,
        )
        .unwrap();

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1, "got {:?}", islands);
        assert_eq!(islands[0].component_name, "Counter");
        let expected = comps
            .join("counter.tsx")
            .canonicalize()
            .expect("canonicalize");
        assert_eq!(islands[0].source_path, expected);
    }

    #[test]
    fn ts_swap_candidates_only_swaps_js_family_extensions() {
        // No extension → no swap.
        assert!(ts_swap_candidates(Path::new("/x/foo")).is_empty());
        // `.ts` / `.tsx` → no swap (already TS).
        assert!(ts_swap_candidates(Path::new("/x/foo.ts")).is_empty());
        assert!(ts_swap_candidates(Path::new("/x/foo.tsx")).is_empty());
        // `.json` / `.css` → no swap.
        assert!(ts_swap_candidates(Path::new("/x/foo.json")).is_empty());
        assert!(ts_swap_candidates(Path::new("/x/foo.css")).is_empty());
        // `.js` → `.ts`, `.tsx` (in that order, matching tsc).
        assert_eq!(
            ts_swap_candidates(Path::new("/x/foo.js")),
            vec![PathBuf::from("/x/foo.ts"), PathBuf::from("/x/foo.tsx")]
        );
        // `.jsx` → `.tsx`.
        assert_eq!(
            ts_swap_candidates(Path::new("/x/foo.jsx")),
            vec![PathBuf::from("/x/foo.tsx")]
        );
        // `.mjs` → `.mts`. Case-insensitive match on the input.
        assert_eq!(
            ts_swap_candidates(Path::new("/x/foo.MJS")),
            vec![PathBuf::from("/x/foo.mts")]
        );
        // `.cjs` → `.cts`.
        assert_eq!(
            ts_swap_candidates(Path::new("/x/foo.cjs")),
            vec![PathBuf::from("/x/foo.cts")]
        );
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
    /// `"use client"`. The fixture mirrors pnpm's real layout: the
    /// workspace package lives in the workspace tree (`workspace/pkg/`)
    /// and `node_modules/@scope/pkg` is a symlink pointing at it. The
    /// resolver only descends into bare specifiers when this symlink
    /// shape is in place — see codex-review on PR #125.
    /// FsResolver must walk node_modules + read package.json.source to
    /// reach the island.
    #[cfg(unix)]
    #[test]
    fn fs_resolver_resolves_pnpm_workspace_scoped_package() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let consumer = dir.path().join("consumer");
        let pages = consumer.join("pages");
        // Workspace tree (outside node_modules) — pnpm's symlink target.
        let pkg = consumer.join("workspace").join("zfb-blog-islands");
        let pkg_src = pkg.join("src");
        // Symlink that pnpm would create under node_modules.
        let pkg_link = consumer
            .join("node_modules")
            .join("@takazudo")
            .join("zfb-blog-islands");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&pkg_src).unwrap();
        make_workspace_link(&pkg, &pkg_link);

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
    /// (`@scope/pkg/components/foo`). The workspace package directory
    /// hosts a `components/foo.tsx` that the resolver reaches without
    /// consulting `package.json` exports.
    #[cfg(unix)]
    #[test]
    fn fs_resolver_resolves_pnpm_workspace_subpath_specifier() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let consumer = dir.path().join("consumer");
        let pages = consumer.join("pages");
        // Workspace tree (outside node_modules) — pnpm's symlink target.
        let pkg = consumer.join("workspace").join("zfb-blog-islands");
        let components = pkg.join("components");
        let pkg_link = consumer
            .join("node_modules")
            .join("@takazudo")
            .join("zfb-blog-islands");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&components).unwrap();
        make_workspace_link(&pkg, &pkg_link);

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
    #[cfg(unix)]
    #[test]
    fn fs_resolver_walks_ancestors_for_node_modules() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let pages = dir.path().join("apps/site/pages/blog");
        // Workspace tree (outside node_modules) — pnpm's symlink target.
        let pkg = dir.path().join("workspace").join("zfb-blog-islands");
        let pkg_src = pkg.join("src");
        let pkg_link = dir
            .path()
            .join("node_modules")
            .join("@takazudo")
            .join("zfb-blog-islands");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&pkg_src).unwrap();
        make_workspace_link(&pkg, &pkg_link);

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
    #[cfg(unix)]
    #[test]
    fn fs_resolver_falls_back_to_src_index_without_package_json_hint() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let pages = dir.path().join("pages");
        let pkg = dir.path().join("workspace").join("ws-pkg");
        let pkg_src = pkg.join("src");
        let pkg_link = dir.path().join("node_modules").join("ws-pkg");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&pkg_src).unwrap();
        make_workspace_link(&pkg, &pkg_link);

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
        let pkg_src = dir.path().join("node_modules").join("ws-pkg").join("src");
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

    /// `package.json` `"source": "src"` (a bare directory, no
    /// `index.tsx` file name in the field) must still resolve via the
    /// directory `index.<ext>` probe — same shape as a relative
    /// `import "./foo"` against a directory.
    #[cfg(unix)]
    #[test]
    fn fs_resolver_probes_directory_index_when_package_json_source_is_a_directory() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let pages = dir.path().join("pages");
        let pkg = dir.path().join("workspace").join("ws-pkg");
        let pkg_src = pkg.join("src");
        let pkg_link = dir.path().join("node_modules").join("ws-pkg");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&pkg_src).unwrap();
        make_workspace_link(&pkg, &pkg_link);

        fs::write(
            pages.join("home.tsx"),
            r#"import { Counter } from "ws-pkg";
            export default function Home() {}
            "#,
        )
        .unwrap();
        fs::write(
            pkg.join("package.json"),
            r#"{ "name": "ws-pkg", "source": "src" }"#,
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
    }

    /// Subpath specifiers that name a directory must also resolve via
    /// the directory `index.<ext>` probe (`@scope/pkg/components`
    /// pointing at `<pkg_dir>/components/index.tsx`).
    #[cfg(unix)]
    #[test]
    fn fs_resolver_probes_directory_index_for_subpath_specifier() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let pages = dir.path().join("pages");
        let pkg = dir.path().join("workspace").join("zfb-blog-islands");
        let components = pkg.join("components");
        let pkg_link = dir
            .path()
            .join("node_modules")
            .join("@takazudo")
            .join("zfb-blog-islands");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&components).unwrap();
        make_workspace_link(&pkg, &pkg_link);

        fs::write(
            pages.join("home.tsx"),
            r#"import { Counter } from "@takazudo/zfb-blog-islands/components";
            export default function Home() {}
            "#,
        )
        .unwrap();
        fs::write(
            components.join("index.tsx"),
            r#""use client";
            export function Counter() {}
            "#,
        )
        .unwrap();

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1, "got {islands:?}");
    }

    /// `package.json` `exports["./<subpath>"]` redirects a subpath
    /// import into a different on-disk shape (`src/<sub>/index.ts`) —
    /// the common pattern for un-built TypeScript workspace packages
    /// that keep source under `src/`. Without this branch the scanner
    /// would silently miss every subpath island in such a package.
    #[cfg(unix)]
    #[test]
    fn fs_resolver_honours_package_json_exports_for_subpath_specifier() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let consumer = dir.path().join("consumer");
        let pages = consumer.join("pages");
        // Workspace tree (outside node_modules) — pnpm's symlink target.
        let pkg = consumer.join("workspace").join("zudo-doc-v2");
        let pkg_src_doclayout = pkg.join("src").join("doclayout");
        // Symlink that pnpm would create under node_modules.
        let pkg_link = consumer
            .join("node_modules")
            .join("@zudo-doc")
            .join("zudo-doc-v2");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&pkg_src_doclayout).unwrap();
        make_workspace_link(&pkg, &pkg_link);

        // Page imports the subpath the legacy literal probe would
        // miss: there is no `<pkg_dir>/doclayout` on disk; the actual
        // source lives under `<pkg_dir>/src/doclayout/index.ts` and
        // is reachable only via the `exports` redirect.
        fs::write(
            pages.join("home.tsx"),
            r#"import { DocLayoutWithDefaults } from "@zudo-doc/zudo-doc-v2/doclayout";
            export default function Home() {}
            "#,
        )
        .unwrap();
        fs::write(
            pkg.join("package.json"),
            r#"{
              "name": "@zudo-doc/zudo-doc-v2",
              "exports": {
                "./doclayout": {
                  "types": "./src/doclayout/index.ts",
                  "default": "./src/doclayout/index.ts"
                }
              }
            }"#,
        )
        .unwrap();
        fs::write(
            pkg_src_doclayout.join("index.ts"),
            r#""use client";
            export function DocLayoutWithDefaults() {}
            "#,
        )
        .unwrap();

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1, "got {islands:?}");
        assert_eq!(islands[0].component_name, "DocLayoutWithDefaults");
        let expected = pkg_src_doclayout
            .join("index.ts")
            .canonicalize()
            .expect("canon");
        assert_eq!(islands[0].source_path, expected);
    }

    /// `exports["./<sub>"]` may also be a bare string (no conditional
    /// shape). The flattener returns it verbatim and the resolver
    /// reaches the redirected source.
    #[cfg(unix)]
    #[test]
    fn fs_resolver_honours_package_json_exports_string_subpath() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let consumer = dir.path().join("consumer");
        let pages = consumer.join("pages");
        let pkg = consumer.join("workspace").join("ws-pkg");
        let pkg_src_sub = pkg.join("src").join("sub");
        let pkg_link = consumer.join("node_modules").join("ws-pkg");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&pkg_src_sub).unwrap();
        make_workspace_link(&pkg, &pkg_link);

        fs::write(
            pages.join("home.tsx"),
            r#"import { Counter } from "ws-pkg/sub";
            export default function Home() {}
            "#,
        )
        .unwrap();
        fs::write(
            pkg.join("package.json"),
            r#"{
              "name": "ws-pkg",
              "exports": {
                "./sub": "./src/sub/index.ts"
              }
            }"#,
        )
        .unwrap();
        fs::write(
            pkg_src_sub.join("index.ts"),
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

    /// When a conditional `exports["./<sub>"]` entry has a `source`
    /// alongside a `default`, the `source` un-built path is preferred
    /// — workspace TypeScript packages keep that field for tools that
    /// (like the islands scanner) must read raw `.ts`/`.tsx` rather
    /// than compiled output.
    #[cfg(unix)]
    #[test]
    fn fs_resolver_prefers_source_over_default_in_exports_conditions() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let consumer = dir.path().join("consumer");
        let pages = consumer.join("pages");
        let pkg = consumer.join("workspace").join("ws-pkg");
        let pkg_src_sub = pkg.join("src").join("sub");
        let pkg_dist_sub = pkg.join("dist").join("sub");
        let pkg_link = consumer.join("node_modules").join("ws-pkg");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&pkg_src_sub).unwrap();
        fs::create_dir_all(&pkg_dist_sub).unwrap();
        make_workspace_link(&pkg, &pkg_link);

        fs::write(
            pages.join("home.tsx"),
            r#"import { Counter } from "ws-pkg/sub";
            export default function Home() {}
            "#,
        )
        .unwrap();
        fs::write(
            pkg.join("package.json"),
            r#"{
              "name": "ws-pkg",
              "exports": {
                "./sub": {
                  "source": "./src/sub/index.ts",
                  "default": "./dist/sub/index.js"
                }
              }
            }"#,
        )
        .unwrap();
        // Both targets exist on disk and both carry "use client" — if
        // the resolver picked `default`, the test would still find an
        // island, just at the wrong path. Assert the source path.
        fs::write(
            pkg_src_sub.join("index.ts"),
            r#""use client";
            export function Counter() {}
            "#,
        )
        .unwrap();
        fs::write(
            pkg_dist_sub.join("index.js"),
            r#""use client";
            export function Counter() {}
            "#,
        )
        .unwrap();

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1, "got {islands:?}");
        let expected = pkg_src_sub.join("index.ts").canonicalize().expect("canon");
        assert_eq!(
            islands[0].source_path, expected,
            "expected src/ path (source condition), got {:?}",
            islands[0].source_path,
        );
    }

    /// The literal-subpath probe still wins when the on-disk shape
    /// matches the import shape — the `exports` lookup is a fallback,
    /// not a replacement for the legacy behaviour.
    #[cfg(unix)]
    #[test]
    fn fs_resolver_subpath_literal_probe_wins_over_exports_when_both_resolve() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let consumer = dir.path().join("consumer");
        let pages = consumer.join("pages");
        let pkg = consumer.join("workspace").join("ws-pkg");
        // Two candidate locations: `<pkg>/components/foo.tsx` (literal)
        // and `<pkg>/src/components/foo.ts` (via exports). The literal
        // probe runs first and must win.
        let components = pkg.join("components");
        let pkg_src_components = pkg.join("src").join("components");
        let pkg_link = consumer.join("node_modules").join("ws-pkg");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&components).unwrap();
        fs::create_dir_all(&pkg_src_components).unwrap();
        make_workspace_link(&pkg, &pkg_link);

        fs::write(
            pages.join("home.tsx"),
            r#"import { Foo } from "ws-pkg/components/foo";
            export default function Home() {}
            "#,
        )
        .unwrap();
        fs::write(
            pkg.join("package.json"),
            r#"{
              "name": "ws-pkg",
              "exports": {
                "./components/foo": "./src/components/foo.ts"
              }
            }"#,
        )
        .unwrap();
        // Literal target — must be the one the scanner picks.
        fs::write(
            components.join("foo.tsx"),
            r#""use client";
            export function Foo() {}
            "#,
        )
        .unwrap();
        // Exports-redirected target — also has "use client", to make
        // sure the assertion catches a regression where both surface.
        fs::write(
            pkg_src_components.join("foo.ts"),
            r#""use client";
            export function Foo() {}
            "#,
        )
        .unwrap();

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1, "got {islands:?}");
        let literal = components.join("foo.tsx").canonicalize().expect("canon");
        assert_eq!(islands[0].source_path, literal);
    }

    #[test]
    fn resolve_exports_subpath_handles_string_entry() {
        let json: serde_json::Value =
            serde_json::from_str(r#"{ "exports": { "./sub": "./src/sub/index.ts" } }"#).unwrap();
        assert_eq!(
            resolve_exports_subpath(&json, "sub"),
            Some("./src/sub/index.ts".to_string())
        );
    }

    #[test]
    fn resolve_exports_subpath_prefers_source_then_default_then_import() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{ "exports": { "./a": { "source": "S", "default": "D", "import": "I" } } }"#,
        )
        .unwrap();
        assert_eq!(resolve_exports_subpath(&json, "a"), Some("S".to_string()));

        let json: serde_json::Value =
            serde_json::from_str(r#"{ "exports": { "./a": { "default": "D", "import": "I" } } }"#)
                .unwrap();
        assert_eq!(resolve_exports_subpath(&json, "a"), Some("D".to_string()));

        let json: serde_json::Value =
            serde_json::from_str(r#"{ "exports": { "./a": { "import": "I" } } }"#).unwrap();
        assert_eq!(resolve_exports_subpath(&json, "a"), Some("I".to_string()));
    }

    #[test]
    fn resolve_exports_subpath_returns_none_when_key_missing() {
        let json: serde_json::Value =
            serde_json::from_str(r#"{ "exports": { "./other": "./other.ts" } }"#).unwrap();
        assert_eq!(resolve_exports_subpath(&json, "missing"), None);
    }

    #[test]
    fn resolve_exports_subpath_returns_none_when_no_exports_field() {
        let json: serde_json::Value = serde_json::from_str(r#"{ "name": "foo" }"#).unwrap();
        assert_eq!(resolve_exports_subpath(&json, "anything"), None);
    }

    #[test]
    fn resolve_exports_dot_reads_subpath_map_dot_entry() {
        // The real `@takazudo/zudo-doc` shape: a subpath map whose "."
        // entry is a `{ types, default }` conditional object.
        let json: serde_json::Value = serde_json::from_str(
            r#"{ "exports": { ".": { "types": "./dist/index.d.ts", "default": "./dist/index.js" },
                              "./sub": { "default": "./dist/sub/index.js" } } }"#,
        )
        .unwrap();
        assert_eq!(
            resolve_exports_dot(&json),
            Some("./dist/index.js".to_string())
        );
    }

    #[test]
    fn resolve_exports_dot_handles_shorthand_string_and_array() {
        let string_form: serde_json::Value =
            serde_json::from_str(r#"{ "exports": "./index.js" }"#).unwrap();
        assert_eq!(
            resolve_exports_dot(&string_form),
            Some("./index.js".to_string())
        );

        let array_form: serde_json::Value =
            serde_json::from_str(r#"{ "exports": ["./index.mjs", "./index.js"] }"#).unwrap();
        assert_eq!(
            resolve_exports_dot(&array_form),
            Some("./index.mjs".to_string())
        );
    }

    #[test]
    fn resolve_exports_dot_handles_dot_condition_object_without_subpath_keys() {
        // `{ "import": …, "default": … }` with no "./"-prefixed keys IS the
        // dot export's condition set — flatten it directly.
        let json: serde_json::Value = serde_json::from_str(
            r#"{ "exports": { "import": "./esm/index.js", "require": "./cjs/index.cjs" } }"#,
        )
        .unwrap();
        assert_eq!(
            resolve_exports_dot(&json),
            Some("./esm/index.js".to_string())
        );
    }

    #[test]
    fn resolve_exports_dot_returns_none_for_subpath_map_without_dot() {
        // A subpath map that does not declare "." has no top-level entry —
        // the caller falls back to `module`/`main`.
        let json: serde_json::Value =
            serde_json::from_str(r#"{ "exports": { "./sub": "./dist/sub.js" } }"#).unwrap();
        assert_eq!(resolve_exports_dot(&json), None);
    }

    #[test]
    fn resolve_exports_dot_returns_none_when_no_exports_field() {
        let json: serde_json::Value =
            serde_json::from_str(r#"{ "name": "foo", "main": "./index.js" }"#).unwrap();
        assert_eq!(resolve_exports_dot(&json), None);
    }

    /// Hostile `package.json` `"source": "../../escape.tsx"` must NOT
    /// resolve outside the package directory — the workspace probe
    /// clamps every resolved path inside `node_modules/<pkg>/`.
    #[cfg(unix)]
    #[test]
    fn fs_resolver_rejects_package_json_source_that_escapes_pkg_dir() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let pages = dir.path().join("pages");
        let pkg = dir.path().join("workspace").join("ws-pkg");
        let pkg_link = dir.path().join("node_modules").join("ws-pkg");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&pkg).unwrap();
        make_workspace_link(&pkg, &pkg_link);

        // The escape target sits OUTSIDE the workspace package
        // directory, in the project root — it has "use client" so if
        // the clamp ever failed, the scanner would surface it as an
        // island.
        fs::write(
            dir.path().join("escape.tsx"),
            r#""use client";
            export function Pwn() {}
            "#,
        )
        .unwrap();
        fs::write(
            pages.join("home.tsx"),
            r#"import { Pwn } from "ws-pkg";
            export default function Home() {}
            "#,
        )
        .unwrap();
        fs::write(
            pkg.join("package.json"),
            r#"{ "name": "ws-pkg", "source": "../../escape.tsx" }"#,
        )
        .unwrap();

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        assert!(
            islands.is_empty(),
            "expected escape attempt to be clamped; got {islands:?}",
        );
    }

    /// Hostile `package.json` `exports["./<sub>"]` pointing outside
    /// the package directory must NOT resolve — same `is_inside`
    /// clamp the legacy `source`/`module`/`main` reads use applies to
    /// the new exports path.
    #[cfg(unix)]
    #[test]
    fn fs_resolver_rejects_package_json_exports_that_escapes_pkg_dir() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let pages = dir.path().join("pages");
        let pkg = dir.path().join("workspace").join("ws-pkg");
        let pkg_link = dir.path().join("node_modules").join("ws-pkg");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&pkg).unwrap();
        make_workspace_link(&pkg, &pkg_link);

        fs::write(
            dir.path().join("escape.tsx"),
            r#""use client";
            export function Pwn() {}
            "#,
        )
        .unwrap();
        fs::write(
            pages.join("home.tsx"),
            r#"import { Pwn } from "ws-pkg/sub";
            export default function Home() {}
            "#,
        )
        .unwrap();
        fs::write(
            pkg.join("package.json"),
            r#"{
              "name": "ws-pkg",
              "exports": { "./sub": "../../escape.tsx" }
            }"#,
        )
        .unwrap();

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        assert!(
            islands.is_empty(),
            "expected exports escape attempt to be clamped; got {islands:?}",
        );
    }

    #[test]
    fn split_bare_specifier_handles_scoped_and_unscoped() {
        let scoped = FsResolver::split_bare_specifier("@takazudo/zfb-blog-islands");
        assert_eq!(
            scoped,
            ("@takazudo/zfb-blog-islands".to_string(), String::new())
        );

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

    /// Headline regression test descended from codex-review on PR #125,
    /// updated for issue #999. A project with a real `node_modules/preact`
    /// (a regular installed dependency, NOT a workspace symlink) AND a
    /// workspace package linked under `node_modules/<pkg>` must:
    ///
    /// (a) discover the workspace package's `"use client"` islands via the
    ///     bare-specifier probe, and
    /// (b) NOT crawl the framework's transitive *bare-dependency* graph —
    ///     a `preact/hooks` module that pulls preact core via a bare
    ///     `import "preact"` must stop at that bare import, so an island
    ///     hiding in preact core never surfaces.
    ///
    /// #999 changed the rule: a bare import *from project source* now does
    /// enter a regular npm package (that is how npm-dist islands get
    /// registered), so the directly-imported `preact/hooks` module IS
    /// scanned — but because it carries no directive it contributes no
    /// island, and its own bare `import "preact"` (made from INSIDE
    /// `node_modules`) is not followed. The net effect is unchanged from
    /// the #125 guarantee: the framework's dependency graph is never walked.
    #[cfg(unix)]
    #[test]
    fn fs_resolver_enters_pkg_from_project_source_but_never_crawls_bare_dep_graph() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let pages = dir.path().join("pages");

        // (1) Real `node_modules/preact/` directory — NOT a symlink.
        // Mimics a regular installed npm package.
        let preact = dir.path().join("node_modules").join("preact");
        let preact_dist = preact.join("dist");
        fs::create_dir_all(&preact_dist).unwrap();
        fs::write(
            preact.join("package.json"),
            r#"{ "name": "preact", "main": "dist/preact.js" }"#,
        )
        .unwrap();
        // preact CORE carries a sneaky island. It is reachable ONLY via a
        // bare `import "preact"` made from inside `preact/hooks` (below) —
        // a bare import from inside node_modules, which the scanner must
        // NOT follow. The test asserts this island never surfaces.
        fs::write(
            preact_dist.join("preact.js"),
            r#""use client";
            export function PreactSneaksIn() {}
            "#,
        )
        .unwrap();
        // The directly-imported `preact/hooks` subpath: a realistic
        // framework module with NO directive that pulls preact core via a
        // bare specifier. It IS scanned now (project source imports it),
        // but contributes no island, and its bare `import "preact"` is the
        // edge that must not be crawled.
        let preact_hooks = preact.join("hooks");
        fs::create_dir_all(&preact_hooks).unwrap();
        fs::write(
            preact_hooks.join("index.js"),
            r#"import { options } from "preact";
            export function useState() {}
            "#,
        )
        .unwrap();

        // (2) Workspace package linked from node_modules/<pkg>.
        let pkg = dir.path().join("workspace").join("ws-pkg");
        let pkg_src = pkg.join("src");
        let pkg_link = dir.path().join("node_modules").join("ws-pkg");
        fs::create_dir_all(&pkg_src).unwrap();
        make_workspace_link(&pkg, &pkg_link);
        fs::write(
            pkg.join("package.json"),
            r#"{ "name": "ws-pkg", "source": "src/index.tsx" }"#,
        )
        .unwrap();
        fs::write(
            pkg_src.join("index.tsx"),
            r#""use client";
            export function WorkspaceCounter() {}
            "#,
        )
        .unwrap();

        // (3) Page imports both: a workspace island via its package
        // name, AND framework hooks via `preact/hooks` (the case the
        // codex-review finding was about).
        fs::create_dir_all(&pages).unwrap();
        fs::write(
            pages.join("home.tsx"),
            r#"import { useState } from "preact/hooks";
            import { WorkspaceCounter } from "ws-pkg";
            export default function Home() {}
            "#,
        )
        .unwrap();

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        // Only the workspace island surfaces. `preact/hooks` is scanned
        // (no island) but its bare `import "preact"` is not followed, so
        // `PreactSneaksIn` in preact core never appears.
        let names: Vec<&str> = islands.iter().map(|i| i.component_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["WorkspaceCounter"],
            "expected only the workspace island; preact core must not be crawled: {islands:?}",
        );
    }

    /// #999: `exports["."]` must take precedence over the legacy `main`
    /// field for a top-level (`import "@scope/pkg"`) regular-package entry —
    /// Node ignores `main` once `exports` is present, and the real
    /// `@takazudo/zudo-doc` ships only `exports`. Pin that `exports` wins
    /// when BOTH are present, so a future reorder of the two probe blocks
    /// is caught.
    #[cfg(unix)]
    #[test]
    fn fs_resolver_prefers_exports_dot_over_main_for_regular_package() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();

        // Regular npm package (a real directory under node_modules) with
        // BOTH `exports["."]` and `main` — pointing at DIFFERENT files,
        // each carrying a distinct island.
        let pkg = root.join("node_modules").join("@acme").join("widgets");
        fs::create_dir_all(pkg.join("esm")).unwrap();
        fs::create_dir_all(pkg.join("legacy")).unwrap();
        fs::write(
            pkg.join("package.json"),
            r#"{ "name": "@acme/widgets", "type": "module",
                "main": "./legacy/index.js",
                "exports": { ".": { "default": "./esm/index.js" } } }"#,
        )
        .unwrap();
        fs::write(
            pkg.join("esm/index.js"),
            "\"use client\";\nexport function FromExports() {}\n",
        )
        .unwrap();
        fs::write(
            pkg.join("legacy/index.js"),
            "\"use client\";\nexport function FromMain() {}\n",
        )
        .unwrap();

        let pages = root.join("pages");
        fs::create_dir_all(&pages).unwrap();
        fs::write(
            pages.join("home.tsx"),
            "import { FromExports } from \"@acme/widgets\";\nexport default function Home() {}\n",
        )
        .unwrap();

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        let names: Vec<&str> = islands.iter().map(|i| i.component_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["FromExports"],
            "exports[\".\"] must win over main: {islands:?}",
        );
    }

    /// #999 boundary invariant, from a WORKSPACE-package importer: even
    /// though a workspace package's source lives outside `node_modules` (so
    /// the regular-package descent relaxation applies to its bare imports
    /// too — see the `resolve` policy comment), the hard guarantee still
    /// holds: a bare import made from INSIDE a regular package's dist is
    /// never followed. So a workspace package importing a regular package
    /// scans that package's own entry but does NOT crawl the regular
    /// package's transitive bare-dependency graph. This is the part of the
    /// PR #125 guarantee that survives #999 unconditionally.
    #[cfg(unix)]
    #[test]
    fn workspace_source_entering_regular_pkg_does_not_crawl_its_bare_dep_graph() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();

        // Workspace package (symlinked into node_modules, source outside it)
        // whose `"use client"` source imports a regular npm package.
        let ws = root.join("workspace").join("ws-pkg");
        let ws_src = ws.join("src");
        fs::create_dir_all(&ws_src).unwrap();
        make_workspace_link(&ws, &root.join("node_modules").join("ws-pkg"));
        fs::write(
            ws.join("package.json"),
            r#"{ "name": "ws-pkg", "source": "src/index.tsx" }"#,
        )
        .unwrap();
        fs::write(
            ws_src.join("index.tsx"),
            "\"use client\";\nimport { Widget } from \"@acme/widgets\";\nexport function WsIsland() {}\n",
        )
        .unwrap();

        // Regular npm package: its entry has an island AND a bare import of
        // a peer dependency whose own module carries a sneaky island. The
        // entry's island IS registered (directly imported); the peer's must
        // NOT be (reached only via a bare import from inside node_modules).
        let widgets = root.join("node_modules").join("@acme").join("widgets");
        fs::create_dir_all(widgets.join("dist")).unwrap();
        fs::write(
            widgets.join("package.json"),
            r#"{ "name": "@acme/widgets", "type": "module",
                "exports": { ".": { "default": "./dist/index.js" } } }"#,
        )
        .unwrap();
        fs::write(
            widgets.join("dist/index.js"),
            "\"use client\";\nimport { x } from \"peer-dep\";\nexport function Widget() {}\n",
        )
        .unwrap();

        let peer = root.join("node_modules").join("peer-dep");
        fs::create_dir_all(&peer).unwrap();
        fs::write(
            peer.join("package.json"),
            r#"{ "name": "peer-dep", "type": "module", "exports": { ".": { "default": "./index.js" } } }"#,
        )
        .unwrap();
        fs::write(
            peer.join("index.js"),
            "\"use client\";\nexport function PeerSneaksIn() {}\n",
        )
        .unwrap();

        let pages = root.join("pages");
        fs::create_dir_all(&pages).unwrap();
        fs::write(
            pages.join("home.tsx"),
            "import { WsIsland } from \"ws-pkg\";\nexport default function Home() {}\n",
        )
        .unwrap();

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        let mut names: Vec<&str> = islands.iter().map(|i| i.component_name.as_str()).collect();
        names.sort_unstable();
        // The workspace island and the directly-imported regular-package
        // entry island both register; the peer dependency's island, reached
        // only via a bare import from inside @acme/widgets/dist, must not.
        assert_eq!(
            names,
            vec!["Widget", "WsIsland"],
            "peer-dep reached via bare import from inside node_modules must not be crawled: {islands:?}",
        );
    }

    // ---------------------------------------------------------------
    // tsconfig.json `compilerOptions.paths` (issue #139)
    // ---------------------------------------------------------------

    #[test]
    fn match_alias_pattern_literal_match() {
        assert_eq!(match_alias_pattern("@foo/bar", "@foo/bar"), Some(None));
    }

    #[test]
    fn match_alias_pattern_literal_no_match() {
        assert_eq!(match_alias_pattern("@foo/bar", "@foo/baz"), None);
    }

    #[test]
    fn match_alias_pattern_wildcard_captures_suffix() {
        assert_eq!(
            match_alias_pattern("@/*", "@/components/foo"),
            Some(Some("components/foo".to_string()))
        );
    }

    #[test]
    fn match_alias_pattern_wildcard_must_match_prefix_and_suffix() {
        assert_eq!(
            match_alias_pattern("lib/*/index", "lib/foo/index"),
            Some(Some("foo".to_string()))
        );
        // Suffix mismatch → no match.
        assert_eq!(match_alias_pattern("lib/*/index", "lib/foo/other"), None);
    }

    #[test]
    fn match_alias_pattern_rejects_multi_wildcards() {
        // We don't support patterns with more than one `*`.
        assert_eq!(match_alias_pattern("a/*/b/*", "a/x/b/y"), None);
    }

    #[test]
    fn substitute_alias_target_wildcard_splices_captured_segment() {
        assert_eq!(
            substitute_alias_target("src/*", Some("components/foo")),
            "src/components/foo"
        );
        assert_eq!(
            substitute_alias_target("vendored/*/index.ts", Some("foo")),
            "vendored/foo/index.ts"
        );
    }

    #[test]
    fn substitute_alias_target_literal_passes_through() {
        assert_eq!(
            substitute_alias_target("src/index.ts", None),
            "src/index.ts"
        );
    }

    /// End-to-end: a project with `"@/*": ["src/*"]` in tsconfig.json
    /// imports `@/components/counter`. The resolver must discover
    /// tsconfig, match the alias, substitute, probe the file system,
    /// and return the resolved island file. This is the exact downstream
    /// shape that motivated issue #139 (zudo-doc Wave 4).
    #[test]
    fn fs_resolver_resolves_tsconfig_path_alias_in_tempdir() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path();
        let pages = project.join("pages");
        let components = project.join("src").join("components");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&components).unwrap();

        fs::write(
            project.join("tsconfig.json"),
            r#"{
              "compilerOptions": {
                "paths": {
                  "@/*": ["src/*"]
                }
              }
            }"#,
        )
        .unwrap();
        fs::write(
            pages.join("home.tsx"),
            r#"import { Counter } from "@/components/counter";
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
        let expected = components
            .join("counter.tsx")
            .canonicalize()
            .expect("canon");
        assert_eq!(islands[0].source_path, expected);
    }

    /// `compilerOptions.baseUrl` shifts the substitution target:
    /// `"baseUrl": "./src", "paths": { "@/*": ["./*"] }` resolves
    /// `@/foo` to `<project>/src/foo`. Same end state as the
    /// `["src/*"]` shape above; this exercises the explicit baseUrl
    /// path so the join math stays right when `baseUrl` is non-default.
    #[test]
    fn fs_resolver_honours_explicit_base_url() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path();
        let pages = project.join("pages");
        let components = project.join("src").join("components");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&components).unwrap();

        fs::write(
            project.join("tsconfig.json"),
            r#"{
              "compilerOptions": {
                "baseUrl": "./src",
                "paths": {
                  "@/*": ["./*"]
                }
              }
            }"#,
        )
        .unwrap();
        fs::write(
            pages.join("home.tsx"),
            r#"import { Counter } from "@/components/counter";
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

    /// Multiple substitution targets are tried in order. The first
    /// hit wins; misses fall through. Mirrors the TypeScript spec's
    /// "fallback list" behaviour.
    #[test]
    fn fs_resolver_falls_through_alias_targets_in_order() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path();
        let pages = project.join("pages");
        // First target points at a non-existent dir; second points at
        // the real one. The resolver must skip the miss and try the
        // next target rather than giving up after the first.
        let components = project.join("real").join("components");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&components).unwrap();

        fs::write(
            project.join("tsconfig.json"),
            r#"{
              "compilerOptions": {
                "paths": {
                  "@/*": ["does-not-exist/*", "real/*"]
                }
              }
            }"#,
        )
        .unwrap();
        fs::write(
            pages.join("home.tsx"),
            r#"import { Counter } from "@/components/counter";
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
        assert_eq!(
            islands.len(),
            1,
            "expected fallback target to win: {islands:?}"
        );
    }

    /// A literal alias (no `*` in the pattern) maps the whole
    /// specifier to a single file. Validates the non-wildcard branch
    /// of `match_alias_pattern` end-to-end.
    #[test]
    fn fs_resolver_resolves_literal_alias_pattern() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path();
        let pages = project.join("pages");
        let lib = project.join("src").join("lib");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&lib).unwrap();

        fs::write(
            project.join("tsconfig.json"),
            r#"{
              "compilerOptions": {
                "paths": {
                  "~theme": ["src/lib/theme.tsx"]
                }
              }
            }"#,
        )
        .unwrap();
        fs::write(
            pages.join("home.tsx"),
            r#"import { Theme } from "~theme";
            export default function Home() {}
            "#,
        )
        .unwrap();
        fs::write(
            lib.join("theme.tsx"),
            r#""use client";
            export function Theme() {}
            "#,
        )
        .unwrap();

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1, "got {islands:?}");
        assert_eq!(islands[0].component_name, "Theme");
    }

    /// Tsconfig with no matching pattern → resolver falls through to
    /// the existing bare-specifier / workspace-probe path. Specifier
    /// resolves to nothing (no node_modules entry exists) and the
    /// scanner skips it cleanly without panicking.
    #[test]
    fn fs_resolver_skips_specifier_when_no_alias_matches() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path();
        let pages = project.join("pages");
        fs::create_dir_all(&pages).unwrap();

        fs::write(
            project.join("tsconfig.json"),
            r#"{
              "compilerOptions": {
                "paths": {
                  "@/*": ["src/*"]
                }
              }
            }"#,
        )
        .unwrap();
        fs::write(
            pages.join("home.tsx"),
            // Bare specifier that is NOT covered by `@/*` — falls
            // through to the bare-specifier branch, hits no
            // node_modules entry, returns None.
            r#"import { Foo } from "preact";
            export default function Home() {}
            "#,
        )
        .unwrap();

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        assert!(islands.is_empty(), "got {islands:?}");
    }

    /// Alias target whose substitution does not exist on disk → the
    /// resolver returns `None` cleanly (no panic, no spurious island).
    /// Mirrors the existing relative-import branch: the resolver does
    /// not invent files, it only follows what's actually on disk.
    /// (The `probe_package_entry` clamp inside `is_inside` only
    /// applies to `node_modules/<pkg>` entries; alias resolution
    /// inherits the relative-path branch's freedom to follow `..`,
    /// just as a hand-written `import "../../something"` would.)
    #[test]
    fn fs_resolver_alias_substitution_miss_returns_no_island() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path();
        let pages = project.join("pages");
        fs::create_dir_all(&pages).unwrap();

        fs::write(
            project.join("tsconfig.json"),
            r#"{
              "compilerOptions": {
                "paths": {
                  "@/*": ["src/*"]
                }
              }
            }"#,
        )
        .unwrap();
        fs::write(
            pages.join("home.tsx"),
            // `@/components/missing` substitutes to
            // `<project>/src/components/missing.{tsx,ts,…}` — no such
            // file exists, so the resolver returns None and the
            // scanner produces no islands for this specifier.
            r#"import { Missing } from "@/components/missing";
            export default function Home() {}
            "#,
        )
        .unwrap();

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        assert!(islands.is_empty(), "got {islands:?}");
    }

    /// `extends` chain is followed: the leaf has no `paths`, but its
    /// `extends` parent does. Models the common Astro / Next.js
    /// scaffold where a `tsconfig.json` extends a `tsconfig.base.json`
    /// (same directory) only — note the extends-target file MUST be
    /// named `tsconfig.json` for the current parser to follow it (see
    /// `resolve_extends_target` doc).
    #[test]
    fn fs_resolver_follows_extends_chain_to_inherit_paths() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path();
        let base_dir = project.join("config");
        let pages = project.join("pages");
        let components = project.join("src").join("components");
        fs::create_dir_all(&base_dir).unwrap();
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&components).unwrap();

        // Base config in a sibling directory: it owns `paths`. The
        // base's `compilerOptions.baseUrl` is the BASE's directory
        // (`config/`), but absent here, so it defaults to that
        // directory. To resolve `src/*` correctly the base also sets
        // `baseUrl` two levels up via "../".
        fs::write(
            base_dir.join("tsconfig.json"),
            r#"{
              "compilerOptions": {
                "baseUrl": "..",
                "paths": {
                  "@/*": ["src/*"]
                }
              }
            }"#,
        )
        .unwrap();
        // Leaf config extends the base via a relative path.
        fs::write(
            project.join("tsconfig.json"),
            r#"{
              "extends": "./config/tsconfig.json"
            }"#,
        )
        .unwrap();
        fs::write(
            pages.join("home.tsx"),
            r#"import { Counter } from "@/components/counter";
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

    #[test]
    fn pattern_specificity_literal_beats_wildcard() {
        // A literal pattern (no `*`) of length 12 outranks a wildcard
        // whose longest fixed run is shorter. This is the dominant
        // tiebreaker used by `try_resolve_tsconfig_alias`.
        assert!(pattern_specificity("@theme/dark") > pattern_specificity("@theme/*"));
    }

    #[test]
    fn pattern_specificity_longer_prefix_wins() {
        // Both patterns end in `*`; the one with the longer fixed
        // prefix is more specific. With the codex P1 finding's
        // example, `@/components/*` (specificity 13) must outrank
        // `@/*` (specificity 2).
        assert!(pattern_specificity("@/components/*") > pattern_specificity("@/*"));
    }

    /// Codex P1 regression: with overlapping patterns, the more
    /// specific one must win regardless of declaration order in the
    /// tsconfig. A specifier of `@/components/Button` previously
    /// resolved through the broader `@/*` rule because `try_resolve`
    /// returned on first map-iteration hit; now it must consult the
    /// narrower `@/components/*` rule first.
    #[test]
    fn fs_resolver_prefers_more_specific_alias_pattern() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path();
        let pages = project.join("pages");
        // The broader `@/*` rule's substitution exists on disk too
        // (`src/components/Button.tsx`) and would yield a different
        // island name without the specificity rule. The narrower
        // `@/components/*` rule maps to `ui/Button.tsx`. The test
        // asserts the narrower rule wins.
        let src_components = project.join("src").join("components");
        let ui = project.join("ui");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&src_components).unwrap();
        fs::create_dir_all(&ui).unwrap();

        fs::write(
            project.join("tsconfig.json"),
            r#"{
              "compilerOptions": {
                "paths": {
                  "@/*": ["src/*"],
                  "@/components/*": ["ui/*"]
                }
              }
            }"#,
        )
        .unwrap();
        fs::write(
            pages.join("home.tsx"),
            r#"import { Narrow } from "@/components/button";
            export default function Home() {}
            "#,
        )
        .unwrap();
        // Broader `@/*` substitutes to `src/components/button` —
        // exists, but should NOT be the winner.
        fs::write(
            src_components.join("button.tsx"),
            r#""use client";
            export function Wide() {}
            "#,
        )
        .unwrap();
        // Narrower `@/components/*` substitutes to `ui/button` —
        // this is what the resolver MUST pick.
        fs::write(
            ui.join("button.tsx"),
            r#""use client";
            export function Narrow() {}
            "#,
        )
        .unwrap();

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        // The narrower rule picked ui/button.tsx → component Narrow.
        // If the broader rule had won, `Wide` would be in the set
        // instead.
        let names: Vec<&str> = islands.iter().map(|i| i.component_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Narrow"],
            "more-specific @/components/* must win over @/*: {islands:?}"
        );
    }

    /// Codex P2 regression: leaf tsconfig declares `paths` but no
    /// `baseUrl`; extends a parent that also lacks `baseUrl`. The
    /// implicit baseUrl must anchor to the LEAF tsconfig's directory
    /// (the one that declared `paths`), not the parent extends
    /// target's directory. Before the fix, the walker fell through
    /// to the parent's `dir` and resolved `@/*` against the wrong
    /// project root.
    #[test]
    fn fs_resolver_extends_implicit_baseurl_anchors_to_leaf() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path();
        // The parent base config lives in a sibling dir. It has NO
        // `paths`, NO `baseUrl` — just unrelated compiler options.
        let parent_config_dir = project.join("shared");
        let pages = project.join("pages");
        let leaf_components = project.join("src").join("components");
        // Notably, `shared/src/components/` does NOT exist — if the
        // walker mistakenly anchored to `shared/`, the probe would
        // miss and the test would fail.
        fs::create_dir_all(&parent_config_dir).unwrap();
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&leaf_components).unwrap();

        fs::write(
            parent_config_dir.join("tsconfig.json"),
            r#"{
              "compilerOptions": {
                "strict": true
              }
            }"#,
        )
        .unwrap();
        fs::write(
            project.join("tsconfig.json"),
            r#"{
              "extends": "./shared/tsconfig.json",
              "compilerOptions": {
                "paths": { "@/*": ["src/*"] }
              }
            }"#,
        )
        .unwrap();
        fs::write(
            pages.join("home.tsx"),
            r#"import { Counter } from "@/components/counter";
            export default function Home() {}
            "#,
        )
        .unwrap();
        fs::write(
            leaf_components.join("counter.tsx"),
            r#""use client";
            export function Counter() {}
            "#,
        )
        .unwrap();

        let resolver = FsResolver::new();
        let islands = scan_islands(&[pages.join("home.tsx")], &resolver).unwrap();
        assert_eq!(
            islands.len(),
            1,
            "leaf-anchored paths must resolve against the leaf tsconfig dir: {islands:?}"
        );
        assert_eq!(islands[0].component_name, "Counter");
    }

    /// Cache hit: parsing the same tsconfig twice via `load_tsconfig_paths`
    /// returns the same data, and the on-disk file change between the
    /// two calls is NOT reflected (the cache is intentionally write-once
    /// per resolver instance — the build is a single short-lived pass,
    /// and we don't want IO churn).
    #[test]
    fn fs_resolver_caches_parsed_tsconfig() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path();
        fs::write(
            project.join("tsconfig.json"),
            r#"{
              "compilerOptions": {
                "paths": { "@/*": ["src/*"] }
              }
            }"#,
        )
        .unwrap();

        let resolver = FsResolver::new();
        let first = resolver.load_tsconfig_paths(project).expect("first load");
        // Mutate the on-disk file so a re-read would change the
        // result — and verify the cached entry is returned unchanged.
        fs::write(
            project.join("tsconfig.json"),
            r#"{
              "compilerOptions": {
                "paths": { "@/*": ["completely/different/*"] }
              }
            }"#,
        )
        .unwrap();
        let second = resolver
            .load_tsconfig_paths(project)
            .expect("second load (cache hit)");
        assert_eq!(first.aliases.len(), second.aliases.len());
        assert_eq!(first.aliases[0].targets, second.aliases[0].targets);
    }

    // -------------------------------------------------------------------
    // JSONC regression test (issue #901)
    // -------------------------------------------------------------------
    //
    // The old `parse_tsconfig_paths` in scanner.rs used `serde_json`
    // directly without stripping JSONC comments first. A tsconfig that
    // contains `//` line comments or `/* */` block comments would fail to
    // parse and silently produce no aliases, breaking island alias resolution.
    // The shared helper in `zfb_plugin_resolver::read_tsconfig_paths` strips
    // JSONC before parsing; this test is the regression guard.

    /// A `tsconfig.json` with `//` line comments and a trailing comma
    /// must still resolve island aliases correctly through the scanner.
    ///
    /// Before the #901 fix this test would fail because the scanner's
    /// `parse_tsconfig_paths` did not strip JSONC artefacts.
    #[test]
    fn fs_resolver_handles_jsonc_annotated_tsconfig() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path();
        let pages = project.join("pages");
        let components = project.join("src").join("components");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&components).unwrap();

        // tsconfig.json with JSONC artefacts: line comments, block comment,
        // and a trailing comma after the last paths entry.
        fs::write(
            project.join("tsconfig.json"),
            r#"{
              // JSONC-style comment that serde_json cannot parse as-is
              "compilerOptions": {
                "paths": {
                  "@/*": ["src/*"], // trailing inline comment
                  "~/*": ["lib/*"]  /* block comment */
                }
              }
            }"#,
        )
        .unwrap();
        fs::write(
            pages.join("home.tsx"),
            r#"import { Counter } from "@/components/counter";
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
        assert_eq!(
            islands.len(),
            1,
            "JSONC-annotated tsconfig must resolve aliases; got {islands:?}"
        );
        assert_eq!(islands[0].component_name, "Counter");
    }

    // -------------------------------------------------------------------
    // Non-source extension gate (issue #141)
    // -------------------------------------------------------------------

    /// `is_scannable_source` accepts the TS/JS family extensions and
    /// rejects everything else.
    #[test]
    fn is_scannable_source_accepts_ts_and_js_family() {
        for ext in ["ts", "tsx", "js", "jsx", "mjs", "cjs"] {
            let p = PathBuf::from(format!("/proj/foo.{ext}"));
            assert!(
                is_scannable_source(&p),
                "expected source extension to be scannable: {p:?}"
            );
        }
    }

    /// Concrete extensions a tsconfig path alias can point at — none of
    /// these can carry a `"use client"` directive, and (per #141) feeding
    /// them to SWC crashes the scanner. The gate must skip them.
    #[test]
    fn is_scannable_source_rejects_non_source_extensions() {
        for ext in [
            "json", "css", "svg", "png", "jpg", "jpeg", "gif", "webp", "avif", "ico", "woff",
            "woff2", "ttf", "eot", "mp4", "webm", "mp3", "wav", "txt", "md", "mdx",
        ] {
            let p = PathBuf::from(format!("/proj/foo.{ext}"));
            assert!(
                !is_scannable_source(&p),
                "expected non-source extension to be rejected: {p:?}"
            );
        }
    }

    /// Files with no extension at all are skipped — the scanner has no
    /// safe way to know what they are, and source files in this codebase
    /// always carry an extension.
    #[test]
    fn is_scannable_source_rejects_extensionless_path() {
        assert!(!is_scannable_source(&PathBuf::from("/proj/Makefile")));
    }

    /// Case-insensitive extension match: case-insensitive filesystems
    /// (macOS default, Windows) can produce `.TSX` / `.Js` etc. Pre-#141
    /// the scanner accepted any extension, so the gate must keep
    /// uppercase source extensions scannable to avoid regressing those
    /// platforms.
    #[test]
    fn is_scannable_source_is_case_insensitive() {
        for ext in ["TS", "Tsx", "JSX", "MJS", "Cjs", "tSx"] {
            let p = PathBuf::from(format!("/proj/Foo.{ext}"));
            assert!(
                is_scannable_source(&p),
                "expected case-insensitive accept: {p:?}"
            );
        }
    }

    /// Mirrors the downstream issue #141 repro at the in-memory scanner
    /// level: a page imports `"#data"`; the resolver maps that to a
    /// populated `.json` file (multiple keys — empty `{}` happens to
    /// parse as a TS expression statement, masking the bug). Pre-fix the
    /// scanner crashed with `ExpectedSemiForExprStmt` inside SWC. The gate
    /// treats `.json` as a terminal leaf, so the scan completes and the
    /// reachable `"use client"` island is still discovered.
    #[test]
    fn scan_islands_skips_json_file_resolved_via_alias() {
        // A bespoke resolver that mimics a tsconfig path alias mapping
        // `"#data"` → `data.json`. Not `is_bare_specifier`-restricted so
        // the bare-looking specifier resolves successfully (matches what
        // FsResolver does with `compilerOptions.paths` after #139).
        struct AliasingResolver {
            inner: InMemoryResolver,
            json_path: PathBuf,
        }
        impl Resolver for AliasingResolver {
            fn resolve(&self, importer_dir: &Path, specifier: &str) -> Option<PathBuf> {
                if specifier == "#data" {
                    return Some(self.json_path.clone());
                }
                self.inner.resolve(importer_dir, specifier)
            }
            fn read(&self, path: &Path) -> std::result::Result<String, String> {
                self.inner.read(path)
            }
        }

        let json_path = root().join("data/payload.json");
        let inner = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r##"import data from "#data";
                import { Counter } from "../components/counter";
                export default function Home() { return <Counter data={data}/>; }
                "##,
            )
            .with_file(
                root().join("components/counter.tsx"),
                r#""use client";
                export function Counter() {}
                "#,
            )
            // Populated JSON (multiple keys). Empty `{}` parses as a
            // legal TS expression statement and would mask the bug; with
            // multiple keys SWC trips on the second key's colon. We add
            // it to the resolver's file map so anything that does try to
            // read it would succeed — proving the skip happens *before*
            // parse, not via a lucky read failure.
            .with_file(json_path.clone(), r#"{"a": 1, "b": 2, "c": 3}"#);
        let resolver = AliasingResolver { inner, json_path };

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(
            paths(&islands),
            vec![(
                "/proj/components/counter.tsx".to_string(),
                "Counter".to_string()
            )],
            "scan must skip the JSON leaf without aborting and still reach the island",
        );
    }

    /// End-to-end via `FsResolver`: mirrors the exact downstream
    /// `zudolab/zudo-doc#1355` Wave 4 shape — `tsconfig.json` declares
    /// `"#doc-history-meta": [".zfb/doc-history-meta.json"]`, the page
    /// imports it, and the scanner must not crash on the JSON.
    #[test]
    fn fs_resolver_skips_json_file_resolved_via_tsconfig_alias() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path();
        let pages = project.join("pages");
        let components = project.join("src").join("components");
        let zfb_dir = project.join(".zfb");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&components).unwrap();
        fs::create_dir_all(&zfb_dir).unwrap();

        fs::write(
            project.join("tsconfig.json"),
            r##"{
              "compilerOptions": {
                "paths": {
                  "@/*": ["src/*"],
                  "#doc-history-meta": [".zfb/doc-history-meta.json"]
                }
              }
            }"##,
        )
        .unwrap();
        // Populated JSON — empty `{}` happens to parse as a TS
        // expression statement, masking the bug. Multiple keys trip
        // SWC's parser at the second key's colon, which is the real
        // crash the downstream hit.
        fs::write(
            zfb_dir.join("doc-history-meta.json"),
            r#"{"docs/foo": {"sha": "abc"}, "docs/bar": {"sha": "def"}}"#,
        )
        .unwrap();
        fs::write(
            pages.join("home.tsx"),
            r##"import historyMeta from "#doc-history-meta";
            import { Counter } from "@/components/counter";
            export default function Home() { return <Counter meta={historyMeta}/>; }
            "##,
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
        let islands =
            scan_islands(&[pages.join("home.tsx")], &resolver).expect("scan must not crash");
        assert_eq!(islands.len(), 1, "got {islands:?}");
        assert_eq!(islands[0].component_name, "Counter");
    }

    // ---------------------------------------------------------------------
    // Issue #149 — SSR-marker name extraction
    //
    // The shared-bundle hydration manifest is keyed on the SSR-marker
    // name (`Island::marker_name`), which the scanner derives from the
    // island's source file. The bundler bakes the marker name as a
    // static literal in the synthesised entry's `__zfb_register(...)`
    // call, bypassing runtime `displayName ?? name` introspection (which
    // esbuild minification breaks).
    //
    // The tests below pin the marker-name derivation rules end-to-end
    // through `scan_islands`.
    // ---------------------------------------------------------------------

    #[test]
    fn marker_name_for_named_function_export_matches_export_name() {
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
                export function Counter() { return null; }
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].component_name, "Counter");
        assert_eq!(islands[0].marker_name, "Counter");
    }

    #[test]
    fn marker_name_for_named_const_export_matches_export_name() {
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { Toggle } from "../components/toggle";
                export default function Home() { return <Toggle/>; }
                "#,
            )
            .with_file(
                root().join("components/toggle.tsx"),
                r#""use client";
                export const Toggle = () => null;
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].component_name, "Toggle");
        assert_eq!(islands[0].marker_name, "Toggle");
    }

    #[test]
    fn marker_name_for_default_export_named_function_uses_function_identifier() {
        // Issue #149 Gap B: host-shape `export default function FooBar()`
        // — the export-side name is "default" but the SSR-marker name is
        // "FooBar" (the function identifier). The scanner must record
        // that gap so the bundler can bake a static literal manifest
        // key that survives esbuild minification.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import SidebarToggle from "../components/sidebar-toggle";
                export default function Home() { return <SidebarToggle/>; }
                "#,
            )
            .with_file(
                root().join("components/sidebar-toggle.tsx"),
                r#""use client";
                export default function SidebarToggle() { return null; }
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].component_name, "default");
        assert_eq!(
            islands[0].marker_name, "SidebarToggle",
            "issue #149 Gap B: marker name must match the function identifier"
        );
    }

    #[test]
    fn marker_name_for_default_export_named_class_uses_class_identifier() {
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import Counter from "../components/counter";
                export default function Home() { return <Counter/>; }
                "#,
            )
            .with_file(
                root().join("components/counter.tsx"),
                r#""use client";
                export default class Counter { render() { return null; } }
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].component_name, "default");
        assert_eq!(islands[0].marker_name, "Counter");
    }

    #[test]
    fn marker_name_for_anonymous_default_export_falls_back_to_default() {
        // `export default function () {}` has no identifier — there is
        // no recoverable SSR-marker name. The bundler will register the
        // entry under "default" and the SSR side will need to use
        // ANONYMOUS_COMPONENT_NAME for the `data-zfb-island=""` marker;
        // either way the scanner cannot do better here, so it just
        // mirrors `component_name`.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import Anon from "../components/anon";
                export default function Home() { return <Anon/>; }
                "#,
            )
            .with_file(
                root().join("components/anon.tsx"),
                r#""use client";
                export default function () { return null; }
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].component_name, "default");
        assert_eq!(islands[0].marker_name, "default");
    }

    // --- export { Foo as default } alias form (#984) --------------------

    #[test]
    fn marker_name_for_export_alias_as_default_uses_local_identifier() {
        // Issue #984: tsup/esbuild compiles `export default function Foo()`
        // to `function Foo() {...}; export { Foo as default }`.
        // component_name must be "default" (the public export name) but
        // marker_name must be "Foo" (the local identifier that the SSR side
        // writes via `displayName ?? name`).
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import SidebarToggle from "../components/sidebar-toggle";
                export default function Home() { return <SidebarToggle/>; }
                "#,
            )
            .with_file(
                root().join("components/sidebar-toggle.tsx"),
                r#""use client";
                function SidebarToggle() { return null; }
                export { SidebarToggle as default };
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].component_name, "default");
        assert_eq!(
            islands[0].marker_name, "SidebarToggle",
            "issue #984: marker_name must use the local identifier, not the alias"
        );
    }

    #[test]
    fn marker_name_for_export_string_literal_default_alias_uses_local_identifier() {
        // String-literal alias form: `export { Foo as "default" }` —
        // semantically identical to `export { Foo as default }`.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import Counter from "../components/counter";
                export default function Home() { return <Counter/>; }
                "#,
            )
            .with_file(
                root().join("components/counter.tsx"),
                r#""use client";
                function Counter() { return null; }
                export { Counter as "default" };
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].component_name, "default");
        assert_eq!(
            islands[0].marker_name, "Counter",
            "issue #984: string-literal 'default' alias must use the local identifier"
        );
    }

    #[test]
    fn marker_name_for_mixed_named_and_default_alias_export() {
        // `export { Foo, Foo as default }` — a module that exports the same
        // function under both its own name and as the default.  The scanner
        // emits two IslandRecords (component_name "Foo" and "default"), both
        // with marker_name "Foo".  Manifest::from_islands keys on marker_name
        // and keeps same-source duplicates silently (no collision), so the
        // manifest ends up with a single "Foo" entry.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import Foo, { Foo as FooNamed } from "../components/foo";
                export default function Home() { return <Foo/>; }
                "#,
            )
            .with_file(
                root().join("components/foo.tsx"),
                r#""use client";
                function Foo() { return null; }
                export { Foo, Foo as default };
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        // Both the named and the default export are emitted as separate
        // IslandRecords (different component_name), but both share
        // marker_name = "Foo".
        assert!(
            islands
                .iter()
                .any(|i| i.component_name == "Foo" && i.marker_name == "Foo"),
            "named export Foo must be emitted with marker_name Foo; got {islands:?}"
        );
        assert!(
            islands
                .iter()
                .any(|i| i.component_name == "default" && i.marker_name == "Foo"),
            "default alias must be emitted with marker_name Foo; got {islands:?}"
        );
    }

    #[test]
    fn marker_name_for_reexport_alias_as_default_defers_to_source_module() {
        // `export { Foo as default } from './x'` — a re-export from another
        // module.  collect_import_specifiers DFS-follows './x' and if it
        // carries "use client" it self-registers there.  The re-export
        // specifier in the barrel also produces an IslandRecord
        // (component_name "default", marker_name "Foo") pointing at the
        // barrel's path, not the source module's path; the manifest will have
        // two entries (barrel and source) both keyed on "Foo" — the one from
        // the actual "use client" source wins via insertion order.
        // This test documents the resulting behavior rather than asserting a
        // precise number of records; the key invariant is that at least one
        // island with marker_name "Foo" is emitted.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import Foo from "../components/index";
                export default function Home() { return <Foo/>; }
                "#,
            )
            .with_file(
                root().join("components/index.tsx"),
                r#"export { Foo as default } from "./foo";
                "#,
            )
            .with_file(
                root().join("components/foo.tsx"),
                r#""use client";
                export function Foo() { return null; }
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            islands.iter().any(|i| i.marker_name == "Foo"),
            "at least one island with marker_name 'Foo' must be emitted; got {islands:?}"
        );
    }

    #[test]
    fn marker_name_extracts_render_ssr_skip_placeholder_first_arg_inline_export() {
        // Issue #149 Gap A: SSR-skip wrapper functions (the
        // `*-island.tsx` shims under `packages/zudo-doc-v2/src/ssr-skip/`)
        // are exported under their wrapper name (e.g. AiChatModalIsland)
        // but emit `data-zfb-island-skip-ssr="AiChatModal"` via
        // `renderSsrSkipPlaceholder("AiChatModal", …)`. The scanner must
        // extract the literal first argument so the bundle's manifest
        // key matches the SSR-side marker, NOT the wrapper's identifier.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { AiChatModalIsland } from "../components/ai-chat-modal-island";
                export default function Home() { return <AiChatModalIsland/>; }
                "#,
            )
            .with_file(
                root().join("components/ai-chat-modal-island.tsx"),
                r#""use client";
                import { renderSsrSkipPlaceholder } from "./helpers";
                export function AiChatModalIsland(props) {
                    return renderSsrSkipPlaceholder("AiChatModal", "load", null, props);
                }
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].component_name, "AiChatModalIsland");
        assert_eq!(
            islands[0].marker_name, "AiChatModal",
            "issue #149 Gap A: marker name must come from \
             renderSsrSkipPlaceholder's first arg literal"
        );
    }

    #[test]
    fn marker_name_extracts_render_ssr_skip_placeholder_first_arg_re_export() {
        // Variant of the previous test where the wrapper is declared
        // at module level and re-exported via `export { Foo }` rather
        // than as an inline `export function Foo()`. The scanner's
        // body-scan path covers this case via `collect_body_markers`.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { ImageEnlargeIsland } from "../components/image-enlarge-island";
                export default function Home() { return <ImageEnlargeIsland/>; }
                "#,
            )
            .with_file(
                root().join("components/image-enlarge-island.tsx"),
                r#""use client";
                import { renderSsrSkipPlaceholder } from "./helpers";
                function ImageEnlargeIsland(props) {
                    return renderSsrSkipPlaceholder("ImageEnlarge", "visible", null, props);
                }
                export { ImageEnlargeIsland };
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].component_name, "ImageEnlargeIsland");
        assert_eq!(islands[0].marker_name, "ImageEnlarge");
    }

    #[test]
    fn marker_name_extracts_render_ssr_skip_placeholder_for_arrow_function_const() {
        // Variant: wrapper authored as `export const Foo = (...) => …`
        // with the helper call as the arrow-function body. The scanner
        // covers this via `marker_from_var_initialiser`.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { MockInitIsland } from "../components/mock-init-island";
                export default function Home() { return <MockInitIsland/>; }
                "#,
            )
            .with_file(
                root().join("components/mock-init-island.tsx"),
                r#""use client";
                import { renderSsrSkipPlaceholder } from "./helpers";
                export const MockInitIsland = (props) =>
                    renderSsrSkipPlaceholder("MockInit", "load", null, props);
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].component_name, "MockInitIsland");
        assert_eq!(islands[0].marker_name, "MockInit");
    }

    #[test]
    fn marker_name_render_ssr_skip_placeholder_overrides_default_export_function_name() {
        // Edge case: a default export whose body calls
        // `renderSsrSkipPlaceholder`. The helper-call extraction wins
        // over the function identifier (because the helper call is the
        // direct authority on the marker name).
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import DesignTokenTweakPanelIsland from "../components/design-token-tweak-panel-island";
                export default function Home() { return <DesignTokenTweakPanelIsland/>; }
                "#,
            )
            .with_file(
                root().join("components/design-token-tweak-panel-island.tsx"),
                r#""use client";
                import { renderSsrSkipPlaceholder } from "./helpers";
                export default function DesignTokenTweakPanelIsland(props) {
                    return renderSsrSkipPlaceholder("DesignTokenTweakPanel", "idle", null, props);
                }
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].component_name, "default");
        assert_eq!(islands[0].marker_name, "DesignTokenTweakPanel");
    }

    #[test]
    fn marker_name_ignores_unrelated_string_literals_in_function_body() {
        // The scanner must only react to `renderSsrSkipPlaceholder("X", …)`,
        // not to any other top-level call with a string-literal first arg.
        // Otherwise it would mis-derive the marker for arbitrary
        // user-written components.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { NotAWrapper } from "../components/not-a-wrapper";
                export default function Home() { return <NotAWrapper/>; }
                "#,
            )
            .with_file(
                root().join("components/not-a-wrapper.tsx"),
                r#""use client";
                import { someUnrelatedHelper } from "./helpers";
                export function NotAWrapper(props) {
                    return someUnrelatedHelper("looks-like-a-marker", props);
                }
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].component_name, "NotAWrapper");
        assert_eq!(
            islands[0].marker_name, "NotAWrapper",
            "marker_name must default to the export name when no \
             renderSsrSkipPlaceholder call is present"
        );
    }

    // --- type-only import/export edges must not reach "use client" islands ---
    // Regression tests for collect_import_specifiers: declaration-level
    // type_only guards prevent phantom islands when the only path to a
    // "use client" module is a compile-time-erased edge.

    #[test]
    fn island_not_emitted_when_reachable_only_via_import_type() {
        // `import type { Counter } from "…"` is compile-time-erased.
        // The "use client" module is reachable ONLY through this edge;
        // it must NOT appear as an island.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import type { Counter } from "../components/counter";
                export default function Home() {}
                "#,
            )
            .with_file(
                root().join("components/counter.tsx"),
                r#""use client";
                export function Counter() {}
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            islands.is_empty(),
            "a 'use client' module reachable only via `import type` must not \
             be emitted as an island; got {islands:?}"
        );
    }

    #[test]
    fn island_not_emitted_when_reachable_only_via_export_type_named() {
        // `export type { Counter } from "…"` is compile-time-erased.
        // A barrel re-exports the island with a type-only named re-export;
        // the "use client" module must NOT appear as an island.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { Counter } from "../components/index";
                export default function Home() {}
                "#,
            )
            .with_file(
                root().join("components/index.tsx"),
                r#"export type { Counter } from "./counter";
                "#,
            )
            .with_file(
                root().join("components/counter.tsx"),
                r#""use client";
                export function Counter() {}
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            islands.is_empty(),
            "a 'use client' module reachable only via `export type {{ }}` must \
             not be emitted as an island; got {islands:?}"
        );
    }

    #[test]
    fn island_not_emitted_when_reachable_only_via_export_type_star() {
        // `export type * from "…"` is compile-time-erased.
        // A barrel re-exports the island with a type-only star re-export;
        // the "use client" module must NOT appear as an island.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { Counter } from "../components/index";
                export default function Home() {}
                "#,
            )
            .with_file(
                root().join("components/index.tsx"),
                r#"export type * from "./counter";
                "#,
            )
            .with_file(
                root().join("components/counter.tsx"),
                r#""use client";
                export function Counter() {}
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            islands.is_empty(),
            "a 'use client' module reachable only via `export type *` must \
             not be emitted as an island; got {islands:?}"
        );
    }

    #[test]
    fn island_still_emitted_when_also_reachable_via_runtime_import() {
        // Control: the same "use client" module is imported both with
        // `import type` AND via a plain runtime import from another barrel.
        // The runtime path must still emit the island.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import type { Counter } from "../components/counter";
                import { Counter as CounterImpl } from "../components/counter";
                export default function Home() { return <CounterImpl/>; }
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
            islands.len(),
            1,
            "a module with both a type-only and a runtime import must still \
             emit the island; got {islands:?}"
        );
        assert_eq!(islands[0].component_name, "Counter");
    }

    #[test]
    fn island_not_emitted_for_declaration_level_type_only_named_reexport() {
        // Guard 1: `export type { Foo } from "./types"` sets type_only=true at
        // the ExportNamed declaration level. The use-client module is reached
        // via a plain runtime import, so it IS discovered — but the type-only
        // re-export must NOT add Foo to the island records.
        // Note: ./types is intentionally not registered; collect_import_specifiers
        // already skips type-only edges so the resolver is never called for it.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { Widget } from "../components/widget";
                export default function Home() {}
                "#,
            )
            .with_file(
                root().join("components/widget.tsx"),
                r#""use client";
                export type { Foo } from "./types";
                export function Widget() {}
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            islands.iter().any(|i| i.component_name == "Widget"),
            "Widget must be emitted as an island; got {islands:?}"
        );
        assert!(
            !islands.iter().any(|i| i.component_name == "Foo"),
            "Foo is a type-only re-export and must NOT be emitted as an island; got {islands:?}"
        );
    }

    #[test]
    fn island_not_emitted_for_per_specifier_type_only_named_export() {
        // Guard 2: `export { type Foo, Bar }` — declaration-level type_only is
        // false, but the Foo specifier has is_type_only=true. The use-client
        // module is reached via a plain runtime import, so it IS discovered —
        // but only Bar must appear as an island, not Foo.
        // Foo is declared (non-exported) and re-exported only via the
        // per-specifier list, so the only path by which Foo could become an
        // island is through the ExportNamed specifier — which Guard 2 must stop.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { Bar } from "../components/widget";
                export default function Home() {}
                "#,
            )
            .with_file(
                root().join("components/widget.tsx"),
                r#""use client";
                export function Bar() {}
                function Foo() {}
                export { type Foo, Bar };
                "#,
            );

        let islands = scan_islands(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            islands.iter().any(|i| i.component_name == "Bar"),
            "Bar must be emitted as an island; got {islands:?}"
        );
        assert!(
            !islands.iter().any(|i| i.component_name == "Foo"),
            "Foo is a per-specifier type-only export and must NOT be emitted as an island; got {islands:?}"
        );
    }

    // --- near-miss candidate detection (issue #822) ---------------------

    #[test]
    fn island_free_project_reports_zero_near_miss_candidates() {
        // A page that imports a server-only component — no `"use client"`
        // anywhere. The scanner must report zero near-miss candidates so
        // the caller can demote the empty-islands warning to a quiet note.
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

        let (islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(islands.is_empty(), "no islands expected: {islands:?}");
        assert_eq!(
            meta.near_miss_candidates, 0,
            "island-free project must report zero near-miss candidates"
        );
    }

    #[test]
    fn misplaced_use_client_directive_counts_as_near_miss() {
        // The directive is present but not first in the prologue (an
        // import precedes it), so `has_use_client_directive` rejects it
        // and no island is registered — but the author clearly intended
        // one. This is the case the verify-hint is written for.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { Counter } from "../components/counter";
                export default function Home() { return <Counter/>; }
                "#,
            )
            .with_file(
                root().join("components/counter.tsx"),
                r#"import { useState } from "preact/hooks";
                "use client";
                export function Counter() {}
                "#,
            );

        let (islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            islands.is_empty(),
            "misplaced directive must not register an island: {islands:?}"
        );
        assert_eq!(
            meta.near_miss_candidates, 1,
            "a module with a misplaced `use client` directive is a near-miss"
        );
    }

    #[test]
    fn whitespace_malformed_use_client_directive_counts_as_near_miss() {
        // Double-spaced and trailing-space directives: `has_use_client_directive`
        // requires the exact `use client` literal value, so both are
        // rejected — but the normalized near-miss check collapses the
        // whitespace and flags them.
        for directive in [r#""use  client";"#, r#""use client ";"#] {
            let resolver = InMemoryResolver::new()
                .with_file(
                    root().join("pages/home.tsx"),
                    r#"import { Counter } from "../components/counter";
                    export default function Home() { return <Counter/>; }
                    "#,
                )
                .with_file(
                    root().join("components/counter.tsx"),
                    format!("{directive}\nexport function Counter() {{}}\n"),
                );

            let (islands, meta) =
                scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
            assert!(
                islands.is_empty(),
                "malformed directive {directive:?} must not register an island: {islands:?}"
            );
            assert_eq!(
                meta.near_miss_candidates, 1,
                "module with whitespace-malformed directive {directive:?} is a near-miss"
            );
        }
    }

    #[test]
    fn miscased_use_client_directive_counts_as_near_miss() {
        // A mis-cased directive (`"Use client"`): `has_use_client_directive`
        // requires the exact lowercase literal, so it is rejected — the
        // normalized near-miss check lowercases and flags it.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { Counter } from "../components/counter";
                export default function Home() { return <Counter/>; }
                "#,
            )
            .with_file(
                root().join("components/counter.tsx"),
                r#""Use client";
                export function Counter() {}
                "#,
            );

        let (islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            islands.is_empty(),
            "mis-cased directive must not register an island: {islands:?}"
        );
        assert_eq!(
            meta.near_miss_candidates, 1,
            "a module with a mis-cased `use client` directive is a near-miss"
        );
    }

    #[test]
    fn use_client_in_comment_or_expression_is_not_a_near_miss() {
        // Issue #822 false-positive guard: the near-miss check is
        // AST-based, so a `use client` mention in a comment or inside an
        // ordinary expression string must NOT resurrect the warning on an
        // intentionally island-free site.
        let resolver = InMemoryResolver::new()
            .with_file(
                root().join("pages/home.tsx"),
                r#"import { ServerOnly } from "../components/server-only";
                export default function Home() { return <ServerOnly/>; }
                "#,
            )
            .with_file(
                root().join("components/server-only.tsx"),
                r#"// use client — note: this island was removed on purpose
                const label = "switch to use client mode";
                export function ServerOnly() { return <div>{label}</div>; }
                "#,
            );

        let (islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(islands.is_empty(), "no islands expected: {islands:?}");
        assert_eq!(
            meta.near_miss_candidates, 0,
            "a `use client` mention in a comment or expression must not be a near-miss"
        );
    }

    #[test]
    fn valid_directive_with_no_exports_counts_as_near_miss() {
        // A valid `"use client"` module that exports nothing: flagged as
        // an island by the author but giving the bundler nothing to ship.
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
                function Counter() {}
                "#,
            );

        let (islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert!(
            islands.is_empty(),
            "no exported island expected: {islands:?}"
        );
        assert_eq!(
            meta.near_miss_candidates, 1,
            "a valid directive with no exported component is a near-miss"
        );
    }

    #[test]
    fn valid_island_reports_zero_near_miss_candidates() {
        // The happy path: a correctly-authored island. It registers as an
        // island, so it must NOT be double-counted as a near-miss.
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
                export function Counter() {}
                "#,
            );

        let (islands, meta) =
            scan_islands_with_meta(&[root().join("pages/home.tsx")], &resolver).unwrap();
        assert_eq!(islands.len(), 1, "expected one island: {islands:?}");
        assert_eq!(
            meta.near_miss_candidates, 0,
            "a correctly-authored island must not be counted as a near-miss"
        );
    }
}
