//! Build-time **bundler stage** — produces a single ESM bundle from the
//! user's `pages/`, `content/`, `components/`, and `layouts/` source
//! roots, suitable for the embedded V8 host and the
//! runtime SSR adapter (`@takazudo/zfb-runtime`).
//!
//! ## Why this lives in `zfb-build` (not `zfb-islands`)
//!
//! `zfb-islands` already wraps esbuild for the *client-side* island
//! bundle (`{outdir}/assets/islands-{hash}.js`). That work targets the
//! browser and is keyed on `"use client"` components only.
//!
//! This module targets the *server side*: every page module the router
//! can serve, every layout/component they transitively pull in, plus the
//! framework's hydration shim for parity with the islands bundle. The
//! output is one ESM file the runtime imports to dispatch SSR for any
//! route. Mixing both jobs into `zfb-islands` would conflate "what runs
//! in the browser" with "what runs on the worker"; keeping them in
//! sibling crates is cleaner and matches how the dev pipeline already
//! splits CSS / islands / pages.
//!
//! ## Pipeline
//!
//! 1. **Materialise a shadow tree** of the user's source roots in a
//!    tempdir (`pages/`, `content/`, `components/`, `layouts/`). The
//!    shadow tree mirrors the project's directory structure so relative
//!    imports resolve unchanged.
//! 2. **Pre-compile MDX** in-place via
//!    [`zfb_content::compile_mdx_to_jsx_module_cached`]. Each `.mdx`
//!    file is rewritten to its JSX output text but keeps the `.mdx`
//!    extension; esbuild is told `--loader:.mdx=jsx` so the rewritten
//!    text is parsed as JSX. This means user-authored `.mdx` import
//!    paths keep working without a custom esbuild plugin (the CLI
//!    cannot load JS plugins).
//! 3. **Materialise the framework hydration shim** as a real file under
//!    the shadow tree (`__zfb_internal_hydrate.jsx`). The synthetic
//!    `zfb:internal/...` specifier from
//!    [`zfb_render::adapters::Adapter::hydrate_shim_specifier`] is
//!    recorded in the [`BundleManifest`] for downstream consumers; the
//!    generated entry-point file imports the shim by relative path so
//!    we don't need esbuild's URL-scheme resolver.
//! 4. **Emit a synthetic `tsconfig.json`** in the shadow root that
//!    carries the user's [`BundlerInput::tsconfig_paths`] (resolved
//!    against the project's `extends` chain by the caller). esbuild
//!    reads this via `--tsconfig=` and uses it to resolve the user's
//!    path aliases (`@/components/foo` → `./components/foo`).
//! 5. **Emit a synthetic `entry.mjs`** that imports every page module
//!    found under `pages/`, plus the hydration shim, plus the framework's
//!    `renderToString`, plus `createPageRouter` from
//!    `@takazudo/zfb-runtime/server`, and re-exports a `routes` map of
//!    route-path → page module, a `hydrateIsland` function, and a Workers
//!    entry shape `default { fetch }`. This is the single load-bearing
//!    module the embedded V8 host and the runtime SSR adapter consume.
//! 6. **Spawn esbuild** with the configured `--define`s, `--alias`es,
//!    and the synthetic `tsconfig.json`.
//!
//! ## What the consumer (T6) sees
//!
//! The output bundle is a single ESM file at [`BundlerOutput::bundle_path`]
//! with three exports the runtime contract pins:
//!
//! - `routes` — an object literal mapping route path strings to the page
//!   module's namespace (`{ default, getStaticProps?, … }`). The set of
//!   keys is also enumerated in
//!   [`BundleManifest::routes`][BundleManifest] so consumers don't have
//!   to import-and-introspect to know what routes the bundle serves.
//! - `hydrateIsland` — re-exported from the framework adapter shim;
//!   the Worker entry bundle expects this symbol so the same bundle
//!   can also feed the islands hydration runtime.
//! - `default` — a Workers-style `{ fetch }` object whose `fetch` field
//!   is a `(Request) => Promise<Response>` constructed by passing
//!   `routes`, an embedded `ContentSnapshot` placeholder, and an inline
//!   framework adapter (the framework's own `renderToString` import) to
//!   `createPageRouter` from `@takazudo/zfb-runtime/server`. This is the entry
//!   shape the embedded V8 host expects (`export default { fetch }`);
//!   without it, the host boot fails with a missing-export
//!   workerd error. Even when the route map is empty the wrapper is
//!   still emitted, so the bundle is unconditionally Workers-shaped.
//!
//! The companion `.map` file ([`BundlerOutput::sourcemap_path`]) is
//! written next to the bundle (esbuild's `--sourcemap=linked` shape).
//!
//! ## Server-secret protection
//!
//! Variables in [`BundlerInput::public_env_vars`] that are **not** prefixed
//! with `PUBLIC_` are silently dropped — they never reach the bundle.
//! Operator-authored [`BundlerInput::define_vars`] are a separate, raw
//! esbuild-define channel populated from validated `bundle.define` config.
//! `import.meta.env.PROD` and `import.meta.env.DEV` are always emitted
//! (driven by [`BundlerInput::mode`]). See [`BundleMode`] and the
//! `server_secrets_are_not_bundled` test for the contract.
//!
//! ## Esbuild binary resolution
//!
//! Handled by the shared resolver [`resolve_esbuild_binary_with_env`], which
//! is also used by `zfb::config` (the config-loader). The lookup order for
//! this (bundler) call site is:
//!
//! 1. [`BundlerInput::esbuild_binary`] (explicit override).
//! 2. `ZFB_ESBUILD_BIN` environment variable.
//! 3. `crates/zfb/binaries/esbuild/esbuild` ([`DEFAULT_ESBUILD_SLOT`] —
//!    release-tarball slot; see that directory's README).
//!
//! The config-loader call site additionally inserts an embedded-extraction
//! tier (tier 3 of 4) between the env var and the slot; see
//! [`resolve_esbuild_binary_with_env`] for the full superset documentation.
//!
//! If the resolved path does not exist, [`bundle`] returns a clear error
//! instructing the operator to either set the env var or stage the
//! binary in the slot.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use walkdir::WalkDir;

use zfb_content::diagnostics::{DiagnosticSeverity, MarkdownDiagnostic, SourceLocation};
use zfb_content::frontmatter as zfb_frontmatter;
use zfb_content::plugins::util::source_map::{
    build_docs_source_map, CollectionRoute, DocsSourceMapOptions,
};
use zfb_content::{
    compile_mdx_to_jsx_module_cached, compile_mdx_to_jsx_module_cached_with_deps, CompiledMdx,
    CrossFileLinkCandidate, FileHeadings, MdxModuleCache,
};
use zfb_render::adapters::{make_adapter, Framework};
use zfb_types::{json_string as json_str, normalize_path_lexical, path_to_posix_string};

use crate::adapter::run_capturing;
// The `import.meta.glob` expansion helpers moved to their own `pub` module
// (issue #1402) so #1404's downstream orchestration in `crates/zfb` can
// reuse them without `zfb-islands` gaining a `zfb-build` dependency. Only
// `expand_import_meta_glob_with_matches` is called directly from this file
// (in `materialise_source_file`) — the richer form is used (rather than the
// `expand_import_meta_glob` convenience wrapper) so the matched target files
// feed the issue #1695 fixed-point staging queue below; `glob_match_relative`
// and `GlobCallCollector` have no other call sites left in `bundler.rs`.
use crate::glob_expand::expand_import_meta_glob_with_matches;
use crate::module_worker::{
    collect_runtime_import_specifiers_from_file, discover_module_preprocessing_with_context,
    discover_module_preprocessing_with_tsconfig_paths,
    discover_registered_virtual_preprocessing_with_context,
    remap_virtual_module_project_imports_to_shadow, rewrite_module_worker_urls_with_context,
    ModuleWorkerBuildContext, ModuleWorkerDependency,
};
use crate::raw_import_expand::{
    expand_raw_imports_with_aliases, RawImportAliasContext, RawImportEdge,
};

/// `import.meta.env.{PROD,DEV}` substitution mode.
///
/// `Production` substitutes `PROD=true`, `DEV=false`. `Development` is
/// the inverse. Any other `import.meta.env.*` access remains untouched —
/// the bundler does not invent values the user did not supply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleMode {
    /// Production build: `import.meta.env.PROD = true`.
    Production,
    /// Development build: `import.meta.env.DEV = true`.
    Development,
}

impl BundleMode {
    fn is_prod(self) -> bool {
        matches!(self, BundleMode::Production)
    }
}

/// One content collection root for the bundler's per-collection
/// materialisation step.
///
/// Mirrors the public-facing `name` + `path` shape of
/// `zfb_content::CollectionConfig` and `crate::config::CollectionDef`.
/// Each entry tells the bundler where a collection's source files live
/// on disk; they are materialised into the shadow tree under
/// `shadow/content/<name>/<rel_path>`. The collection name doubles as
/// the prefix used for the `import * as __zfb_content_<i>` lines and
/// is also what the JS bridge keys on (paired with the `mdx://`
/// specifier from `compile_mdx_to_jsx_module_cached`).
///
/// When [`BundlerInput::content_collections`] is empty, the bundler
/// falls back to the legacy single-`content_dir` materialisation path
/// and does NOT emit a content bridge in entry.mjs.
#[derive(Debug, Clone)]
pub struct ContentCollectionSpec {
    /// Public collection name, matching `zfb.config.ts#collections[].name`.
    pub name: String,
    /// Source directory (project-relative or absolute).
    pub root: PathBuf,
    /// Optional include globs (Astro-style). MUST match
    /// `zfb_content::CollectionConfig::include` exactly so the snapshot ↔
    /// bridge keys agree on which files survive.
    pub include: Option<Vec<String>>,
    /// Optional exclude globs. MUST match the snapshot side.
    pub exclude: Option<Vec<String>>,
    /// Optional suffix to strip from the `<slug>` segment of the
    /// `mdx://` / `tsx://` specifier the bridge installer emits for
    /// each kept entry. MUST match the snapshot side — otherwise
    /// every `globalThis.__zfb.content.get(spec)` misses.
    pub id_strip_suffix: Option<String>,
}

impl ContentCollectionSpec {
    /// Convenience constructor (no filters). Use struct-init shorthand
    /// to set the filter fields directly when needed.
    pub fn new(name: impl Into<String>, root: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            root: root.into(),
            include: None,
            exclude: None,
            id_strip_suffix: None,
        }
    }
}

/// What to do when a `.md`/`.mdx` link cannot be resolved during a build.
///
/// Mirrors `zfb::config::OnBrokenLinks` — re-declared here so the bundler
/// has no dependency on the `zfb` config crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnBrokenLinks {
    /// Emit a warning to stderr but continue. Default.
    #[default]
    Warn,
    /// Return an error after the walk completes (all broken links reported).
    Error,
    /// Silently ignore broken links.
    Ignore,
}

/// Options for the `ResolveLinksPlugin` in the bundler pipeline.
///
/// When present on [`BundlerInput`], the bundler builds a
/// `path → URL` source map from every entry in `routes`, appends
/// [`zfb_content::plugins::ResolveLinksPlugin`] to the mdast pipeline of
/// every materialisation call, and handles broken links according to
/// `on_broken_links` after the walk completes.
///
/// Each entry in `routes` is one source dir + its route prefix
/// (e.g. EN docs at `src/content/docs/` → `/docs/`, JA docs at
/// `src/content/docs-ja/` → `/ja/docs/`). The bundler scans them all
/// and merges the resulting `path → URL` map. Required for any project
/// with locale mirrors so each mirror dir resolves under its own route
/// prefix — see `zudolab/zudo-doc#1577` for the host-side bug this
/// surfaces when only a single dir is configured.
#[derive(Debug, Clone)]
pub struct ResolveMarkdownLinksSpec {
    /// Per-dir source map. Empty `routes` is a no-op (caller would
    /// just not pass `Some` in that case).
    pub routes: Vec<ResolveMarkdownLinksRoute>,
    /// What to do with unresolved `.md`/`.mdx` links.
    pub on_broken_links: OnBrokenLinks,
}

/// One entry in [`ResolveMarkdownLinksSpec::routes`]. Mirrors
/// [`zfb_content::plugins::util::source_map::CollectionRoute`] minus the
/// informational `name` (callers don't surface route names).
#[derive(Debug, Clone)]
pub struct ResolveMarkdownLinksRoute {
    /// Directory (absolute or project-relative) whose `.md`/`.mdx`
    /// files are scanned.
    pub docs_dir: PathBuf,
    /// Route prefix applied to every slug from this dir
    /// (e.g. `"/docs/"`). Must include the trailing slash.
    pub route_prefix: String,
}

/// All the inputs the bundler needs.
///
/// The struct is deliberately *flat* — no builder, no defaults that hide
/// path resolution. Callers (T6 / T7) construct one explicitly so the
/// wiring is visible at the call site.
#[derive(Debug, Clone)]
pub struct BundlerInput {
    /// Project root. All other paths are interpreted relative to this if
    /// they are relative; absolute paths pass through.
    pub project_root: PathBuf,
    /// Directory of route source files. Every file whose extension is in
    /// [`zfb_types::ROUTABLE_PAGE_EXTENSIONS`] (`.tsx`/`.ts`/`.jsx`/`.js`/
    /// `.mdx`/`.md`/`.html`) under this directory becomes a route in the
    /// bundle — see [`derive_route`].
    pub pages_dir: PathBuf,
    /// ADDITIVE second pages root for the dev server's package-owned
    /// **injected** routes (epic #1228, S2 #1230 — the B1 multi-root
    /// mechanism). When `Some`, the bundler materialises this root into the
    /// SAME shadow `pages/` tree as [`Self::pages_dir`] (a second
    /// `materialise_shadow` call). Conventional dev sessions therefore contain
    /// both the user's pages and the synthesized injected modules. It holds
    /// ONLY the injected modules -- no user-page copy -- so conventional `zfb
    /// dev` keeps `pages_dir` = the real `project_root/pages` for the router
    /// scan + watcher (user-page `source_path` identity / HMR untouched).
    /// #1518 true-zero-pages dev instead uses a private empty primary
    /// `pages_dir` alongside this additive root.
    ///
    /// Distinct from the `zfb build` overlay, which instead OVERRIDES
    /// `pages_dir` with a root that already contains a copy of the user pages.
    /// `None` for `zfb build` and for `zfb dev` with no injected routes —
    /// byte-identical to a bundle that never knew this field (the additive
    /// walk is skipped entirely). Default: `None`.
    pub injected_pages_root: Option<PathBuf>,
    /// Directory of content collections. `.mdx` files anywhere under
    /// this directory are pre-compiled with
    /// [`compile_mdx_to_jsx_module_cached`] before esbuild sees them.
    ///
    /// **Legacy single-root path.** When [`Self::content_collections`]
    /// is non-empty, the bundler walks each configured collection
    /// individually and this field is ignored. The legacy field stays
    /// in the struct so historical test fixtures keep compiling and
    /// projects whose `zfb.config.ts` has no `collections` entry still
    /// get a (no-op) materialisation step.
    pub content_dir: PathBuf,
    /// Per-collection content roots. When non-empty, supersedes
    /// [`Self::content_dir`]: each collection is materialised under
    /// `shadow/content/<name>/<rel_path>` so the synthetic `entry.mjs`
    /// can `import * as __zfb_content_<i> from "./content/<name>/..."`
    /// for every `.mdx` entry. The paired bridge installer
    /// (`globalThis.__zfb.content`) is then emitted in `entry.mjs`
    /// before `createPageRouter`, matching the contract documented in
    /// `crates/zfb-render/src/loader.rs`.
    ///
    /// Empty by default. Callers that want the bridge wired (i.e. all
    /// production builds whose `zfb.config.ts` declares collections)
    /// must populate this from `config.collections`.
    pub content_collections: Vec<ContentCollectionSpec>,
    /// Directory of shared components (subject to `paths` aliasing).
    pub components_dir: PathBuf,
    /// Directory of layout components.
    pub layouts_dir: PathBuf,
    /// Which JSX framework's hydration shim to fold into the bundle.
    /// Drives [`make_adapter`] selection.
    pub framework: Framework,
    /// Operator-authored raw esbuild `--define` substitutions populated from
    /// validated `bundle.define` config. Values are forwarded verbatim; string
    /// expressions must already be quoted JSON. This path is deliberately
    /// separate from [`Self::public_env_vars`].
    pub define_vars: BTreeMap<String, String>,
    /// Environment variables considered for public exposure. Only keys
    /// prefixed with `PUBLIC_` are emitted, and values are JSON-encoded before
    /// being mapped to both `process.env.<KEY>` and `import.meta.env.<KEY>`.
    /// All other entries are silently dropped so server secrets never appear
    /// in the bundle. See [`server_secrets_are_not_bundled`] in tests.
    pub public_env_vars: HashMap<String, String>,
    /// `compilerOptions.paths`-style alias map (TS path aliases). The
    /// bundler writes a rebased copy into a synthetic `tsconfig.json`
    /// inside the shadow tree; esbuild then resolves user imports
    /// (`@/components/foo`) through it via `--tsconfig=`.
    ///
    /// Caller is responsible for resolving the project's `extends`
    /// chain (e.g. `tsconfig.base.json`) before passing the merged map
    /// here. Path targets are expected to be **absolute paths under the
    /// project root** (the shape `read_tsconfig_paths` produces by
    /// absolutising each target against the project root, preserving a
    /// trailing `/*`).
    ///
    /// Before writing the synthetic tsconfig the bundler rebases each
    /// under-`project_root` target to a **shadow-first dual-target**
    /// `["<shadow>/<rel>[/*]", "<original real-abs target>"]` (see
    /// [`rebase_tsconfig_paths_to_shadow`]). This is what makes an aliased
    /// import reach the in-shadow `import.meta.glob` / `.module.css`
    /// transform (the shadow copy is tried first; the real target is the
    /// graceful fallback). Targets NOT under `project_root` (plugin /
    /// virtual / out-of-tree) are written unchanged.
    pub tsconfig_paths: BTreeMap<String, Vec<String>>,
    /// Bare specifiers to leave unresolved in the bundle. Use for
    /// `preact`, `react`, `react-dom/server`, etc. — packages the
    /// runtime SSR adapter (T2) provides at embedded V8 host load time. An
    /// empty vec means "bundle everything from node_modules".
    pub external: Vec<String>,
    /// Explicit esbuild `--main-fields` list for the `--platform=neutral`
    /// page/SSR pass. Under `neutral` esbuild's main-fields list is EMPTY by
    /// default, so a dep resolved purely via `package.json` `main`/`module`
    /// (no `exports` map) fails with `The "main" field here was ignored. Main
    /// fields must be configured explicitly when using the "neutral"
    /// platform.` Setting e.g. `["main", "module"]` lets such CJS-main-only
    /// deps resolve (#676 -- `msw` -> `path-to-regexp@6`).
    ///
    /// Empty (the default) -> no `--main-fields` is emitted EXCEPT the existing
    /// React-only `main,module` shim, so a non-React bundle stays
    /// byte-identical to a build without this knob. When non-empty it applies
    /// to every framework and takes precedence over the React shim.
    pub main_fields: Vec<String>,
    /// Additional validated esbuild `--loader:<ext>=<loader>` arguments.
    /// Appended after [`ESBUILD_LOADER_ARGS`] in deterministic config order.
    pub extra_loader_args: Vec<String>,
    /// Where the final `bundle.mjs` (and its `.map`) is written.
    pub outdir: PathBuf,
    /// Production / development mode (drives `import.meta.env.{PROD,DEV}`).
    pub mode: BundleMode,
    /// `--minify` toggle. Only meaningful in `Production` mode.
    pub minify: bool,
    /// Override for the esbuild binary path. See module-level docs for
    /// the resolution precedence when `None`.
    pub esbuild_binary: Option<PathBuf>,
    /// Test escape hatch: when `Some`, **skip the esbuild subprocess**
    /// and write `mock_subprocess_output` to the bundle path instead.
    /// Mirrors `zfb_islands::EsbuildSubprocessConfig::mock_output` so
    /// unit tests don't need the real binary on disk.
    pub mock_subprocess_output: Option<String>,
    /// Optional JSON-serialized content snapshot to embed in the worker
    /// bundle. When `Some`, the bundler replaces the placeholder empty
    /// `{ collections: {} }` with the supplied JSON so the worker's
    /// `getCollection(...)` calls resolve real content entries. When
    /// `None`, the placeholder is used (safe for builds where content
    /// collections are not needed or not yet built).
    ///
    /// The value MUST be a valid JSON object whose top-level shape is
    /// `{ "collections": { "<name>": [...] } }` — the same shape
    /// [`zfb_content::ContentSnapshot`] serializes to. The bundler
    /// inlines it verbatim; no validation is performed.
    pub content_snapshot_json: Option<String>,
    /// Optional directory to symlink into the shadow tree as
    /// `node_modules` before esbuild runs. Useful in tests and
    /// tooling environments where the shadow tree (a tempdir) cannot
    /// reach workspace-level `node_modules` via the standard
    /// ancestor-directory walk.
    ///
    /// When `Some(path)`, a **symlink** `<shadow>/node_modules →
    /// <path>` is created so esbuild finds packages there first.
    /// The path MUST be an existing directory. On platforms where
    /// symlinks are restricted, a junction is attempted; if both
    /// fail, [`bundle`] returns an error.
    ///
    /// In a typical pnpm workspace, pass
    /// `<workspace-root>/node_modules/.pnpm/node_modules` to give
    /// esbuild access to the shared virtual store.
    ///
    /// Production builds leave this `None`; the project root's own
    /// `node_modules` tree is accessible via the ancestor walk.
    pub node_modules_dir: Option<PathBuf>,

    /// When `true`, esbuild is invoked with `--preserve-symlinks` so it
    /// keeps every importer anchored at its shadow-tree location during
    /// resolution. Set by callers that point
    /// [`Self::node_modules_dir`] at a synthetic vendored tree (e.g.
    /// the binary-embedded `@takazudo/zfb-runtime` extracted into a
    /// tempdir) where the package contents physically live OUTSIDE the
    /// project root. With `--preserve-symlinks` esbuild stays at the
    /// `<shadow>/node_modules/<pkg>` symlink and finds transitive deps
    /// in the injected vendor tree; without it, esbuild would canonicalise
    /// the symlinked source back to the real path and walk up from there
    /// (where no `node_modules` exists).
    ///
    /// Leave `false` for production builds whose
    /// [`Self::node_modules_dir`] points at the **project's own**
    /// `node_modules`. Two reasons:
    ///
    /// 1. The project root has a real `node_modules` directly above it,
    ///    so the canonicalised-then-walk-up path already finds it.
    /// 2. `--preserve-symlinks` keeps workspace-package importers
    ///    anchored at `<shadow>/node_modules/<pkg>/...`, and esbuild
    ///    skips `tsconfig.json` discovery for any importer whose path
    ///    contains a `node_modules` segment — so the synthetic
    ///    tsconfig's `paths` (e.g. `"@/*": ["src/*"]`) would not apply
    ///    to imports written inside workspace packages. See issues
    ///    #443 / #450 for the regression that landed in
    ///    `0.1.0-next.2` when this flag was made unconditional.
    pub node_modules_preserve_symlinks: bool,

    /// The shared markdown-pipeline knob set (zfb#917). This is the SAME
    /// [`zfb_content::PipelineSpec`] type the snapshot walker
    /// (`zfb_content::build_snapshot_with_config`) accepts, and both
    /// surfaces build their pipelines through the single
    /// [`zfb_content::PipelineSpec::build_pipeline`] path — which is what
    /// keeps the JSX `content_hash` baked into compiled MDX modules
    /// byte-identical to the snapshot's `module_specifier` hashes
    /// (zfb#187 / #188).
    ///
    /// One field is special: `pipeline_spec.resolve_source_map` is
    /// derivation-owned by [`bundle`] — it is ALWAYS overwritten from
    /// [`Self::resolve_markdown_links`] (built when `Some`, cleared when
    /// `None`), so callers cannot desync the map from the route spec.
    /// Set every other knob here; leave the map alone.
    ///
    /// The dev loader at `crates/zfb-render/src/loader.rs` honours the
    /// same knobs via its own `with_*` builders so `zfb dev` and
    /// `zfb build` produce the same output shape.
    pub pipeline_spec: zfb_content::PipelineSpec,

    /// Optional markdown link resolver. When `Some`, the bundler builds a
    /// source map from [`ResolveMarkdownLinksSpec::docs_dir`], appends
    /// [`zfb_content::plugins::ResolveLinksPlugin`] to the MDX pipeline,
    /// and handles broken links per [`ResolveMarkdownLinksSpec::on_broken_links`].
    ///
    /// **Shape decision (zfb#917):** this stays a bundler-side INPUT —
    /// separate from the pipeline-visible knob
    /// (`pipeline_spec.resolve_source_map`) — because relative
    /// [`ResolveMarkdownLinksSpec::docs_dir`] entries need the bundler's
    /// [`PathResolver`] and `on_broken_links` is a build policy, not a
    /// pipeline-shape knob. [`bundle`] derives the source-map knob from
    /// this spec via the same `build_docs_source_map` helper the
    /// snapshot path uses, so both surfaces resolve identical URL
    /// strings.
    ///
    /// Mirrors `zfb::config::Config::resolve_markdown_links`. Default: `None`
    /// (pass-through — links are not rewritten).
    pub resolve_markdown_links: Option<ResolveMarkdownLinksSpec>,

    /// Canonical origin URL for the site, threaded from
    /// `zfb::config::Config::site`. When `Some`, the bundler emits
    /// `globalThis.__zfb.site = <value>` in the synthetic `entry.mjs`
    /// so layouts can build canonical `<link>` tags, OG URLs, sitemap
    /// absolute hrefs, and hreflang `<link rel="alternate">` without
    /// hard-coding the origin. When `None`, no setter is emitted — the
    /// build output is byte-for-byte identical to the pre-`site` build.
    ///
    /// The value is validated as an absolute HTTP/HTTPS URL by the
    /// config loader before reaching here. Default: `None`.
    pub site: Option<String>,

    /// When `true`, the bundler emits `globalThis.__zfb.prefetchDisabled = true`
    /// in the synthetic `entry.mjs` so `<ClientRouter />` renders the disable
    /// meta tag and the prefetch-core module short-circuits at `init()` time.
    ///
    /// Mirrors `zfb::config::Config::prefetch.disabled`. Default: `false`.
    pub prefetch_disabled: bool,

    /// Plugin-registered import aliases. Each `(from, to)` pair maps a
    /// bare specifier (e.g. `@/foo`) to an absolute path string (e.g.
    /// `/abs/src/foo.tsx`). Forwarded to esbuild as `--alias:<from>=<to>`
    /// flags alongside the hard-coded preact shim aliases.
    ///
    /// Populated by the command layer from `setup_registries.aliases` for
    /// both `zfb build` and `zfb dev`. Default: empty.
    ///
    /// Mirrors `zfb_islands::EsbuildSubprocessConfig::alias_entries` so
    /// the islands path and the main bundler path receive identical alias sets.
    pub plugin_alias_entries: Vec<(String, String)>,

    /// Plugin-registered virtual modules. Each `(specifier, source)` pair
    /// maps a bare specifier (e.g. `virtual:foo`) to its JS/TS source text.
    /// At bundle time the source is written to a temp `.mjs` file and an
    /// `--alias:<specifier>=<path>` flag redirects imports to that file.
    ///
    /// Populated by the command layer from `setup_registries.virtual_modules`
    /// (sources pre-fetched via `invoke_virtual_loader`). Default: empty.
    ///
    /// Mirrors `zfb_islands::EsbuildSubprocessConfig::virtual_modules`.
    pub plugin_virtual_modules: Vec<(String, String)>,

    /// When `Some(set)`, the synthetic `entry.mjs` only imports + registers
    /// the routes whose [`RouteEntry::entry_key`] appears in the set. All
    /// other discovered routes are still visible to `BundlerOutput::manifest`
    /// (so build-time bookkeeping continues to know about them) but they
    /// are unreachable from the entry — esbuild's tree-shaker drops them
    /// and their transitive deps from the final bundle.
    ///
    /// **The strings in the set MUST be Hono-form** (e.g. `/blog/:slug`,
    /// `/manuals/:path{.+}`). `RouteEntry::entry_key` is also stored in
    /// Hono-form (via `bracket_to_hono`), so the filter matches by exact
    /// string equality for every route shape, including catch-alls (zfb#532).
    ///
    /// Intended for "runtime-only" bundle passes consumed by deploy
    /// adapters whose dispatch path serves prerendered routes from a
    /// separate static-asset server (e.g. Cloudflare Pages' `ASSETS first,
    /// inner on 404` shape). Pass the set of routes with `prerender =
    /// false`; the resulting bundle ships only the SSR code path.
    ///
    /// `None` (the default) preserves the existing single-bundle behavior:
    /// every discovered route is imported into the entry. SSG render
    /// pipelines must keep this `None` because the embedded V8 host walks
    /// every route to write static HTML.
    ///
    /// Additional side-effects when `Some`:
    /// - `content_imports` is emptied for the entry — getCollection()
    ///   bridge entries do not reach the runtime bundle. Combined with
    ///   the content snapshot suppression below, this drops MDX-derived
    ///   data (incl. inline image / blurhash data URIs) from the worker.
    /// - [`Self::content_snapshot_json`] is treated as if it were `None`,
    ///   so `getCollection(...)` inside SSR routes resolves against an
    ///   empty snapshot. Refine if a real consumer needs SSR-time content
    ///   access.
    pub worker_only_routes: Option<std::collections::BTreeSet<String>>,

    /// Filename (not full path) of the emitted bundle inside [`Self::outdir`].
    /// `None` defaults to `"bundle.mjs"` for backward compatibility.
    ///
    /// Allows two consecutive `bundle()` calls in the same `outdir` to
    /// produce distinct artifacts — e.g. a full `bundle.mjs` for SSG
    /// render and a `bundle-runtime.mjs` for the deploy adapter. The
    /// sourcemap suffix `.map` is appended automatically.
    pub bundle_basename: Option<String>,

    /// CSS Modules class-name maps, keyed by the **absolute** path of
    /// each `.module.css` file on disk. Each value is the
    /// original-class → scoped-class map produced by
    /// [`zfb_css::CssPipelineOutput::class_maps`].
    ///
    /// This is how the build-time CSS Modules JSX rewrite is wired
    /// (see the `lib.rs` "CSS Modules JS-side rewrite contract" in
    /// `zfb-css`). When a `.module.css` file under one of the
    /// materialised source roots has an entry here, the bundler
    /// writes a JS module — `export default { "orig": "scoped", … }`
    /// — into the shadow tree in place of the raw CSS bytes, and adds
    /// `--loader:.module.css=js` so esbuild parses it as JS. A user's
    /// `import styles from "./x.module.css"; styles.foo` then resolves
    /// to the scoped class string at bundle time.
    ///
    /// The raw CSS bytes are NOT lost — the scoped CSS itself is
    /// emitted into `dist/assets/styles-<hash>.css` by
    /// `CssPipeline::build_emitter`. Only the *import* is redirected
    /// to the map.
    ///
    /// Empty by default — projects with no `.module.css` files (or
    /// callers that do not run the CSS pipeline first) get the
    /// previous behaviour, where `.module.css` falls through to the
    /// `.css=empty` loader.
    pub css_module_class_maps: HashMap<PathBuf, HashMap<String, String>>,

    /// Optional absolute path to a project-root `mdx-components.tsx` file
    /// (the Next-style global element→component override map convention,
    /// sub-issue #616). When `Some`, the bundler copies the file into the
    /// shadow root (so its relative imports + tsconfig `paths` resolve
    /// in-shadow) and emits, in the synthetic `entry.mjs`:
    ///
    /// ```js
    /// import __zfb_mdx_components from "./mdx-components.tsx";
    /// globalThis.__zfb = globalThis.__zfb ?? {};
    /// globalThis.__zfb.mdxComponents = __zfb_mdx_components;
    /// ```
    ///
    /// The file's **default export** is the canonical contract — a flat
    /// `{ h2: MyH2, … }` map read by `mergeMdxComponents` in
    /// `@takazudo/zfb`'s `content.ts` (the precedence seam from #614:
    /// defaultComponents → this global slot → per-`<Content>` `components`).
    ///
    /// Emission is **independent of `content_imports`** — a project may
    /// define overrides with zero content-collection entries, so the
    /// install is gated only on this field being `Some`, guarded by the
    /// idempotent `__zfb ??= {}` namespacing. When `None`, zero bytes are
    /// emitted and no file is copied, so the build output is byte-for-byte
    /// identical to a project without the convention.
    ///
    /// The shadow is a fresh tempdir per `bundle()` call, so discovery
    /// re-runs every build and `zfb dev` / preview pick up edits with no
    /// special-casing. Default: `None`.
    pub mdx_components_file: Option<PathBuf>,

    /// Gitignore-style globs for source files the bundler must keep OUT of the
    /// esbuild graph.
    ///
    /// Mirrors `zfb::config::resolve_bundle_exclude(config.bundle)`. Patterns
    /// match a candidate file's relative POSIX path (e.g.
    /// `components/Foo.stories.tsx`, `components/**/*.stories.tsx`) under a
    /// TWO-TIER anchoring contract (issue #1668 part 2):
    ///
    /// - **Project files** (under [`Self::project_root`]) match their
    ///   PROJECT-relative path — the original, documented behavior. An
    ///   existing `src/**` keeps meaning "under the project's `src/`", so a
    ///   nested pnpm-workspace host does not silently change what a pattern
    ///   excludes.
    /// - **Workspace-sibling files** (under the enclosing pnpm workspace root
    ///   but OUTSIDE the project, staged into the SSR shadow by #1668) match
    ///   their WORKSPACE-relative path (e.g. `lib/shared/secret.ts`). A
    ///   project-relative pattern like `src/**` therefore can never
    ///   accidentally exclude a sibling whose workspace-relative path lies
    ///   outside `src/`. The sibling tier is inert outside a pnpm workspace.
    ///
    /// A matched file is applied at TWO consistent points so they cannot
    /// diverge:
    ///
    /// 1. `materialise_shadow` SKIPS copying/symlinking it into the shadow
    ///    tree (the file is never present for esbuild to resolve).
    /// 2. The #665 `import.meta.glob` eager-expansion seam
    ///    ([`expand_import_meta_glob_with_matches`]) DROPS it from any glob
    ///    expansion, so an excluded file is never emitted as a static import
    ///    (which would otherwise make esbuild error on the generated
    ///    import).
    ///
    /// Why this is needed: a `--platform=neutral` worker bundle rejects
    /// CJS-only packages resolved only via `main`/`module` or a
    /// `require`-only `exports` condition (e.g. `msw` → `path-to-regexp@6`).
    /// Once #665's eager glob over `components/**/*.stories.tsx` lands, the
    /// build newly pulls such a package in; excluding the offending file
    /// keeps it green.
    ///
    /// Empty (the default) → no files are skipped; the build output is
    /// byte-for-byte identical to a build without this knob.
    pub bundle_exclude: Vec<String>,

    /// Base prefix for client-script assets, emitted as
    /// `globalThis.__zfb.base = "<prefix>"` in the synthetic `entry.mjs` so
    /// `clientScript(name)` can build the correct base-prefixed stable URL
    /// at SSR time.
    ///
    /// ## Conditional emission rule
    ///
    /// `Some(prefix)` is set by the caller ONLY when at least one
    /// `*.client.{ts,tsx,js,jsx}` entry was discovered in the project.
    /// When `None`, zero bytes are emitted so builds without client scripts
    /// remain byte-for-byte identical to pre-#978 builds (#261 zero-
    /// registration parity, #940 byte-identical dev bundle skip).
    ///
    /// ## Value semantics
    ///
    /// The prefix is the normalised base string from
    /// `zfb::config::asset_url_base_prefix(config.base)` for production
    /// builds, and `zfb_types::dev_mount_prefix(config.base).unwrap_or_default()`
    /// for dev builds.
    ///
    /// - Root-mounted or no-base sites → `Some("")` (empty string; the
    ///   JS side reads `globalThis.__zfb?.base ?? ""` and gets `""`, so
    ///   `clientScript("x")` returns `"/assets/client/x.js"` as expected).
    /// - Sub-path deploy (`base="/foo/"`) → `Some("/foo")`.
    /// - Absolute-URL base (CDN, `https://cdn.example.com/`) under
    ///   `zfb dev` → `Some("")` (dev server cannot serve a different origin;
    ///   `dev_mount_prefix` collapses absolute-URL bases to `None`, which
    ///   the caller converts to `""` so the dev bundle still emits the slot).
    ///
    /// Default: `None`.
    pub base_prefix: Option<String>,
}

impl BundlerInput {
    /// Construct a `BundlerInput` with the shared project-wide defaults,
    /// overriding only the fields that differ per command.
    ///
    /// Shared defaults:
    /// - Standard relative directory names (`pages`, `content`, `components`,
    ///   `layouts`).
    /// - Empty raw `define_vars`, `public_env_vars`, `tsconfig_paths`,
    ///   `external`, and `extra_loader_args`.
    /// - `minify: false`, `esbuild_binary: None`, `mock_subprocess_output:
    ///   None`, `node_modules_dir: None`.
    ///
    /// Callers that need to override additional fields (e.g. test escape
    /// hatches) should use struct-update syntax: `BundlerInput { field:
    /// new_value, ..BundlerInput::for_project(...) }`.
    pub fn for_project(
        project_root: PathBuf,
        framework: Framework,
        mode: BundleMode,
        outdir: PathBuf,
        content_snapshot_json: Option<String>,
    ) -> Self {
        Self {
            project_root,
            pages_dir: PathBuf::from("pages"),
            injected_pages_root: None,
            content_dir: PathBuf::from("content"),
            content_collections: Vec::new(),
            components_dir: PathBuf::from("components"),
            layouts_dir: PathBuf::from("layouts"),
            framework,
            define_vars: Default::default(),
            public_env_vars: Default::default(),
            tsconfig_paths: Default::default(),
            external: Vec::new(),
            main_fields: Vec::new(),
            extra_loader_args: Vec::new(),
            outdir,
            mode,
            minify: false,
            esbuild_binary: None,
            mock_subprocess_output: None,
            content_snapshot_json,
            node_modules_dir: None,
            node_modules_preserve_symlinks: false,
            pipeline_spec: zfb_content::PipelineSpec::default(),
            resolve_markdown_links: None,
            site: None,
            prefetch_disabled: false,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            worker_only_routes: None,
            bundle_basename: None,
            css_module_class_maps: HashMap::new(),
            mdx_components_file: None,
            bundle_exclude: Vec::new(),
            base_prefix: None,
        }
    }
}

/// Output of [`bundle`].
#[derive(Debug, Clone)]
pub struct BundlerOutput {
    /// Final ESM bundle on disk. ESM, not CommonJS — exports `routes`
    /// and `hydrateIsland` per the module-level contract.
    pub bundle_path: PathBuf,
    /// Linked sourcemap next to `bundle_path` (esbuild's
    /// `--sourcemap=linked` shape). When the bundler ran in
    /// `mock_subprocess_output` mode, this path is still returned but
    /// the file may not exist.
    pub sourcemap_path: PathBuf,
    /// Routes the bundle serves (also addressable through
    /// `bundle.routes` at runtime).
    pub manifest: BundleManifest,
    /// Per-route transitive `Module` deps parsed from esbuild's `--metafile`
    /// (#1284/#1287). Populated only for [`BundleMode::Development`] (the live
    /// dev graph is the sole consumer); empty for the prod / SSG path, whose
    /// esbuild arg set and output stay byte-identical to before. Each entry's
    /// `source_path` is the route's project-relative page source (`PageId`), and
    /// `module_deps` are the canonicalised real on-disk paths it transitively
    /// imports — what the dev graph upserts as `DepKind::Module` edges and what
    /// the watcher registers as extra targets for out-of-root (symlinked
    /// workspace) deps.
    pub route_module_deps: Vec<crate::metafile_deps::RouteModuleDeps>,
    /// Wasm files emitted beside this bundle by esbuild's `.wasm=copy` loader.
    ///
    /// Every path is relative to [`Self::bundle_path`]'s parent directory,
    /// sorted, deduplicated, and validated to exist under that directory before
    /// this output is returned. Deployment adapters use this contract to carry
    /// the copied modules through to the final Worker package.
    pub emitted_wasm_assets: Vec<PathBuf>,
}

/// What the bundle exports, in a form a downstream tool can read without
/// having to import the bundle itself.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BundleManifest {
    /// Framework name (`"preact"` / `"react"`) the bundle was built for.
    /// Mirrors [`zfb_render::adapters::Adapter::name`] and is the same
    /// string the runtime adapter (T2) keys on to load the right
    /// render-to-string module.
    pub framework: String,
    /// JSX import source the bundler injected. Mirrors
    /// [`zfb_render::adapters::Adapter::jsx_import_source`].
    pub jsx_import_source: String,
    /// Synthetic `zfb:internal/...` specifier that **identifies** the
    /// hydration shim. The bundle itself does not import
    /// the shim under this specifier (the CLI cannot resolve URL
    /// schemes; we use a relative import internally). Consumers (T6,
    /// docs) use this string for tracing / diagnostics.
    pub hydrate_shim_specifier: String,
    /// Filename of the bundle on disk (matches `bundle_path.file_name()`).
    pub bundle_basename: String,
    /// Routes the bundle serves, in `pages_dir` walk order.
    pub routes: Vec<RouteEntry>,
}

/// One route the bundle handles.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteEntry {
    /// URL route, derived from the page's path under `pages_dir` with
    /// the file extension stripped and `index` collapsed to the
    /// directory's path. Examples:
    ///
    /// - `pages/index.tsx`        → `/`
    /// - `pages/about.tsx`        → `/about`
    /// - `pages/blog/index.tsx`   → `/blog`
    /// - `pages/blog/[slug].tsx`  → `/blog/[slug]`
    /// - `pages/post.mdx`         → `/post`
    pub route: String,
    /// Source path of the page module, relative to `project_root`.
    pub source_path: PathBuf,
    /// Hono-form filter key used in the bundle's `routes` object literal
    /// and matched against `BundlerInput::worker_only_routes`. Equal to
    /// `bracket_to_hono(&route)` — bracket syntax (`/blog/[slug]`) is
    /// normalised to Hono syntax (`/blog/:slug`) so the
    /// `filter.contains(&r.entry_key)` check matches `worker_only_routes`
    /// (which is also Hono-form). Kept as a separate field so the filter
    /// contract can evolve independently of `route`.
    pub entry_key: String,
    /// When `true`, this route was produced from a `.html` source file.
    /// It is recorded in the manifest for plugin consumers but is NOT
    /// compiled into the JS bundle — it bypasses esbuild and V8 render
    /// entirely. The build pipeline copies the source body verbatim.
    #[serde(default)]
    pub static_html: bool,
    /// Path of the page module **relative to `pages_dir`** (the
    /// materialise walk's `rel`) — e.g. `docs/intro.tsx`, `index.tsx`.
    /// This is the load-bearing input to the entry-module's per-route
    /// import (`import … "./pages/<rel>"`, forward-slashed at the emit):
    /// it is carried straight from the walk so the import is correct
    /// regardless of where `pages_dir` physically lives (the real
    /// `project_root/pages`, #1518's private empty dev root, OR a per-build
    /// overlay temp dir under package-owned routes). Deriving it instead from
    /// `source_path` via
    /// a literal `pages/`-prefix strip silently collapsed nested overlay
    /// routes to a bare filename (issue #1193) — that path is retired.
    /// `#[serde(default)]` keeps older manifests deserialisable; an empty
    /// value falls back to the legacy `source_path` derivation at the
    /// import site.
    #[serde(default)]
    pub rel_under_pages: PathBuf,
}

const SHADOW_HYDRATE_FILENAME: &str = "__zfb_internal_hydrate.jsx";
const SHADOW_ENTRY_FILENAME: &str = "entry.mjs";
const SHADOW_TSCONFIG_FILENAME: &str = "tsconfig.json";
/// Server-only runtime subpath the generated `entry.mjs` imports `createPageRouter`
/// from. Declared once so `write_entry_module`'s emitted import and the
/// staged-dependency seed set (issue #1645) cannot drift: with `bundle.exclude`
/// active this package is staged solely because it is seeded here, and it is
/// seeded from the same constant the entry actually imports.
const ZFB_RUNTIME_SERVER_SPECIFIER: &str = "@takazudo/zfb-runtime/server";
/// Top-level directory names either handled by a dedicated materialise pass
/// (`pages`/`content`/`components`/`layouts`) or that are never project source
/// (`node_modules`, build output, VCS). Used both to enumerate the "extra"
/// top-level source dirs and to bound the staged-dependency module-graph seed
/// walk (issue #1645).
const KNOWN_SOURCE_DIRS: &[&str] = &[
    "pages",
    "content",
    "components",
    "layouts",
    "node_modules",
    "dist",
    ".git",
    "target",
    ".turbo",
    ".next",
    ".vercel",
];
/// Infra directories never mirrored in a wholesale sibling copy (issue #1692).
/// This is `KNOWN_SOURCE_DIRS` minus the four project-owned source-dir names
/// (`pages`/`content`/`components`/`layouts`): a workspace-sibling library may
/// legitimately own a directory named `pages`/`content`/etc., so those are NOT
/// pruned when mirroring a sibling region — only genuine build/VCS/vendor
/// output is. `node_modules` is pruned here AND enforced again by the
/// per-file `path_is_inside_node_modules` guard (defense in depth).
const MIRROR_SKIP_DIRS: &[&str] = &[
    "node_modules",
    "dist",
    ".git",
    "target",
    ".turbo",
    ".next",
    ".vercel",
];
/// First-party dot-path staging allowlist (issue #1840): project-relative
/// DIRECTORIES whose whole subtree is staged into the shadow even though every
/// generic walker skips them (hidden at the top level, and conventionally
/// gitignored — `enumerate_extra_top_level_dirs`'s `standard_filters` honors
/// the consumer's `.gitignore`, so removing the dot-filter alone could never
/// reach these). zudo-doc 4.x's `packageOwnedRoutes` mechanism generates REAL
/// first-party route source into `<project>/.zudo-doc/routes-src/*.tsx`;
/// without a staged spelling esbuild resolves the LIVE files via the
/// dual-target tsconfig fallback and the (correct) stage-escape audit
/// hard-fails with case 4 ("first-party input resolved outside every stage
/// root, no staged spelling"). Staging the narrow compatibility surface makes
/// the staged spelling exist so the audit's in-stage rule allows it naturally
/// — the metafile stays the oracle; the audit itself is untouched.
///
/// Deliberately NOT the whole dot-dir (`.zudo-doc/` may hold caches / private
/// metadata), and NOT a permanent contract — this is compatibility for the
/// CURRENT zudo-doc 4.x mechanism; zfb's long-term plan is real injected
/// routes (see `crates/zfb/tests/build_package_routes_consumer.rs`).
const KNOWN_FIRST_PARTY_STAGING_DIRS: &[&str] = &[".zudo-doc/routes-src"];
/// Companion to [`KNOWN_FIRST_PARTY_STAGING_DIRS`] (issue #1840): dot-dirs
/// whose TOP-LEVEL `*.json` meta files are staged — zudo-doc's docHistory
/// writes `<project>/.zfb/doc-history-meta.json`, imported by the generated
/// route source above. Only depth-1 `*.json` files; anything else in the dir
/// (caches, lockfiles, nested state) never reaches the stage. The staged
/// basenames don't collide with `is_synthetic_input` (metafile_deps.rs — the
/// `.zfb-` BASENAME prefix; `.zfb` here is a directory component) nor with
/// `is_reserved_shadow_root_name` (reserved `.zfb-*` FILE names at the shadow
/// root, not a `.zfb/` dir).
const KNOWN_FIRST_PARTY_STAGING_JSON_DIRS: &[&str] = &[".zfb"];
/// Filename of the project-root global override map (sub-issue #616). Both
/// the on-disk source convention and the materialised shadow copy use this
/// exact name; it is the public file-convention contract relied on by #618.
const MDX_COMPONENTS_FILENAME: &str = "mdx-components.tsx";

/// Esbuild `--loader:` flags for the Worker bundle.
///
/// - `.mdx=jsx` — `.mdx` files were rewritten to JSX text by
///   `materialise_shadow`; tell esbuild to parse them as JSX so the
///   `.mdx` extension keeps working for user import paths.
/// - `.md=jsx` — `.md` files are now routed through the same
///   `compile_mdx_to_jsx_module_cached` path as `.mdx` files (plain
///   CommonMark is a strict MDX subset). The shadow file retains the
///   `.md` extension; this loader tells esbuild to parse it as JSX so
///   the specifier and loader extension agree (zfb#405).
/// - `.css=empty` — plain `.css` imports inside JS modules are
///   converted to no-op modules at compile time. The Worker bundle
///   must NOT carry user CSS bytes — `ProductionAssetPipeline` writes
///   the real hashed `dist/assets/styles-<hash>.css` from
///   `CssPipeline::build_emitter` (S2) and the renderer injects a
///   `<link rel=stylesheet>` pointing at that file (S1+S4). With
///   esbuild's default `.css` loader the import would either (a) emit
///   a sibling `_zfb_inner.css` next to the worker bundle that nothing
///   references, or (b) inline the CSS, both inflating the Worker
///   bundle with bytes that are already shipped externally.
///   `loader=empty` substitutes an empty exports object at compile
///   time, so no runtime `import "...css"` statement is left behind to
///   crash the Worker at module load. The alternative
///   `--external:*.css` was REJECTED because esbuild can leave runtime
///   `import` statements that workerd cannot resolve.
/// - `.module.css=js` — CSS Modules support. A `.module.css` file
///   imported as `import styles from "./x.module.css"` must yield the
///   *scoped class-name map*, not empty. The bundler rewrites every
///   `.module.css` file in the shadow tree — including those placed
///   there as symlinks by `materialise_shadow` / `symlink_or_copy`
///   (fix #553) — to a JS module
///   (`export default { "orig": "scoped", … }`) using the maps in
///   [`BundlerInput::css_module_class_maps`]; this loader tells
///   esbuild to parse that rewritten file as JS. esbuild matches the
///   **longest** file extension, so `.module.css` wins over `.css`
///   here — plain `.css` still routes to `=empty`. The scoped CSS
///   itself is shipped externally via `styles-<hash>.css` exactly
///   like Tailwind output. When a `.module.css` file has no map entry
///   (e.g. the CSS pipeline was not run or a deep import was missed),
///   `rewrite_css_modules_in_shadow` writes `export default {};` —
///   a graceful degradation: `styles.foo` evaluates to `undefined`
///   and the build succeeds rather than crashing with a parse error.
/// - `.wasm=copy` — preserves a runtime ESM import while copying the Wasm
///   module beside the Worker bundle. `--external:*.wasm` is intentionally not
///   used: its relative path would be anchored to the ephemeral shadow tree,
///   not the deployable bundle directory.
pub const ESBUILD_LOADER_ARGS: &[&str] = &[
    "--loader:.mdx=jsx",
    "--loader:.md=jsx",
    "--loader:.css=empty",
    "--loader:.module.css=js",
    "--loader:.wasm=copy",
];

#[derive(Debug, Clone)]
struct ShadowParentEnv {
    temp_dir: PathBuf,
    xdg_cache_home: Option<OsString>,
    home: Option<OsString>,
    #[cfg_attr(not(windows), allow(dead_code))]
    local_app_data: Option<OsString>,
}

impl ShadowParentEnv {
    fn from_process() -> Self {
        Self {
            temp_dir: std::env::temp_dir(),
            xdg_cache_home: std::env::var_os("XDG_CACHE_HOME"),
            home: std::env::var_os("HOME"),
            local_app_data: std::env::var_os("LOCALAPPDATA"),
        }
    }
}

/// Return a tempdir parent that is guaranteed not to live inside the project.
pub fn shadow_parent_dir(project_root: &Path) -> Result<PathBuf> {
    shadow_parent_dir_with_env(project_root, &ShadowParentEnv::from_process())
}

fn shadow_parent_dir_with_env(project_root: &Path, env: &ShadowParentEnv) -> Result<PathBuf> {
    let mut rejected = Vec::new();

    if let Some(parent) = usable_shadow_parent_candidate(
        project_root,
        &env.temp_dir,
        "system temp dir",
        &mut rejected,
    )? {
        return Ok(parent);
    }

    match env.xdg_cache_home.as_ref().map(PathBuf::from) {
        Some(xdg) if xdg.is_absolute() => {
            let candidate = xdg.join("zfb");
            if let Some(parent) = usable_shadow_parent_candidate(
                project_root,
                &candidate,
                "XDG_CACHE_HOME/zfb",
                &mut rejected,
            )? {
                return Ok(parent);
            }
        }
        Some(xdg) => rejected.push(format!("XDG_CACHE_HOME is not absolute: {}", xdg.display())),
        None => rejected.push("XDG_CACHE_HOME is unset".to_string()),
    }

    #[cfg(windows)]
    {
        match env.local_app_data.as_ref().map(PathBuf::from) {
            Some(local_app_data) if local_app_data.is_absolute() => {
                // Best effort: LOCALAPPDATA is the Windows per-user cache root
                // closest to Unix's XDG/HOME cache locations.
                let candidate = local_app_data.join("zfb");
                if let Some(parent) = usable_shadow_parent_candidate(
                    project_root,
                    &candidate,
                    "LOCALAPPDATA/zfb",
                    &mut rejected,
                )? {
                    return Ok(parent);
                }
            }
            Some(local_app_data) => rejected.push(format!(
                "LOCALAPPDATA is not absolute: {}",
                local_app_data.display()
            )),
            None => rejected.push("LOCALAPPDATA is unset".to_string()),
        }
    }

    #[cfg(not(windows))]
    {
        match env.home.as_ref().map(PathBuf::from) {
            Some(home) if home.is_absolute() => {
                let candidate = home.join(".cache").join("zfb");
                if let Some(parent) = usable_shadow_parent_candidate(
                    project_root,
                    &candidate,
                    "HOME/.cache/zfb",
                    &mut rejected,
                )? {
                    return Ok(parent);
                }
            }
            Some(home) => rejected.push(format!("HOME is not absolute: {}", home.display())),
            None => rejected.push("HOME is unset".to_string()),
        }
    }

    bail!(
        "bundler: could not find a shadow tempdir parent outside project root {}. \
         Tried: {}. Set TMPDIR/TEMP, XDG_CACHE_HOME, HOME, or LOCALAPPDATA to a \
         directory outside the project tree.",
        project_root.display(),
        rejected.join("; ")
    )
}

fn usable_shadow_parent_candidate(
    project_root: &Path,
    candidate: &Path,
    label: &str,
    rejected: &mut Vec<String>,
) -> Result<Option<PathBuf>> {
    if let Err(error) = fs::create_dir_all(candidate) {
        rejected.push(format!(
            "{label} {} could not be created: {error}",
            candidate.display()
        ));
        return Ok(None);
    }

    match path_is_inside_project(project_root, candidate) {
        Ok(true) => {
            rejected.push(format!(
                "{label} {} resolves inside project root {}",
                candidate.display(),
                project_root.display()
            ));
            Ok(None)
        }
        Ok(false) => {
            let canonical = fs::canonicalize(candidate).with_context(|| {
                format!(
                    "{label} {} was proven outside project root {} but could not be canonicalized",
                    candidate.display(),
                    project_root.display()
                )
            })?;
            Ok(Some(canonical))
        }
        Err(error) => {
            rejected.push(format!(
                "{label} {} could not be proven outside project root {}: {error}",
                candidate.display(),
                project_root.display()
            ));
            Ok(None)
        }
    }
}

fn path_is_inside_project(project_root: &Path, candidate: &Path) -> Result<bool> {
    let project_canonical = fs::canonicalize(project_root);
    let candidate_canonical = fs::canonicalize(candidate);
    if let (Ok(project), Ok(candidate)) = (&project_canonical, &candidate_canonical) {
        return Ok(candidate.starts_with(project));
    }

    let project_lexical = normalize_path_lexical(project_root);
    let candidate_lexical = normalize_path_lexical(candidate);
    if candidate_lexical.starts_with(&project_lexical) {
        return Ok(true);
    }

    let project_error = project_canonical
        .as_ref()
        .err()
        .map(ToString::to_string)
        .unwrap_or_else(|| "ok".to_string());
    let candidate_error = candidate_canonical
        .as_ref()
        .err()
        .map(ToString::to_string)
        .unwrap_or_else(|| "ok".to_string());
    Err(anyhow!(
        "canonicalization failed (project_root: {project_error}; candidate: {candidate_error})"
    ))
}

fn ensure_shadow_path_outside_project(
    project_root: &Path,
    path: &Path,
    invariant_name: &str,
) -> Result<()> {
    match path_is_inside_project(project_root, path) {
        Ok(false) => Ok(()),
        Ok(true) => bail!(
            "bundler invariant violation: {invariant_name} must be outside project_root; \
             got {} inside {}",
            path.display(),
            project_root.display()
        ),
        Err(error) => bail!(
            "bundler invariant violation: {invariant_name} must be provably outside \
             project_root; could not verify {} against {}: {error}",
            path.display(),
            project_root.display()
        ),
    }
}

fn esbuild_loader_args(input: &BundlerInput) -> impl Iterator<Item = &str> {
    ESBUILD_LOADER_ARGS
        .iter()
        .copied()
        .chain(input.extra_loader_args.iter().map(String::as_str))
}

fn bundle_mode_define_args(mode: BundleMode) -> [String; 3] {
    let prod = mode.is_prod();
    let node_env = if prod { "production" } else { "development" };
    [
        format!("--define:import.meta.env.PROD={prod}"),
        format!("--define:import.meta.env.DEV={}", !prod),
        format!("--define:process.env.NODE_ENV=\"{node_env}\""),
    ]
}

fn operator_define_args(define_vars: &BTreeMap<String, String>) -> Vec<String> {
    define_vars
        .iter()
        .map(|(key, value)| format!("--define:{key}={value}"))
        .collect()
}

fn public_env_define_args(
    public_env_vars: &HashMap<String, String>,
    operator_define_vars: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut entries: Vec<(&String, &String)> = public_env_vars.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));

    let mut args = Vec::with_capacity(entries.len() * 2);
    for (key, value) in entries {
        if !key.starts_with("PUBLIC_") {
            continue;
        }

        let json_value = json_str(value);
        for expression in [
            format!("process.env.{key}"),
            format!("import.meta.env.{key}"),
        ] {
            // `bundle.define` is the explicit operator-authored channel. It
            // wins over generated PUBLIC values for the exact expression so
            // SSR agrees with the browser bundlers, which receive the
            // operator definitions but not this generated PUBLIC map.
            if !operator_define_vars.contains_key(&expression) {
                args.push(format!("--define:{expression}={json_value}"));
            }
        }
    }
    args
}

/// Default release-tarball slot for the esbuild binary. Mirrors
/// `zfb_islands::EsbuildSubprocessConfig::default`'s default — kept in
/// sync deliberately, both crates resolve the same slot.
///
/// This is the canonical definition; `crates/zfb/src/config.rs` formerly
/// kept a private duplicate that has been removed in favour of this one.
pub const DEFAULT_ESBUILD_SLOT: &str = "crates/zfb/binaries/esbuild/esbuild";

/// `ZFB_DEV_TIMING` gate for the per-call [`bundle`] phase-split line
/// (issue #993 Step 0 — extends the #991 instrumentation INSIDE the
/// bundler). Same env var and truthy parser as the dev-tick timing in
/// `crates/zfb/src/commands/dev.rs::dev_timing_enabled` so one flag turns
/// on the whole timing story. Unset/empty/unrecognized → off, and every
/// `Instant::now()` in [`bundle`] is behind the flag (zero hot-path cost).
fn bundler_timing_enabled() -> bool {
    std::env::var("ZFB_DEV_TIMING")
        .ok()
        .as_deref()
        .map(|raw| {
            let t = raw.trim();
            t.eq_ignore_ascii_case("1") || t.eq_ignore_ascii_case("true")
        })
        .unwrap_or(false)
}

/// Persistent dev shadow-tree session (issue #993).
///
/// A plain [`bundle`] call materialises a fresh shadow tempdir, re-writes
/// every source file into it (~hundreds of creates/writes on a docs-sized
/// site), and recursively deletes the whole tree at scope exit — pure
/// filesystem churn when most bytes are unchanged between dev ticks. A
/// `ShadowSession` keeps ONE shadow tempdir alive for the whole dev
/// process and lets [`bundle_with_session`] skip byte-identical rewrites
/// ("compute always, write only if changed" — spec locked by #992 on the
/// #987 epic).
///
/// Safety model:
///
/// - **No computation is ever skipped.** Every materialise walk, MDX
///   compile (cache-hit), `import.meta.glob` expansion, CSS-modules
///   rewrite, diagnostics drain, and link gate runs exactly as in a fresh
///   [`bundle`]. Glob expansion and transclude output depend on *other*
///   files, so any stat-based computation skipping would be unsound. Only
///   the final `fs::write` of byte-identical content is elided — the
///   shadow tree (and therefore the bundle bytes, and therefore the #940
///   skip key) stays byte-for-byte what a fresh build would produce.
/// - **Stale files are pruned** before esbuild runs: every path written
///   by the previous call but not visited by this one is deleted, so a
///   deleted/renamed/newly-excluded source can never leave a stale module
///   in the bundle (same wrong-output hazard family as #727).
/// - **Dirty-reset on error** (false-invalidate, never false-reuse — the
///   #940 spirit): `dirty` is armed on entry and cleared only on success.
///   A session whose last call failed wipes the shadow dir and
///   materialises from scratch, so a half-updated tree can never feed
///   esbuild.
/// - **Path-type flips heal in place**: a source path whose kind changes
///   between calls (directory→file or file→directory) clears the stale
///   shadow entry at write/mkdir time — recursively for directories,
///   invalidating every `written` hash beneath — so the session succeeds
///   on the same call a fresh [`bundle`] would, instead of erroring into
///   the dirty-reset. Directories are never recorded in `visited` (the
///   prune pass stays file-based); the prune tolerates a stale file path
///   that a live directory has replaced.
///
/// `zfb build` keeps calling [`bundle`], which passes no session — the
/// production path is byte-for-byte unchanged.
pub struct ShadowSession {
    /// The persistent shadow tempdir — lives as long as the session.
    work: tempfile::TempDir,
    /// SHA-256 of the last-written bytes per shadow-relative path. Only
    /// real files written through [`ShadowWriter`] are recorded; symlinks
    /// and the always-write infra files (entry.mjs / shim / tsconfig)
    /// are not.
    written: HashMap<PathBuf, [u8; 32]>,
    /// Shadow-relative paths visited by the previous call's materialise
    /// passes — the prune baseline.
    prev_visited: HashSet<PathBuf>,
    /// `true` while a call is in flight or after a failed call; cleared
    /// only when a call returns `Ok`.
    dirty: bool,
    /// `copy_mode` of the last call. A mode flip invalidates the whole
    /// tree — symlink-mode and copy-mode materialise the same source
    /// differently, so reusing across the flip would mix the two.
    copy_mode: Option<bool>,
    /// Incremental content-materialise skip cache (zfb#1148), keyed by
    /// the DESTINATION shadow-relative path. A content `.md`/`.mdx` whose
    /// stored source `(mtime, size)` and every recorded dependency are
    /// unchanged reuses its cached bridge import and shadow file instead
    /// of re-reading / re-compiling / re-writing.
    ///
    /// Keyed by DEST (not source) because the SAME source `.mdx` is
    /// materialised into two distinct shadow dests each tick — once via
    /// `materialise_collection` (`content/<name>/foo.mdx`, with a bridge
    /// import) and once via the extra-top-level-dir `materialise_shadow`
    /// walk of `src/` (`src/mdx/foo.mdx`, no bridge import). Dest-keying
    /// gives each pass its own independent entry so both can skip.
    ///
    /// MUST be cleared whenever the session wipes the shadow tree (dirty
    /// or copy_mode flip): the entries describe shadow files that the
    /// wipe just deleted, so a stale entry would reuse a vanished file.
    /// Cleared alongside `written` / `prev_visited` in [`ShadowWriter::new`].
    content_skip: HashMap<PathBuf, ContentSkipEntry>,
    /// Incremental NON-MDX source/asset skip cache (zfb#1148), keyed by
    /// the DESTINATION shadow-relative path (same rationale as
    /// `content_skip` — the same source can land at two dests). Lets a
    /// later tick skip the plain copy/symlink of a source or asset file
    /// whose own `(mtime, size)` is unchanged — the dominant cost of the
    /// extra-top-level-dir pass over large ancillary trees (`doc/`,
    /// `sub-packages/`, `static/`, …), which `materialise_source_file`
    /// otherwise re-copies/re-symlinks every tick.
    ///
    /// A plain copy/symlink is a pure function of the file's own bytes, so
    /// an unchanged `(mtime, size)` is an exact skip. Files using
    /// `import.meta.glob` are NEVER skipped (`has_glob` gate) — their
    /// expansion depends on the live project tree, so they re-expand every
    /// tick, preserving glob add/remove soundness.
    ///
    /// Cleared on the same dirty/copy_mode wipe as `content_skip`.
    source_skip: HashMap<PathBuf, SourceSkipEntry>,
    /// Incremental sibling-mirror skip cache (issue #1777), keyed by the
    /// DESTINATION shadow-relative path — same dest-keying rationale as
    /// `source_skip`. Lets a later tick skip the wholesale raw byte copy
    /// [`mirror_sibling_root`] performs for an unchanged sibling file.
    ///
    /// DELIBERATELY SEPARATE from `source_skip`, not a shared namespace:
    /// `mirror_sibling_root` runs BEFORE the preprocessing-materialise pass
    /// in the same tick, over the same dest paths a later
    /// `materialise_source_file` call for a reachable sibling `?raw`/glob/
    /// worker source can also target. If a mirror-skip entry lived in
    /// `source_skip`, a later same-tick `materialise_source_file` lookup on
    /// that dest could find it and wrongly skip — the mirror pass only ever
    /// proves the RAW bytes are unchanged, never that no preprocessing
    /// rewrite is owed (mirror copies are never glob/raw/worker-aware; see
    /// [`mirror_sibling_root`]). Keeping the caches apart means a mirror
    /// entry can never satisfy a materialise lookup or vice versa.
    ///
    /// Cleared on the same dirty/copy_mode wipe as `content_skip`.
    mirror_skip: HashMap<PathBuf, MirrorSkipEntry>,
    /// The pipeline `config_fingerprint` of the LAST successful call —
    /// the wipe trigger for a config/route-map change (zfb#1148, Defect
    /// A). Both skip caches reuse a file's previous compiled output, but
    /// `ResolveLinksPlugin` (resolve_markdown_links) rewrites links from an
    /// in-memory route→URL map and records NO deps, so a map change is
    /// invisible to the per-dep stat check. The fingerprint folds in the
    /// map digest (and every other compile-affecting knob), so comparing
    /// it on each call and wiping both skip caches on a mismatch keeps the
    /// skip cache consistent with what the MDX compile cache invalidates.
    /// `None` until the first call. Updated in [`ShadowWriter::new`].
    config_fingerprint: Option<String>,
}

impl ShadowSession {
    /// Allocate the persistent shadow tempdir for a dev session.
    pub fn new(project_root: &Path) -> Result<Self> {
        let parent = shadow_parent_dir(project_root)?;
        let work = tempfile::Builder::new()
            .prefix("zfb-shadow-session-")
            .tempdir_in(parent)
            .context("shadow session: failed to allocate persistent shadow tempdir")?;
        Ok(Self {
            work,
            written: HashMap::new(),
            prev_visited: HashSet::new(),
            dirty: false,
            copy_mode: None,
            content_skip: HashMap::new(),
            source_skip: HashMap::new(),
            mirror_skip: HashMap::new(),
            config_fingerprint: None,
        })
    }

    /// Root of the persistent shadow tree. Exposed for tests that assert
    /// on the materialised/pruned file set.
    pub fn shadow_root(&self) -> &Path {
        self.work.path()
    }
}

/// Remove every entry inside `dir` without removing `dir` itself.
/// Symlinked children (e.g. the `node_modules` link) are removed as
/// links, never followed into.
fn wipe_dir_contents(dir: &Path) -> Result<()> {
    let entries = fs::read_dir(dir).with_context(|| {
        format!(
            "shadow session: failed reading shadow dir {}",
            dir.display()
        )
    })?;
    for entry in entries {
        let entry = entry.with_context(|| {
            format!(
                "shadow session: failed walking shadow dir {}",
                dir.display()
            )
        })?;
        let ft = entry.file_type().with_context(|| {
            format!(
                "shadow session: failed to stat shadow entry {}",
                entry.path().display()
            )
        })?;
        let result = if ft.is_dir() {
            fs::remove_dir_all(entry.path())
        } else {
            // Files AND symlinks (symlink_metadata-based file_type reports
            // a symlink-to-dir as symlink, not dir — remove_file is right).
            fs::remove_file(entry.path())
        };
        result.with_context(|| {
            format!(
                "shadow session: failed to wipe shadow entry {}",
                entry.path().display()
            )
        })?;
    }
    Ok(())
}

/// Canonicalize the shadow tree root so esbuild emits byte-deterministic
/// source-path comments (issue #1006).
///
/// PLATFORM KNOWLEDGE (not recoverable from the code): on macOS the system
/// temp dir (`$TMPDIR`, where every shadow tempdir is allocated) lives under
/// `/var/folders/...`, and `/var` is a symlink to `/private/var`. esbuild
/// canonicalizes the OUTFILE path through the OS (so it lands on
/// `/private/var/...`) but takes the entry/source paths as given. When the
/// shadow root is the raw `/var/...` form, the outfile and the sources sit on
/// two different path roots (`/var` vs `/private/var`), so esbuild's
/// outbase-relative path math has to walk all the way up to `/` and back down
/// — embedding the full absolute tail (including the tempdir's random
/// basename) into each `// <path>` module comment. Two independent shadow
/// tempdirs (a persistent `ShadowSession` vs an ephemeral `bundle()` call)
/// have DIFFERENT basenames, so those comments diverge and the bundle bytes
/// differ — corrupting the #940 skip key, which hashes these bytes. Resolving
/// the symlink here puts the shadow on the same `/private/var` root the
/// outfile already resolves to, so esbuild emits clean relative comments with
/// no tempdir name. Linux's `/tmp` is not a symlink, so this is a no-op there
/// (which is why the divergence is macOS-only).
///
/// Gated to macOS: on Windows `fs::canonicalize` returns `\\?\`-prefixed
/// verbatim paths, which would newly reach esbuild's entry/outfile args on a
/// platform no CI runs tests on — and the underlying symlink divergence does
/// not exist there (`%TEMP%` is not a symlink).
#[cfg(target_os = "macos")]
fn canonical_shadow_root(work: &Path) -> Result<PathBuf> {
    fs::canonicalize(work).with_context(|| {
        format!(
            "bundler: failed to canonicalize shadow root {}",
            work.display()
        )
    })
}

#[cfg(not(target_os = "macos"))]
fn canonical_shadow_root(work: &Path) -> Result<PathBuf> {
    Ok(work.to_path_buf())
}

/// Conditional-write seam threaded through [`MaterialiseCtx`] to every
/// materialise call (issue #993).
///
/// In passthrough mode (`session: None` — prod builds and sessionless
/// callers) each method performs exactly the pre-#993 filesystem
/// operation. In session mode it skips byte-identical rewrites, records
/// each visited shadow-relative path for the prune pass, and maintains
/// the session's `written` hash map.
struct ShadowWriter<'s> {
    shadow_root: PathBuf,
    /// `None` → passthrough. `Some` wraps the borrowed session in a
    /// `RefCell` so the `&MaterialiseCtx` plumbing (shared refs) can
    /// still mutate the bookkeeping — all single-threaded within one
    /// bundle call.
    session: Option<RefCell<&'s mut ShadowSession>>,
    /// Shadow-relative paths visited by THIS call (session mode only).
    visited: RefCell<HashSet<PathBuf>>,
}

impl<'s> ShadowWriter<'s> {
    /// Build the writer; in session mode also performs the dirty /
    /// copy-mode-flip wipe and arms the dirty flag for this call.
    ///
    /// `config_fingerprint` is the effective pipeline's
    /// `config_fingerprint` for THIS call (zfb#1148, Defect A). When it
    /// differs from the session's stored value, both incremental-skip
    /// caches are cleared so a config/route-map change forces a full
    /// re-materialise — see the field doc on
    /// [`ShadowSession::config_fingerprint`].
    fn new(
        shadow_root: PathBuf,
        session: Option<&'s mut ShadowSession>,
        copy_mode: bool,
        config_fingerprint: Option<String>,
    ) -> Result<Self> {
        let session = match session {
            Some(s) => {
                if s.dirty || s.copy_mode != Some(copy_mode) {
                    // Previous call failed mid-flight (or the symlink/copy
                    // materialise mode flipped): the tree may be
                    // half-updated, so it must never feed esbuild —
                    // rebuild it from scratch this call.
                    wipe_dir_contents(s.work.path())?;
                    s.written.clear();
                    s.prev_visited.clear();
                    // The wipe just deleted every shadow file the skip
                    // cache describes — clearing it here is what keeps a
                    // wipe from ever leaving us reusing a vanished file
                    // (zfb#1148, rule 5).
                    s.content_skip.clear();
                    s.source_skip.clear();
                    s.mirror_skip.clear();
                }
                // Config/route-map change wipe (zfb#1148, Defect A): a
                // change to any compile-affecting knob — in particular the
                // `resolve_source_map` digest, which `ResolveLinksPlugin`
                // consumes WITHOUT recording any dep — alters a page's
                // rewritten URLs / content hash while its own
                // `(mtime, size)` is unchanged. The per-dep stat check
                // cannot see that, so without this a resolve-links page
                // would be wrongly skipped (stale URL → 404, snapshot↔
                // bridge hash desync → `<pre data-zfb-content-fallback>`,
                // and a suppressed `onBrokenLinks: 'error'`). Wiping both
                // skip caches on a fingerprint mismatch forces a full
                // re-materialise (the shadow tree itself is left intact —
                // every file simply re-materialises with no skip entry and
                // `write_if_changed` overwrites what changed). `None` (an
                // uncacheable pipeline) is treated as "config unknown" and
                // always mismatches, so it never skips. Steady-state body
                // edits don't change the fingerprint, so normal skipping is
                // preserved.
                if s.config_fingerprint != config_fingerprint || config_fingerprint.is_none() {
                    s.content_skip.clear();
                    s.source_skip.clear();
                    s.mirror_skip.clear();
                    s.config_fingerprint = config_fingerprint;
                }
                s.copy_mode = Some(copy_mode);
                // Armed for the whole call; cleared by `mark_clean()` only
                // after esbuild succeeded. An early `?` return anywhere in
                // `bundle_with_session` leaves it set, forcing the wipe
                // above on the next call.
                s.dirty = true;
                Some(RefCell::new(s))
            }
            None => None,
        };
        Ok(Self {
            shadow_root,
            session,
            visited: RefCell::new(HashSet::new()),
        })
    }

    fn rel_of(&self, to: &Path) -> std::io::Result<PathBuf> {
        to.strip_prefix(&self.shadow_root)
            .map(|p| p.to_path_buf())
            .map_err(|_| {
                std::io::Error::other(format!(
                    "shadow writer: {} is not under shadow root {}",
                    to.display(),
                    self.shadow_root.display()
                ))
            })
    }

    /// Write `bytes` at `to`, skipping the write when the session already
    /// wrote identical bytes there. Records the path as visited.
    fn write_if_changed(&self, to: &Path, bytes: &[u8]) -> std::io::Result<()> {
        self.write_inner(to, bytes, true)
    }

    /// Variant for the in-place `.module.css` rewrite, which walks the
    /// whole (possibly stale) shadow tree: it must NOT mark paths
    /// visited — marking a stale file visited would shield it from the
    /// prune. Legitimate `.module.css` files were already visited by
    /// their materialise pass this call.
    fn write_if_changed_no_visit(&self, to: &Path, bytes: &[u8]) -> std::io::Result<()> {
        self.write_inner(to, bytes, false)
    }

    fn write_inner(&self, to: &Path, bytes: &[u8], mark_visited: bool) -> std::io::Result<()> {
        let Some(cell) = &self.session else {
            // Passthrough — pre-#993 semantics. The remove-first protects
            // against writing THROUGH a pre-existing symlink into the
            // user's source tree (#553); for the plain-write sites it is
            // a no-op ENOENT unlink (the prod shadow tempdir is fresh).
            let _ = fs::remove_file(to);
            return fs::write(to, bytes);
        };
        let rel = self.rel_of(to)?;
        if mark_visited {
            self.visited.borrow_mut().insert(rel.clone());
        }
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let hash: [u8; 32] = hasher.finalize().into();
        let mut session = cell.borrow_mut();
        match session.written.get(&rel) {
            // Identical bytes already on disk — the whole point: skip.
            Some(prev) if *prev == hash => Ok(()),
            // We wrote a (different) regular file here last time — plain
            // overwrite is safe.
            Some(_) => {
                fs::write(to, bytes)?;
                session.written.insert(rel, hash);
                Ok(())
            }
            // Unknown provenance (first write at this path, or the path
            // was last created as a symlink): remove first so we never
            // write THROUGH a symlink (#553).
            None => {
                // A stale DIRECTORY at this path (directory→file source
                // mutation during the dev session) would make the write
                // fail until the next dirty wipe — a fresh bundle() would
                // succeed immediately. Remove it recursively and drop
                // every `written` hash beneath it, or a later write at a
                // descendant path could be wrongly skipped.
                if fs::symlink_metadata(to).is_ok_and(|m| m.is_dir()) {
                    fs::remove_dir_all(to)?;
                    session.written.retain(|p, _| !p.starts_with(&rel));
                }
                let _ = fs::remove_file(to);
                fs::write(to, bytes)?;
                session.written.insert(rel, hash);
                Ok(())
            }
        }
    }

    /// Copy `from` to `to` through the write-if-changed logic (reads are
    /// cheap relative to writes — the dev snapshot walk already reads
    /// every content file each tick).
    fn copy_if_changed(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        if self.session.is_none() {
            let _ = fs::remove_file(to);
            return fs::copy(from, to).map(|_| ());
        }
        let bytes = fs::read(from)?;
        self.write_if_changed(to, &bytes)
    }

    /// Symlink `target` at `to`, re-creating only when the link is
    /// missing or points elsewhere. Records the path as visited.
    fn symlink_if_absent(&self, target: &Path, to: &Path) -> std::io::Result<()> {
        let Some(cell) = &self.session else {
            return symlink_or_copy(target, to);
        };
        let rel = self.rel_of(to)?;
        self.visited.borrow_mut().insert(rel.clone());
        if let Ok(existing) = fs::read_link(to) {
            if existing == target {
                return Ok(());
            }
        }
        // A stale DIRECTORY at this path (directory→file source mutation
        // during the dev session) would make the symlink creation fail
        // until the next dirty wipe. Remove it recursively and drop every
        // `written` hash beneath it (see the matching block in
        // `write_inner`). `symlink_metadata` reports a symlink-to-dir as
        // a symlink, so only real directories take this branch — links
        // are handled by `symlink_or_copy`'s remove-first.
        if fs::symlink_metadata(to).is_ok_and(|m| m.is_dir()) {
            fs::remove_dir_all(to)?;
            cell.borrow_mut()
                .written
                .retain(|p, _| !p.starts_with(&rel));
        }
        // (Re-)creating the link invalidates any recorded content hash:
        // the path's provenance is now "symlink", not "bytes we wrote",
        // so a later write_if_changed must take the remove-first branch.
        cell.borrow_mut().written.remove(&rel);
        symlink_or_copy(target, to)
    }

    /// Create the directory at `to` (and any missing parents). In session
    /// mode a stale non-directory entry at `to` — a regular file, or a
    /// symlink (which `create_dir_all` would FOLLOW, so a symlink whose
    /// source path became a directory would alias the live source tree
    /// and let later child writes escape the shadow — the #553 hazard) —
    /// left by a previous call's file→directory source mutation is
    /// removed first and its `written` hash dropped. Directories are NOT
    /// recorded as visited: the prune pass stays file-based, and a stale
    /// dir is instead cleared lazily by whichever later call needs a
    /// file (`write_inner` / `symlink_if_absent`) or dir (here) at its
    /// path. Passthrough mode is the plain pre-#993 `create_dir_all`
    /// (the prod shadow tempdir is fresh, so no conflict can exist).
    fn ensure_dir(&self, to: &Path) -> std::io::Result<()> {
        if let Some(cell) = &self.session {
            if fs::symlink_metadata(to).is_ok_and(|m| !m.is_dir()) {
                // Validate the path is shadow-relative BEFORE the
                // destructive removal (rel_of rejects out-of-shadow
                // paths — none exist today, but never delete first).
                let rel = self.rel_of(to)?;
                fs::remove_file(to)?;
                cell.borrow_mut().written.remove(&rel);
            }
        }
        fs::create_dir_all(to)
    }

    /// Whether this writer is in session mode (a persistent dev
    /// [`ShadowSession`] is attached). The content-file skip cache
    /// (zfb#1148) only ever engages in session mode; passthrough
    /// (`bundle()` / prod) always takes the full materialise path, so
    /// production builds stay byte-for-byte unchanged.
    fn in_session(&self) -> bool {
        self.session.is_some()
    }

    /// Record a shadow path as visited THIS call WITHOUT writing it —
    /// the prune-pass seam the content skip-cache needs (zfb#1148, rule
    /// 4). A skipped content file does not re-write its shadow JSX, so
    /// without this it would land in `prev_visited − visited` and be
    /// pruned, breaking the bundle. No-op in passthrough mode (the prune
    /// pass only runs in session mode).
    fn record_visited(&self, to: &Path) -> std::io::Result<()> {
        if self.session.is_some() {
            let rel = self.rel_of(to)?;
            self.visited.borrow_mut().insert(rel);
        }
        Ok(())
    }

    /// Look up a content skip-cache entry by its DESTINATION
    /// shadow-relative path (zfb#1148). The dest path is the key (not the
    /// source) so the same source materialised into two distinct shadow
    /// dests — e.g. `content/<name>/foo.mdx` via a collection AND
    /// `src/mdx/foo.mdx` via the extra-top-level-dir walk — gets two
    /// independent entries. Returns a clone if present (session mode
    /// only). The caller validates the stored source's `(mtime, size)`,
    /// every dep's state, and the dest-file existence before honouring it.
    fn content_skip_get(&self, dest_rel: &Path) -> Option<ContentSkipEntry> {
        let cell = self.session.as_ref()?;
        cell.borrow().content_skip.get(dest_rel).cloned()
    }

    /// Insert/update a content skip-cache entry keyed by its DESTINATION
    /// shadow-relative path (session mode only; no-op in passthrough).
    fn content_skip_store(&self, dest_rel: PathBuf, entry: ContentSkipEntry) {
        if let Some(cell) = &self.session {
            cell.borrow_mut().content_skip.insert(dest_rel, entry);
        }
    }

    /// Drop a content skip-cache entry by its DESTINATION shadow-relative
    /// path (session mode only; no-op in passthrough). Called when a file
    /// is recompiled but must not be cached (e.g. the defensive
    /// broken-JSX skip), so a later tick never false-reuses a stale entry.
    fn content_skip_remove(&self, dest_rel: &Path) {
        if let Some(cell) = &self.session {
            cell.borrow_mut().content_skip.remove(dest_rel);
        }
    }

    /// Look up a NON-MDX source/asset skip entry by its DESTINATION
    /// shadow-relative path (zfb#1148; session mode only). The caller
    /// validates the stored source `(mtime, size)`, the `!has_glob` gate,
    /// and the dest existence before honouring it.
    fn source_skip_get(&self, dest_rel: &Path) -> Option<SourceSkipEntry> {
        let cell = self.session.as_ref()?;
        cell.borrow().source_skip.get(dest_rel).cloned()
    }

    /// Insert/update a NON-MDX source/asset skip entry keyed by its
    /// DESTINATION shadow-relative path (session mode only; no-op in
    /// passthrough).
    fn source_skip_store(&self, dest_rel: PathBuf, entry: SourceSkipEntry) {
        if let Some(cell) = &self.session {
            cell.borrow_mut().source_skip.insert(dest_rel, entry);
        }
    }

    /// Drop a NON-MDX source/asset skip entry by its DESTINATION
    /// shadow-relative path (session mode only; no-op in passthrough).
    /// Called when the file's own stat cannot be taken — no sound skip key.
    fn source_skip_remove(&self, dest_rel: &Path) {
        if let Some(cell) = &self.session {
            cell.borrow_mut().source_skip.remove(dest_rel);
        }
    }

    /// Look up a sibling-mirror skip-cache entry by its DESTINATION
    /// shadow-relative path (issue #1777; session mode only). Deliberately
    /// a SEPARATE namespace from `source_skip_get` — see the field doc on
    /// [`ShadowSession::mirror_skip`] for why the two caches must never
    /// share entries. The caller validates the stored source `(mtime,
    /// size)` and the dest existence before honouring the skip.
    fn mirror_skip_get(&self, dest_rel: &Path) -> Option<MirrorSkipEntry> {
        let cell = self.session.as_ref()?;
        cell.borrow().mirror_skip.get(dest_rel).cloned()
    }

    /// Insert/update a sibling-mirror skip-cache entry keyed by its
    /// DESTINATION shadow-relative path (session mode only; no-op in
    /// passthrough).
    fn mirror_skip_store(&self, dest_rel: PathBuf, entry: MirrorSkipEntry) {
        if let Some(cell) = &self.session {
            cell.borrow_mut().mirror_skip.insert(dest_rel, entry);
        }
    }

    /// Drop a sibling-mirror skip-cache entry by its DESTINATION
    /// shadow-relative path (session mode only; no-op in passthrough).
    /// Called when the file's own stat cannot be taken after mirroring —
    /// no sound skip key.
    fn mirror_skip_remove(&self, dest_rel: &Path) {
        if let Some(cell) = &self.session {
            cell.borrow_mut().mirror_skip.remove(dest_rel);
        }
    }

    /// Delete stale shadow files (`prev_visited − visited`) — MUST run
    /// before esbuild so the bundle can never include a module a fresh
    /// build would not (#727 hazard family). Also commits this call's
    /// visited set as the next call's prune baseline.
    fn prune_stale(&self) -> Result<()> {
        let Some(cell) = &self.session else {
            return Ok(());
        };
        let mut session = cell.borrow_mut();
        let visited = std::mem::take(&mut *self.visited.borrow_mut());
        let stale: Vec<PathBuf> = session.prev_visited.difference(&visited).cloned().collect();
        for rel in stale {
            // Never prune the node_modules link (defense in depth — the
            // link bypasses the writer entirely, so it can never appear
            // in `prev_visited`; keep the guard in case that changes).
            if rel
                .components()
                .next()
                .is_some_and(|c| c.as_os_str() == "node_modules")
            {
                continue;
            }
            let abs = self.shadow_root.join(&rel);
            // Path-type flips (see the safety model on [`ShadowSession`])
            // leave stale entries the plain remove_file below would choke
            // on; discriminate via lstat first:
            match fs::symlink_metadata(&abs) {
                // file→dir flip: a materialise pass replaced the stale
                // file with a LIVE directory this call (`ensure_dir`
                // already deleted the file). Removing the dir would prune
                // freshly-written output — keep it, drop the bookkeeping.
                Ok(m) if m.is_dir() => {
                    session.written.remove(&rel);
                    continue;
                }
                Ok(_) => {}
                // Already gone: deleted with an ancestor directory
                // (dir→file flip removes whole subtrees — NotFound), or
                // an ancestor is now a regular file so the path can no
                // longer be traversed (NotADirectory). Nothing stale is
                // on disk; drop the bookkeeping.
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                    ) =>
                {
                    session.written.remove(&rel);
                    continue;
                }
                // Any other lstat failure: a stale file we cannot verify
                // gone would feed esbuild wrong input — hard error, same
                // self-healing contract as the remove_file arm below.
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!(
                            "shadow session: failed to stat stale shadow path {}",
                            abs.display()
                        )
                    });
                }
            }
            match fs::remove_file(&abs) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                // Any other failure is a hard error: a stale file we
                // could not remove would feed esbuild wrong input. The
                // error propagates, dirty stays armed, and the next call
                // wipes the tree (self-healing).
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!(
                            "shadow session: failed pruning stale shadow file {}",
                            abs.display()
                        )
                    });
                }
            }
            session.written.remove(&rel);
        }
        session.prev_visited = visited;
        Ok(())
    }

    /// Success epilogue — the shadow tree now exactly matches this call's
    /// inputs, so the next call may trust `written` / `prev_visited`.
    fn mark_clean(&self) {
        if let Some(cell) = &self.session {
            cell.borrow_mut().dirty = false;
        }
    }
}

/// Bundle the user's source tree into a single ESM file.
///
/// See the module-level documentation for the full pipeline. Production
/// path: materialises a fresh shadow tempdir per call (no session) —
/// byte-for-byte the pre-#993 behaviour.
pub fn bundle(input: BundlerInput) -> Result<BundlerOutput> {
    bundle_with_session(input, None)
}

/// [`bundle`] with an optional persistent dev [`ShadowSession`]
/// (issue #993). `None` is the production path; `Some` reuses the
/// session's shadow tree across calls, skipping byte-identical rewrites
/// and pruning stale files. See [`ShadowSession`] for the safety model.
pub fn bundle_with_session(
    input: BundlerInput,
    session: Option<&mut ShadowSession>,
) -> Result<BundlerOutput> {
    // 1. Resolve & validate.
    let resolver = PathResolver::new(&input.project_root);
    let pages_dir = resolver.resolve(&input.pages_dir);
    let content_dir = resolver.resolve(&input.content_dir);
    let components_dir = resolver.resolve(&input.components_dir);
    let layouts_dir = resolver.resolve(&input.layouts_dir);
    let outdir = resolver.resolve(&input.outdir);

    if !pages_dir.is_dir() {
        bail!(
            "bundler: pages_dir does not exist or is not a directory: {}",
            pages_dir.display()
        );
    }

    let adapter = make_adapter(input.framework);

    // `copy_mode` — when esbuild will run WITHOUT `--preserve-symlinks`
    // (branch 4: project node_modules + non-empty tsconfig paths), every
    // symlinked source file in the shadow tree is canonicalised by esbuild
    // back to the real project tree, so the in-shadow `import.meta.glob`
    // expansion and `.module.css` rewrite become invisible. In that mode
    // we materialise source files as REAL COPIES (not symlinks) so the
    // transformed shadow file is the one esbuild reads. Derived from the
    // SAME predicate that gates the `--preserve-symlinks` flag in
    // `run_esbuild`, so the two decisions cannot drift. `node_modules` is
    // always symlinked regardless (see the 2b block) — copy_mode only
    // affects source files.
    let copy_mode = !esbuild_will_preserve_symlinks(&input);

    // 1b. Build the resolve-links source map when the feature is enabled.
    //
    // The map is built once here (before the shadow walk) from every
    // configured route, then stored into the effective `PipelineSpec`
    // shared by all materialise calls (see the `mat_ctx` construction
    // below). This derivation step is why `resolve_markdown_links` stays
    // a separate bundler-side input rather than living on the spec: the
    // route dirs may be relative (resolved against `project_root` here)
    // and the spec only carries the pipeline-visible RESULT (zfb#917).
    //
    // Multi-route shape (sub #234) lets locale mirrors map to distinct
    // route prefixes — required for any project with EN+JA mirrors, or
    // any other multi-collection layout, so each mirror dir resolves
    // under its own route prefix (`/docs/` vs `/ja/docs/`).
    let resolve_source_map: Option<HashMap<std::path::PathBuf, String>> =
        input.resolve_markdown_links.as_ref().map(|spec| {
            let collections: Vec<CollectionRoute> = spec
                .routes
                .iter()
                .enumerate()
                .map(|(i, r)| CollectionRoute {
                    name: format!("routes[{i}]"),
                    dir: resolver.resolve(&r.docs_dir),
                    route_prefix: r.route_prefix.clone(),
                })
                .collect();
            build_docs_source_map(DocsSourceMapOptions { collections })
        });

    // Accumulated broken links across all materialise calls.
    // After the walk completes, handled according to `on_broken_links`.
    let mut all_broken_links: Vec<(String, String)> = Vec::new(); // (file_path, url)

    // Accumulated markdown diagnostics (transclude errors, imageDimensions
    // warnings, linkValidation findings) across all materialise calls.
    // Drained adjacent to `take_broken_links`; policy applied after ALL walks
    // complete — mirroring the broken-links gate above (zfb#953).
    let mut all_markdown_diagnostics: Vec<MarkdownDiagnostic> = Vec::new();

    // Cross-file anchor-check side channels (#980): cross-file fragment-link
    // candidates and per-file heading records drained from every materialise
    // call. Checked build-wide after ALL walks complete (gate 2c-anchor),
    // immediately before the markdown-diagnostics gate (2c).
    let mut all_cross_file_links: Vec<CrossFileLinkCandidate> = Vec::new();
    let mut all_file_headings: Vec<FileHeadings> = Vec::new();

    // Compile `bundle.exclude` once and share it across every
    // `materialise_shadow` call (pages / content / components / layouts /
    // extra top-level dirs). Empty patterns → a matcher that never matches,
    // so an unset `bundle.exclude` is byte-identical to a build without the
    // knob. An invalid glob is a hard, clearly-named build error.
    //
    // `with_workspace_root` arms the issue #1668 part 2 two-tier contract: in
    // a claimed pnpm workspace a sibling file matches patterns WORKSPACE-
    // relative while project files stay PROJECT-relative. Outside a workspace
    // it is a no-op (single-tier, byte-identical to pre-#1668).
    let bundle_exclude =
        BundleExcludeMatcher::new(&input.bundle_exclude)?.with_workspace_root(&input.project_root);

    // `ZFB_DEV_TIMING=1` — per-call phase split (issue #993 Step 0):
    // `materialise` (tempdir alloc + every materialise walk + diagnostics
    // gates + css rewrite + entry/shim/tsconfig writes), `esbuild` (the
    // subprocess), `post` (manifest assembly after the subprocess), and
    // `teardown` (the shadow TempDir's recursive delete, timed via an
    // explicit drop). One stderr line per successful call; error paths
    // print nothing (the failed tick is reported by the caller anyway).
    let timing_enabled = bundler_timing_enabled();

    // 2. Materialise the shadow tree.
    //
    // Sessionless (prod): a fresh tempdir per call, recursively deleted at
    // the end (`owned_work`). Session mode (#993): reuse the session's
    // persistent tempdir; `ShadowWriter::new` handles the dirty-wipe and
    // arms the dirty flag.
    let materialise_start = if timing_enabled {
        Some(std::time::Instant::now())
    } else {
        None
    };
    let (owned_work, work) = match &session {
        Some(s) => (None, canonical_shadow_root(s.work.path())?),
        None => {
            let parent = shadow_parent_dir(&input.project_root)?;
            let work = tempfile::Builder::new()
                .prefix("zfb-bundler-")
                .tempdir_in(parent)
                .context("bundler: failed to allocate shadow tempdir")?;
            let path = canonical_shadow_root(work.path())?;
            (Some(work), path)
        }
    };
    let work: &Path = &work;
    ensure_shadow_path_outside_project(&input.project_root, work, "bundler shadow root")?;

    // Issue #1668 part 1: re-root the SSR shadow at the widened first-party
    // boundary. The allocated tempdir (`work`) mirrors the whole first-party
    // tree — the workspace root in a pnpm workspace — and the PROJECT mirror
    // (`shadow`) is nested at the project's workspace-relative subpath. Every
    // downstream step keeps keying off `shadow` (entry.mjs / synthetic
    // tsconfig / hydration shim live there; esbuild's cwd is `shadow`), so
    // without a workspace `first_party_root == project_root`, `workspace_rel`
    // is empty, and `shadow == work` — a byte-identical no-op vs the pre-#1668
    // single-mirror layout. Sibling-package sources are NOT staged in this
    // part (the #1668 guard still escapes them); only the layout re-roots.
    let first_party_root = zfb_types::first_party_root_for(&input.project_root);
    let workspace_rel = normalize_path_lexical(&input.project_root)
        .strip_prefix(&first_party_root)
        .ok()
        .filter(|rel| !rel.as_os_str().is_empty())
        .map(Path::to_path_buf);
    let shadow_buf = match &workspace_rel {
        Some(rel) => work.join(rel),
        None => work.to_path_buf(),
    };
    let shadow: &Path = &shadow_buf;

    // The effective `PipelineSpec` is the input spec with its
    // `resolve_source_map` knob ALWAYS rewritten from the derivation
    // above (`Some(map)` when `resolve_markdown_links` is configured,
    // `None` otherwise) — the bundler owns that knob, so a caller-set
    // value on `input.pipeline_spec` can never desync from the route
    // spec (zfb#917).
    let effective_spec = {
        let mut spec = input.pipeline_spec.clone();
        spec.resolve_source_map = resolve_source_map;
        spec
    };

    // Pipeline config fingerprint for the incremental-skip wipe trigger
    // (zfb#1148, Defect A). The MDX compile cache keys every entry on this
    // `config_fingerprint` (see `Pipeline::config_fingerprint`), which
    // folds in EVERY compile-affecting knob — crucially the
    // `resolve_source_map` digest (`add_resolve_links`), whose
    // `ResolveLinksPlugin` rewrites `./other.mdx` links from an in-memory
    // route→URL map and records NOTHING through the ReadRecorder. So a
    // resolve-links page's recorded deps are EMPTY: a renamed/removed link
    // target changes the map (and the page's rewritten URLs + content
    // hash) while the page's own source `(mtime, size)` is unchanged. The
    // per-dep stat check cannot see that. Folding the fingerprint into the
    // session wipe makes a config/route-map change force a full
    // re-materialise (matching exactly what the compile cache already
    // invalidates), while steady-state body edits (which don't change the
    // fingerprint) keep skipping. Built once here from the effective spec
    // so the wipe in `ShadowWriter::new` can compare it. `None` (an
    // uncacheable pipeline — the bundler never produces one, but be
    // defensive) is treated as "config unknown" → always wipe.
    let config_fingerprint: Option<String> = effective_spec
        .build_pipeline()
        .ok()
        .and_then(|p| p.config_fingerprint());

    // The writer's dest-relative bookkeeping (visited / prune / skip caches)
    // is rooted at the WORK mirror, so every destination is workspace-root
    // -relative (`<workspace-rel>/pages/…` in a workspace, plain `pages/…`
    // without one). `ShadowWriter::new` also performs the session dirty /
    // copy-mode-flip wipe of the whole `work` tree, so the project mirror must
    // be (re-)created AFTER it.
    let writer = ShadowWriter::new(work.to_path_buf(), session, copy_mode, config_fingerprint)?;
    if workspace_rel.is_some() {
        fs::create_dir_all(shadow).with_context(|| {
            format!(
                "bundler: failed to create nested project mirror {}",
                shadow.display()
            )
        })?;
    }

    // Build the shared materialisation context from the fields of `input`
    // that are invariant across every materialise_shadow / materialise_collection
    // call in this bundle invocation.
    let worker_build_context = ModuleWorkerBuildContext::from_esbuild_loader_args(
        input.mode.is_prod(),
        &input.extra_loader_args,
        &input.define_vars,
        make_adapter(input.framework).jsx_import_source(),
    )
    .with_plugins(
        input.plugin_alias_entries.clone(),
        input.plugin_virtual_modules.clone(),
    )
    // This context describes the later browser-worker pass, not this SSR
    // bundle's own minify flag. Browser worker presets derive minify and
    // sourcemaps directly from mode in both islands/client emitters.
    .with_output_semantics(input.mode.is_prod(), !input.mode.is_prod());
    let mat_ctx = MaterialiseCtx {
        pipeline_spec: effective_spec,
        copy_mode,
        bundle_exclude: &bundle_exclude,
        project_root: &input.project_root,
        writer: &writer,
        glob_matched_files: RefCell::new(BTreeSet::new()),
        raw_import_edges: RefCell::new(BTreeSet::new()),
        raw_import_aliases: RawImportAliasContext::from_paths_and_project_base_url(
            &input.tsconfig_paths,
            &input.project_root,
        ),
        module_worker_dependencies: RefCell::new(BTreeSet::new()),
        worker_build_context,
        raw_preflight_complete: Cell::new(false),
        // #1151: the SHA-accurate collection-skip signal, parsed once per
        // bundle from the snapshot JSON the bundler already received.
        snapshot_specifiers: snapshot_specifier_set(input.content_snapshot_json.as_deref()),
    };
    let project_root = normalize_path_lexical(&input.project_root);
    let plugin_main_fields = effective_ssr_main_fields(&input);
    let mut exact_target_staging_files = BTreeSet::new();
    let mut exact_target_staging_dirs = BTreeSet::new();
    let mut exact_target_staging_alias_dirs = BTreeMap::new();

    // Concrete user mappings only need explicit staging when exclusions make
    // a live-real fallback unsafe. Plugin aliases are always staged because
    // they may point at hidden/unwalked files or package directories.
    for target in input
        .tsconfig_paths
        .values()
        .flatten()
        .filter(|target| !target.contains('*'))
    {
        let target_path = normalize_path_lexical(Path::new(target));
        if target_path.starts_with(&project_root) {
            plan_concrete_target_staging(
                target,
                &project_root,
                &bundle_exclude,
                &plugin_main_fields,
                !bundle_exclude.is_empty(),
                &mut exact_target_staging_files,
                &mut exact_target_staging_dirs,
            );
        }
    }
    for (_, target) in input
        .plugin_alias_entries
        .iter()
        // Exact user tsconfig mappings win over plugin registrations.
        .filter(|(specifier, _)| !input.tsconfig_paths.contains_key(specifier))
    {
        let target_path = normalize_path_lexical(Path::new(target));
        if target_path.starts_with(&project_root) {
            plan_concrete_target_staging(
                target,
                &project_root,
                &bundle_exclude,
                &plugin_main_fields,
                true,
                &mut exact_target_staging_files,
                &mut exact_target_staging_dirs,
            );
        }
    }
    let effective_virtual_context = mat_ctx
        .worker_build_context
        .clone()
        .without_user_claimed_virtual_modules(&input.tsconfig_paths);
    let mut plugin_preprocessing_files = BTreeSet::new();
    let mut root_entry_dependency_seed_files = BTreeSet::new();
    let mut root_entry_dependency_logical_importers = BTreeMap::new();
    if mat_ctx.worker_build_context.has_plugin_resolver_inputs() {
        let virtual_discovery = discover_registered_virtual_preprocessing_with_context(
            &input.project_root,
            &effective_virtual_context,
        )
        .context("bundler: validate registered virtual-module preprocessing syntax")?;
        plugin_preprocessing_files.extend(virtual_discovery.files);
        mat_ctx.raw_import_edges.borrow_mut().extend(
            virtual_discovery
                .raw_import_edges
                .into_iter()
                .map(|edge| RawImportEdge {
                    importer: edge.importer,
                    target: edge.target,
                }),
        );
    }

    // Discover preprocessing closure from every explicitly staged candidate,
    // including user exact tsconfig targets. This catches relative imports
    // escaping a hidden/package tree and prevents the isolated target from
    // losing required allowed dependencies. Root-level exact entry files also
    // seed the staged dependency view from their transitive first-party file
    // closure; the walk root is the entry file, never the project root as a
    // package. node_modules candidates keep the bounded containing-package copy
    // instead of parsing vendor trees here.
    for target in exact_target_staging_files.clone() {
        if !target.is_file()
            || bundle_exclude.is_excluded(&target, &project_root)
            || node_modules_package_root(&target, &project_root).is_some()
        {
            continue;
        }
        let discovery = match discover_module_preprocessing_with_context(
            &target,
            &input.project_root,
            &effective_virtual_context,
        ) {
            Ok(discovery) => discovery,
            Err(error) => {
                let message = format!("{error:#}");
                if message
                    .contains("zfb bundler: cannot safely skip unparseable module-worker source")
                {
                    return Err(error).with_context(|| {
                        format!(
                            "bundler: discover exact-target preprocessing graph from {}",
                            target.display()
                        )
                    });
                }
                if message.contains("zfb bundler: failed to parse ")
                    || message.contains(" is not valid UTF-8")
                {
                    // A syntactically invalid plausible alternative may be
                    // irrelevant in the actual CSS/JS/node_modules context.
                    // Keep it raw so esbuild reports it only when selected.
                    continue;
                }
                // Contract-specific preprocessing failures (unsupported
                // SharedWorker/query forms, unsafe graph escapes, etc.) remain
                // deterministic hard errors even for a registered alternate.
                return Err(error).with_context(|| {
                    format!(
                        "bundler: discover exact-target preprocessing graph from {}",
                        target.display()
                    )
                });
            }
        };
        let root_level_entry = root_level_staged_entry_file(&target, &project_root).is_some();
        let discovered_files = discovery.files;
        plugin_preprocessing_files.insert(target.clone());
        if root_level_entry {
            root_entry_dependency_seed_files.insert(target.clone());
            root_entry_dependency_seed_files.extend(
                discovered_files
                    .iter()
                    .filter(|path| {
                        path.starts_with(&project_root)
                            && path.is_file()
                            && !project_path_is_inside_node_modules(path, &project_root)
                            && !bundle_exclude.is_excluded(path, &project_root)
                    })
                    .cloned(),
            );
        }
        plugin_preprocessing_files.extend(discovered_files);
        mat_ctx
            .raw_import_edges
            .borrow_mut()
            .extend(
                discovery
                    .raw_import_edges
                    .into_iter()
                    .map(|edge| RawImportEdge {
                        importer: edge.importer,
                        target: edge.target,
                    }),
            );
    }

    // A workspace-root wildcard such as `@/* -> <workspace>/*` is too broad
    // to be a staging claim by itself. Resolve only specifiers that actually
    // occur in runtime position, then walk those concrete files' ordinary
    // first-party graphs. This keeps the workspace root from becoming a
    // wholesale mirror while still giving the existing materialisation pass
    // every reached file (including CSS and JSON leaves).
    let (runtime_alias_claims, runtime_alias_raw_import_edges, exact_gitignored_runtime_leaves) =
        collect_runtime_alias_claim_graph(
            &project_root,
            &first_party_root,
            &input.tsconfig_paths,
            &bundle_exclude,
            &effective_virtual_context,
        )?;
    plugin_preprocessing_files.extend(runtime_alias_claims);
    mat_ctx
        .raw_import_edges
        .borrow_mut()
        .extend(runtime_alias_raw_import_edges);

    // Issue #1692: the claim plan — computed ONCE here now that
    // `plugin_preprocessing_files` (claim sources a + c) is fully populated,
    // alongside the tsconfig/plugin alias targets (claim source b). It drives
    // the wholesale sibling mirror below and is the single source of truth the
    // Wave 2/3 CSS-discovery / audit sub-issues consume. Empty (and inert) for
    // a standalone project, so a non-workspace build stays byte-identical.
    let sibling_mirror_claim_files = plugin_preprocessing_files
        .difference(&exact_gitignored_runtime_leaves)
        .cloned()
        .collect();
    let sibling_mirror_plan = SiblingMirrorPlan::compute(
        &project_root,
        &first_party_root,
        &sibling_mirror_claim_files,
        &input.tsconfig_paths,
        &input.plugin_alias_entries,
    );

    // Issue #1724: make `mirror_sibling_root`'s own promise true — "reachable
    // source files that DO need `?raw`/glob/worker rewrites are overwritten
    // afterwards by the plugin-preprocessing pass". That pass materialises
    // `plugin_preprocessing_files`, which a claimed sibling's macro-bearing
    // source could never join: the exact-target loops above are gated to
    // `project_root`, and `collect_runtime_alias_claim_graph` fail-closes on
    // workspace membership (a bare `lib/shared` region is claimed by the
    // mirror plan but is not a workspace package). So the mirror's RAW byte
    // copy was the only pass those files ever saw and their
    // `import.meta.glob` / module-worker `new URL(...)` macros reached
    // esbuild literally — a GREEN build with unexpanded macro text in the
    // bundle. Enrolling every mirrored SOURCE file here routes it through the
    // same `materialise_source_file` every project file gets, which
    // overwrites the raw copy in place. This is the same conclusion the #1695
    // glob fixed-point queue already reached for glob matches inside a
    // claimed region (see `stage_glob_matched_files_to_fixed_point`'s doc
    // comment): a wholesale mirror is a byte copy, so macro expansion has to
    // come from somewhere else. Non-source leaves (CSS/JSON/text) carry no
    // macros and keep the pure mirror copy.
    //
    // A mirror root is claimed WHOLESALE, so this set includes files no
    // import edge ever reaches — an unused fixture carrying an unsupported
    // `SharedWorker` form or an unparseable snippet that esbuild would never
    // load. Those must not become build failures they were not before, so
    // the enrolments recorded here are marked NON-FATAL: if
    // `materialise_source_file` rejects one, the pass below keeps the
    // mirror's raw copy (already on disk and already recorded visited) and
    // moves on, which is byte-for-byte the pre-#1724 outcome for that file.
    // Every other membership in `plugin_preprocessing_files` — a discovered
    // graph file, an exact alias target, a virtual-module import — stays a
    // hard error, because those ARE reached and a silent non-expansion there
    // is the very defect this issue fixes.
    let mut mirror_derived_preprocessing_files = BTreeSet::new();
    for mirror_root in sibling_mirror_plan.mirror_roots() {
        for src in enumerate_mirror_root_files(mirror_root) {
            // The mirror pass's own guards, so this set can never be wider
            // than what was actually mirrored.
            if !raw_source_extension(&src)
                || path_is_inside_node_modules(&src)
                || !src.starts_with(&first_party_root)
                || src.starts_with(&project_root)
                || bundle_exclude.is_excluded(&src, &project_root)
            {
                continue;
            }
            if plugin_preprocessing_files.insert(src.clone()) {
                mirror_derived_preprocessing_files.insert(src);
            }
        }
    }

    // Seed the staged-dependency closure from the materialized project module
    // graph and the framework packages the generated entry/hydration shim
    // import (issue #1645). Source discovery also runs with an empty exclude so
    // package-name workspace siblings can be detected and copied before the
    // stage-escape audit. Ordinary dependencies remain live-link-resolved in
    // that mode; the root-seed loop below only admits workspace sources.
    let mut synthetic_entry_import_specifiers = BTreeSet::new();
    {
        let mut source_graph_roots = vec![
            pages_dir.clone(),
            components_dir.clone(),
            layouts_dir.clone(),
        ];
        // A package-route overlay is physically outside the project but is
        // copied to `<shadow>/pages`. Preserve that logical importer location
        // so a synthesized relative `../node_modules/<package>/...` import can
        // seed the package at the same staged location esbuild will resolve.
        let mut logical_source_roots = vec![(pages_dir.clone(), project_root.join("pages"))];
        if input.content_collections.is_empty() {
            source_graph_roots.push(content_dir.clone());
        } else {
            for collection in &input.content_collections {
                source_graph_roots.push(resolver.resolve(&collection.root));
            }
        }
        if let Some(injected) = input.injected_pages_root.as_ref() {
            let injected = resolver.resolve(injected);
            source_graph_roots.push(injected.clone());
            logical_source_roots.push((injected, project_root.join("pages")));
        }
        source_graph_roots.extend(enumerate_extra_top_level_dirs(
            &input.project_root,
            KNOWN_SOURCE_DIRS,
        ));
        // The dot-path staging allowlist (issue #1840) is staged like an extra
        // source dir, so its files must seed the module graph too —
        // `enumerate_extra_top_level_dirs` above can never see them (hidden +
        // conventionally gitignored), and an import FROM `.zudo-doc/routes-src`
        // would otherwise be missing from the staged-dependency closure.
        source_graph_roots.extend(
            enumerate_first_party_staging_dirs(&input.project_root)
                .into_iter()
                .map(|(src, _)| src),
        );
        collect_project_source_module_graph_seed_files(
            &source_graph_roots,
            &project_root,
            &bundle_exclude,
            &logical_source_roots,
            &mut root_entry_dependency_seed_files,
            &mut root_entry_dependency_logical_importers,
        );

        // The server runtime subpath (`createPageRouter`), the framework
        // render-to-string module, and the JSX runtime source (which also
        // covers the hydration shim's bare framework import) appear in NO
        // project source file, so the file-driven seed above can never discover
        // them.
        if !bundle_exclude.is_empty() {
            synthetic_entry_import_specifiers.insert(ZFB_RUNTIME_SERVER_SPECIFIER.to_string());
            synthetic_entry_import_specifiers.insert(adapter.render_to_string_module().to_string());
            synthetic_entry_import_specifiers.insert(adapter.jsx_import_source().to_string());
        }
    }

    // Specifiers the alias system resolves (tsconfig `paths`, plugin aliases,
    // plugin virtual modules). The source-graph seed must not stage a same-named
    // installed package for these — see `specifier_is_alias_claimed` (#1557/#1645).
    let alias_claimed_specifier_keys: BTreeSet<String> = input
        .tsconfig_paths
        .keys()
        .cloned()
        .chain(
            input
                .plugin_alias_entries
                .iter()
                .map(|(specifier, _)| specifier.clone()),
        )
        .chain(
            input
                .plugin_virtual_modules
                .iter()
                .map(|(specifier, _)| specifier.clone()),
        )
        .collect();

    extend_node_modules_dependency_staging(
        &project_root,
        input.node_modules_dir.as_deref(),
        &bundle_exclude,
        !esbuild_will_preserve_symlinks(&input),
        &root_entry_dependency_seed_files,
        &root_entry_dependency_logical_importers,
        &synthetic_entry_import_specifiers,
        &alias_claimed_specifier_keys,
        &input.external,
        &mut exact_target_staging_dirs,
        &mut exact_target_staging_alias_dirs,
    );

    // WHERE staged `node_modules` targets land depends on `bundle.exclude`:
    //
    // - Empty exclude, ordinary deps only → the live `<shadow>/node_modules`
    //   symlink exists (2b), so explicitly staged node_modules targets stay in a
    //   separate `zfb-exact-node-modules-*` tempdir.
    // - Any workspace-package source → use REAL copies at the natural shadow
    //   node_modules position even with an empty exclude. Otherwise a bare
    //   package import cannot see the isolated tempdir, climbs to the WORK
    //   mirror's live workspace link, and correctly trips the stage-escape
    //   audit. The staged dependency closure supplies the ordinary packages
    //   those workspace packages need, so no live shadow link is necessary.
    //
    // - Exclusions active → the live symlink is NOT created (2b), so there is
    //   nothing to climb to. Staged deps are materialised as REAL copies at
    //   their NATURAL shadow position (`<shadow>/node_modules/<dep>`,
    //   `<shadow>/<pkg>/node_modules/<dep>`), where esbuild's ordinary walk finds
    //   the allowed deps — top-level AND nested (a first-party importer's
    //   vendored / non-hoisted dep). This is expressed by passing `shadow` itself
    //   as the isolation ROOT (so `shadow_path_for_project_path` maps
    //   node_modules paths to their in-place shadow location) with NO separate
    //   isolation writer, so the main session-aware `ShadowWriter` materialises
    //   them (and the prune pass keeps them correct across session ticks).
    let workspace_package_staging_active = exact_target_staging_alias_dirs
        .values()
        .chain(
            exact_target_staging_dirs
                .iter()
                .filter(|path| project_path_is_inside_node_modules(path, &project_root)),
        )
        .any(|source_root| {
            source_root.canonicalize().is_ok_and(|canonical| {
                canonical_workspace_package_logical_path(&canonical, &project_root).is_some()
            })
        });
    let needs_tempdir_isolation = bundle_exclude.is_empty()
        && !workspace_package_staging_active
        && exact_target_staging_files
            .iter()
            .chain(exact_target_staging_dirs.iter())
            .chain(exact_target_staging_alias_dirs.keys())
            .any(|path| project_path_is_inside_node_modules(path, &project_root));
    let node_modules_isolation = needs_tempdir_isolation
        .then(|| {
            let parent = shadow_parent_dir(&input.project_root)?;
            tempfile::Builder::new()
                .prefix("zfb-exact-node-modules-")
                .tempdir_in(parent)
                .context("bundler: allocate exact node_modules isolation root")
        })
        .transpose()?;
    let tempdir_isolation_root = node_modules_isolation
        .as_ref()
        .map(|isolation| isolation.path());
    let node_modules_isolation_root: Option<&Path> =
        if bundle_exclude.is_empty() && !workspace_package_staging_active {
            tempdir_isolation_root
        } else {
            Some(shadow)
        };
    if let Some(root) = node_modules_isolation_root {
        ensure_shadow_path_outside_project(
            &input.project_root,
            root,
            "exact-node-modules isolation root",
        )?;
    }
    let node_modules_isolation_writer = tempdir_isolation_root
        .map(|root| ShadowWriter::new(root.to_path_buf(), None, true, None))
        .transpose()?;

    if let Some(src) = input.mdx_components_file.as_deref() {
        if !bundle_exclude.is_excluded(src, &input.project_root) {
            preflight_raw_file(src, src, &mat_ctx);
        }
    }

    let shadow_pages = shadow.join("pages");
    let shadow_content = shadow.join("content");
    let shadow_components = shadow.join("components");
    let shadow_layouts = shadow.join("layouts");
    let extra_source_dirs = enumerate_extra_top_level_dirs(&input.project_root, KNOWN_SOURCE_DIRS);
    let first_party_staging_dirs = enumerate_first_party_staging_dirs(&input.project_root);

    // Preflight every source root before materialising any of them. An
    // importer under a later root may target a JS-looking file under an
    // earlier root, so per-root discovery would make terminal behavior depend
    // on walk order.
    preflight_raw_tree(&pages_dir, &shadow_pages, &mat_ctx)?;
    if let Some(injected_root) = input.injected_pages_root.as_ref() {
        let injected_root = resolver.resolve(injected_root);
        preflight_raw_tree(&injected_root, &shadow_pages, &mat_ctx)?;
    }
    if input.content_collections.is_empty() {
        preflight_raw_tree(&content_dir, &shadow_content, &mat_ctx)?;
    } else {
        for collection in &input.content_collections {
            let root = resolver.resolve(&collection.root);
            preflight_raw_tree(&root, &shadow_content.join(&collection.name), &mat_ctx)?;
        }
    }
    preflight_raw_tree(&components_dir, &shadow_components, &mat_ctx)?;
    preflight_raw_tree(&layouts_dir, &shadow_layouts, &mat_ctx)?;
    for src_dir in &extra_source_dirs {
        preflight_raw_tree(
            src_dir,
            &shadow.join(src_dir.file_name().unwrap_or_default()),
            &mat_ctx,
        )?;
    }
    // Dot-path staging allowlist (issue #1840) — preflight the allowlisted
    // trees like any other source root so a `?raw` edge from/into one is
    // established before anything is materialised.
    for (src_dir, rel) in &first_party_staging_dirs {
        preflight_raw_tree(src_dir, &shadow.join(rel), &mat_ctx)?;
    }
    mat_ctx.raw_preflight_complete.set(true);

    let mut routes: Vec<RouteEntry> = Vec::new();
    {
        let mut broken = Vec::new();
        let mut md_diags = Vec::new();
        let mut cfl = Vec::new();
        let mut fh = Vec::new();
        materialise_shadow(
            &pages_dir,
            &shadow_pages,
            &mut routes,
            &mat_ctx,
            &mut broken,
            &mut md_diags,
            &mut cfl,
            &mut fh,
        )
        .with_context(|| {
            format!(
                "bundler: failed materialising pages from {}",
                pages_dir.display()
            )
        })?;
        all_broken_links.extend(broken);
        all_markdown_diagnostics.extend(md_diags);
        all_cross_file_links.extend(cfl);
        all_file_headings.extend(fh);
    }

    // S2 (#1230) — ADDITIVE injected-route root for `zfb dev` (B1 multi-root).
    // When `injected_pages_root` is set, walk it into the SAME shadow `pages/`
    // tree as the primary `pages_dir` above, appending the synthesized injected
    // modules to `routes`. Conventional dev bundles contain user pages plus
    // injected entrypoints (and resolve their `virtual:` imports) without
    // copying the user's `pages/` (the command layer staged ONLY the injected
    // modules there). #1518 zero-pages dev uses a private empty primary root,
    // so this remains the additive injected-only walk. The staging root holds
    // no user pages, so there is no file collision with the main walk. `None`
    // (every `zfb build`, and `zfb dev` with no injected routes) skips this
    // entirely — byte-identical to a bundle that never knew the field.
    if let Some(injected_root) = input.injected_pages_root.as_ref() {
        let injected_root = resolver.resolve(injected_root);
        // A missing/empty staging root is a no-op (the same defensive bias as
        // `materialise_shadow`'s own missing-src early return).
        if injected_root.is_dir() {
            let mut broken = Vec::new();
            let mut md_diags = Vec::new();
            let mut cfl = Vec::new();
            let mut fh = Vec::new();
            materialise_shadow(
                &injected_root,
                &shadow_pages,
                &mut routes,
                &mat_ctx,
                &mut broken,
                &mut md_diags,
                &mut cfl,
                &mut fh,
            )
            .with_context(|| {
                format!(
                    "bundler: failed materialising injected routes from {}",
                    injected_root.display()
                )
            })?;
            all_broken_links.extend(broken);
            all_markdown_diagnostics.extend(md_diags);
            all_cross_file_links.extend(cfl);
            all_file_headings.extend(fh);
        }
    }

    // Per-collection content materialisation (#506).
    //
    // When `content_collections` is non-empty (every production build
    // whose `zfb.config.ts` declares collections), walk each
    // collection's source root individually into
    // `shadow/content/<name>/<rel_path>` and remember the
    // `(specifier, shadow_rel_path)` pair for each `.mdx` entry.
    // The pairs feed the bridge installer emitted in `entry.mjs` so
    // `globalThis.__zfb.content.get(specifier)` resolves to the
    // compiled MDX module at runtime. When the field is empty, fall
    // back to the legacy single-`content_dir` walk (no bridge entries)
    // so existing fixtures and tests keep compiling unchanged.
    let mut content_imports: Vec<ContentImport> = Vec::new();
    if !input.content_collections.is_empty() {
        for col in &input.content_collections {
            let col_root = resolver.resolve(&col.root);
            let dest = shadow_content.join(&col.name);
            let mut broken = Vec::new();
            let mut md_diags = Vec::new();
            let mut cfl = Vec::new();
            let mut fh = Vec::new();
            materialise_collection(
                &col_root,
                &dest,
                &col.name,
                &mut content_imports,
                &mat_ctx,
                col.include.as_deref(),
                col.exclude.as_deref(),
                col.id_strip_suffix.as_deref(),
                &mut broken,
                &mut md_diags,
                &mut cfl,
                &mut fh,
            )
            .with_context(|| {
                format!(
                    "bundler: failed materialising collection `{}` from {}",
                    col.name,
                    col_root.display()
                )
            })?;
            all_broken_links.extend(broken);
            all_markdown_diagnostics.extend(md_diags);
            all_cross_file_links.extend(cfl);
            all_file_headings.extend(fh);
        }
        // Deterministic ordering — keys are `(collection, rel_path)`
        // so the emitted import indices match the underlying file
        // tree on every build, regardless of WalkDir's per-OS order.
        content_imports.sort_by(|a, b| a.shadow_rel_path.cmp(&b.shadow_rel_path));
    } else {
        let mut broken = Vec::new();
        let mut md_diags = Vec::new();
        let mut cfl = Vec::new();
        let mut fh = Vec::new();
        materialise_shadow(
            &content_dir,
            &shadow_content,
            &mut Vec::new(),
            &mat_ctx,
            &mut broken,
            &mut md_diags,
            &mut cfl,
            &mut fh,
        )
        .with_context(|| {
            format!(
                "bundler: failed materialising content from {}",
                content_dir.display()
            )
        })?;
        all_broken_links.extend(broken);
        all_markdown_diagnostics.extend(md_diags);
        all_cross_file_links.extend(cfl);
        all_file_headings.extend(fh);
    }

    {
        let mut broken = Vec::new();
        let mut md_diags = Vec::new();
        let mut cfl = Vec::new();
        let mut fh = Vec::new();
        materialise_shadow(
            &components_dir,
            &shadow_components,
            &mut Vec::new(),
            &mat_ctx,
            &mut broken,
            &mut md_diags,
            &mut cfl,
            &mut fh,
        )
        .with_context(|| {
            format!(
                "bundler: failed materialising components from {}",
                components_dir.display()
            )
        })?;
        all_broken_links.extend(broken);
        all_markdown_diagnostics.extend(md_diags);
        all_cross_file_links.extend(cfl);
        all_file_headings.extend(fh);
    }
    {
        let mut broken = Vec::new();
        let mut md_diags = Vec::new();
        let mut cfl = Vec::new();
        let mut fh = Vec::new();
        materialise_shadow(
            &layouts_dir,
            &shadow_layouts,
            &mut Vec::new(),
            &mat_ctx,
            &mut broken,
            &mut md_diags,
            &mut cfl,
            &mut fh,
        )
        .with_context(|| {
            format!(
                "bundler: failed materialising layouts from {}",
                layouts_dir.display()
            )
        })?;
        all_broken_links.extend(broken);
        all_markdown_diagnostics.extend(md_diags);
        all_cross_file_links.extend(cfl);
        all_file_headings.extend(fh);
    }

    // 2a-extra. Materialise any *other* directories at the project root
    // that the bundler does not own (e.g. `styles/`, `lib/`, `utils/`).
    // Layout and component files often import relative paths like
    // `"../styles/global.css"` — those paths need to resolve to something
    // in the shadow tree even though the bundler treats CSS as empty
    // (`--loader:.css=empty`). esbuild's resolution step runs before the
    // loader, so a missing file causes a hard error regardless of the loader.
    //
    // Directories already materialised above (pages, content, components,
    // layouts) and infrastructure directories (node_modules, dist, .git,
    // hidden dirs, zfb output dirs) are skipped. Additionally, directories
    // excluded by the consumer's .gitignore (or global git ignore) are
    // skipped — see `enumerate_extra_top_level_dirs` for the full rules.
    //
    // Behavior note: consumers using negated patterns like `!worktrees/keep/`
    // to opt a sub-path back in will find the negation silently ignored at
    // this pass — `max_depth=1` means we operate whole-dir-or-nothing.
    {
        for src_dir in &extra_source_dirs {
            let name = src_dir.file_name().unwrap_or_default().to_os_string();
            let dst_dir = shadow.join(&name);
            let mut broken = Vec::new();
            let mut md_diags = Vec::new();
            let mut cfl = Vec::new();
            let mut fh = Vec::new();
            materialise_shadow(
                src_dir,
                &dst_dir,
                &mut Vec::new(),
                &mat_ctx,
                &mut broken,
                &mut md_diags,
                &mut cfl,
                &mut fh,
            )
            .with_context(|| {
                format!(
                    "bundler: failed materialising extra dir {} into shadow",
                    src_dir.display()
                )
            })?;
            all_broken_links.extend(broken);
            all_markdown_diagnostics.extend(md_diags);
            all_cross_file_links.extend(cfl);
            all_file_headings.extend(fh);
        }
    }

    // 2a-dot. First-party dot-path staging allowlist (issue #1840). Stage the
    // narrow zudo-doc 4.x compatibility surface — `.zudo-doc/routes-src/**`
    // (generated first-party route source) and the top-level `.zfb/*.json`
    // meta files — that every generic walker above deliberately skips (hidden
    // dirs pruned; `.gitignore` honored). Materialised through the SAME
    // `mat_ctx`/`ShadowWriter` machinery as the extra dirs so the session
    // visited/prune bookkeeping applies: deleting `.zudo-doc/` live prunes the
    // staged copy on the next session call. Additive only — the shared
    // `is_pruned_infra_dir` semantics are untouched (each allowlisted dir is
    // its own walk ROOT, which the depth-0 guard never prunes).
    {
        for (src_dir, rel) in &first_party_staging_dirs {
            let dst_dir = shadow.join(rel);
            let mut broken = Vec::new();
            let mut md_diags = Vec::new();
            let mut cfl = Vec::new();
            let mut fh = Vec::new();
            materialise_shadow(
                src_dir,
                &dst_dir,
                &mut Vec::new(),
                &mat_ctx,
                &mut broken,
                &mut md_diags,
                &mut cfl,
                &mut fh,
            )
            .with_context(|| {
                format!(
                    "bundler: failed materialising first-party staging dir {} into shadow",
                    src_dir.display()
                )
            })?;
            all_broken_links.extend(broken);
            all_markdown_diagnostics.extend(md_diags);
            all_cross_file_links.extend(cfl);
            all_file_headings.extend(fh);
        }
        for (src, rel) in enumerate_first_party_staging_json_files(&input.project_root) {
            if bundle_exclude.is_excluded(&src, &input.project_root) {
                continue;
            }
            let dest = shadow.join(&rel);
            if let Some(parent) = dest.parent() {
                writer.ensure_dir(parent).with_context(|| {
                    format!(
                        "bundler: create first-party staging meta dir {}",
                        parent.display()
                    )
                })?;
            }
            writer.copy_if_changed(&src, &dest).with_context(|| {
                format!(
                    "bundler: stage first-party meta file {} -> {}",
                    src.display(),
                    dest.display()
                )
            })?;
        }
    }

    // 2a-sibling. Wholesale sibling mirror (issue #1692). For each claimed
    // mirror root, RAW-copy its whole tree (gitignore + skip-list honored,
    // root-level files included) into its workspace-relative slot in the WORK
    // mirror, so esbuild — the sole resolver — can resolve anything inside a
    // sibling region (glob targets, index files, `package.json` main/imports,
    // wildcard-alias targets) that the Rust preprocessing discovery could not
    // predict. Reachable source files that need `?raw`/glob/worker rewrites are
    // overwritten by the plugin-preprocessing pass further below (which
    // materialises the discovered graph AFTER this raw copy). Gated on
    // `workspace_rel.is_some()` (⇔ the plan being non-empty requires a widened
    // first-party root), so a non-workspace build never enters here and stays
    // byte-identical.
    if workspace_rel.is_some() && !sibling_mirror_plan.is_empty() {
        for mirror_root in sibling_mirror_plan.mirror_roots() {
            mirror_sibling_root(
                mirror_root,
                &project_root,
                &first_party_root,
                shadow,
                work,
                node_modules_isolation_root,
                &writer,
                &bundle_exclude,
            )
            .with_context(|| {
                format!(
                    "bundler: wholesale-mirror sibling region {}",
                    mirror_root.display()
                )
            })?;
        }
    }

    // 2a-ws. Workspace-hoisted `node_modules` at the WORK root (issue #1668
    //     part 1; contract inverted by #1693). In a pnpm workspace the deps
    //     are hoisted to the workspace root, so the work mirror gets its own
    //     `<work>/node_modules` symlink — the SECOND install root above the
    //     project's own nested install (linked at `<shadow>/node_modules` by
    //     2b below). esbuild walks up from a project-mirror file to
    //     `<shadow>/node_modules` first, then to `<work>/node_modules`,
    //     preserving nearest-package precedence — including for a
    //     wholesale-mirrored sibling staged directly under
    //     `<work>/<workspace_rel>/...` (#1692), which has no ancestor
    //     `node_modules` to resolve a bare import against unless this link
    //     exists.
    //
    //     UNLIKE the 2b link, this one is now created REGARDLESS of
    //     `bundle.exclude` — empty or active. Removing it under exclusions
    //     (the pre-#1693 behavior) broke every sibling bare import, not just
    //     the excluded ones, once #1692 started staging sibling sources
    //     outside `<shadow>`. "Exclusion = absence from a staged shadow tree"
    //     stays enforceable anyway: the fail-closed metafile audit
    //     (`audit_metafile_exclusions_at_path`, run after esbuild below)
    //     checks every input esbuild actually resolved — canonicalised —
    //     against the TIER-2 workspace-relative `BundleExcludeMatcher`
    //     pattern set (issue #1676), which is armed for exactly this case: a
    //     resolved path under `first_party_root` but outside `project_root`.
    //     So a resurrection climbing this link is not silently missed — it
    //     hard-fails the build as an offending metafile input, the same
    //     fail-closed guarantee the old "never create the link" approach
    //     gave, without breaking legitimate sibling deps.
    //
    //     Guard (b) (issue #1706, the stage-escape audit run right after the
    //     exclusion audit below) is the second, exclusion-independent lock on
    //     this same link: `bundle.exclude` resurrection is only ONE way to
    //     abuse it. A bare first-party PACKAGE-NAME import (as opposed to the
    //     alias/relative reach #1691 stages) climbs this link straight to a
    //     LIVE sibling regardless of whether anything is excluded, so the
    //     stage-escape audit fires for EVERY real pass when `workspace_rel`
    //     is set — not just under active exclusions.
    //
    //     Without a workspace `workspace_rel` is `None`, so this is skipped
    //     entirely and the layout stays byte-identical. Created directly (not
    //     through the ShadowWriter), like the 2b link, so it never enters the
    //     prune bookkeeping.
    if workspace_rel.is_some() {
        let workspace_node_modules = first_party_root.join("node_modules");
        if workspace_node_modules.is_dir() {
            let work_nm = work.join("node_modules");
            #[cfg(unix)]
            {
                let already_correct = fs::read_link(&work_nm)
                    .map(|t| t == workspace_node_modules)
                    .unwrap_or(false);
                if !already_correct {
                    let _ = fs::remove_file(&work_nm);
                    let _ = fs::remove_dir_all(&work_nm);
                    std::os::unix::fs::symlink(&workspace_node_modules, &work_nm).with_context(
                        || {
                            format!(
                                "bundler: failed to symlink workspace node_modules {} → {}",
                                workspace_node_modules.display(),
                                work_nm.display()
                            )
                        },
                    )?;
                }
            }
            #[cfg(not(unix))]
            {
                fs::create_dir_all(&work_nm).with_context(|| {
                    format!(
                        "bundler: failed to create workspace node_modules dir {}",
                        work_nm.display()
                    )
                })?;
            }
        }
    }

    // 2b. `<shadow>/node_modules` resolution root.
    //     When `BundlerInput::node_modules_dir` is set, esbuild needs a
    //     `<shadow>/node_modules` to walk into instead of an empty tempdir
    //     ancestry. WHAT lives there depends on `bundle.exclude`:
    //
    //     - Empty exclude with no staged workspace package → symlink
    //       `<shadow>/node_modules → <live node_modules>` (historical behavior).
    //     - Exclusions active OR a staged workspace package → the live symlink is a fallback that could
    //       resurrect an excluded dependency (esbuild climbing to the real tree),
    //       so it is NEVER created. Non-excluded dependencies are supplied as REAL
    //       staged copies materialised into the shadow at their logical paths
    //       (`extend_node_modules_dependency_staging` + the exact-target staging
    //       loops below, which write into `<shadow>` when no isolation root is
    //       allocated). esbuild's ordinary node_modules walk then finds the
    //       allowed deps and misses the excluded ones (acceptance #5 — handled
    //       via the staged view, not plain link removal).
    //
    //     "Don't create the live link" is not enough for a persistent
    //     `ShadowSession` (#993) that previously ran with an empty exclude: the
    //     link bypasses the ShadowWriter prune bookkeeping, so a stale live link
    //     must be actively REMOVED on the empty→non-empty transition (else a
    //     `<shadow>/node_modules` symlink to the live tree would survive, and the
    //     staging loops' `ensure_dir` would follow it out of the shadow). When the
    //     session later returns to an empty exclude the branch below re-creates it.
    if let Some(ref nm_dir) = input.node_modules_dir {
        let shadow_nm = shadow.join("node_modules");
        if bundle_exclude.is_empty() && !workspace_package_staging_active {
            #[cfg(unix)]
            {
                // Session mode (#993) reuses the persistent shadow across
                // calls, so the link usually already exists — re-creating it
                // unconditionally would fail with AlreadyExists. Recreate only
                // when missing or pointing elsewhere. (Sessionless path: the
                // tempdir is fresh, read_link fails, behaviour is identical
                // to the previous unconditional symlink.) The link bypasses
                // the ShadowWriter bookkeeping entirely — the prune pass can
                // never see it, and guards against it anyway.
                let already_correct = fs::read_link(&shadow_nm)
                    .map(|t| &t == nm_dir)
                    .unwrap_or(false);
                if !already_correct {
                    // Clear whatever occupies the slot: a stale symlink, OR a
                    // REAL staged `node_modules` dir left by a previous
                    // exclusions tick of this persistent session (which stages
                    // deps in place under `<shadow>/node_modules`). remove_file
                    // clears a symlink; remove_dir_all clears a real dir; the
                    // other no-ops.
                    let _ = fs::remove_file(&shadow_nm);
                    let _ = fs::remove_dir_all(&shadow_nm);
                    std::os::unix::fs::symlink(nm_dir, &shadow_nm).with_context(|| {
                        format!(
                            "bundler: failed to symlink node_modules {} → {}",
                            nm_dir.display(),
                            shadow_nm.display()
                        )
                    })?;
                }
            }
            #[cfg(not(unix))]
            {
                // On Windows, attempt a directory junction.
                fs::create_dir_all(&shadow_nm).with_context(|| {
                    format!("bundler: failed to create node_modules dir in shadow tree")
                })?;
            }
        } else {
            // Staged-only dependency view: remove any stale live symlink from a previous
            // empty-exclude tick of a persistent session. `remove_file` deletes
            // the symlink itself (not its target); a real staged `node_modules`
            // dir written by the staging loops below is left untouched because it
            // is a directory, not a symlink (so this no-ops it). Absence + real
            // staged copies is the steady state.
            #[cfg(unix)]
            {
                if fs::symlink_metadata(&shadow_nm)
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false)
                {
                    let _ = fs::remove_file(&shadow_nm);
                }
            }
            #[cfg(not(unix))]
            {
                if fs::symlink_metadata(&shadow_nm)
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false)
                {
                    // A Windows symlink may be a file-symlink or a
                    // directory-symlink/junction. Remove the LINK itself without
                    // recursing into its target: `remove_file` clears a
                    // file-symlink, `remove_dir` clears a dir-symlink/junction
                    // (NOT `remove_dir_all`, which could follow the link and
                    // delete the target tree's contents). The other no-ops.
                    let _ = fs::remove_file(&shadow_nm);
                    let _ = fs::remove_dir(&shadow_nm);
                }
            }
        }
    }

    // 2c-anchor. Post-compile cross-file anchor check (#980).
    //
    // After ALL materialise walks ran, assemble a build-scoped heading map
    // from the per-file heading records collected above, then verify every
    // cross-file fragment-link candidate against it.  Target files that
    // produced no `FileHeadings` entry (out-of-build targets — excluded files,
    // non-walked dirs) are SKIPPED: same existence-only degrade as before,
    // narrowed to genuinely-out-of-build targets.
    //
    // Synthesised `BrokenLink` findings are pushed into `all_markdown_diagnostics`
    // so the existing severity-routing gate (2c below) handles them: the
    // `CrossFileLinkCandidate::severity` field already encodes the
    // `failOnBroken` → `Error` / else `Warning` decision the recording plugin
    // made at compile time, so we re-use it directly.
    //
    // Key-normalization contract: both heading-map keys and candidate target
    // keys go through `zfb_types::normalize_path_lexical` (imported above) —
    // the shared helper applied at record time.  Using the same helper here is
    // the grep-verifiable guarantee the two lookup sides agree on path spelling
    // (zfb#980 acceptance criterion).
    {
        // Build a HashMap<normalized_path, HashSet<id>> from the collected
        // per-file heading records.  A file present in the map but with an
        // empty heading set is meaningful: it compiled and has no
        // anchor-addressable headings.  A file absent from the map was never
        // compiled by the bundler (out-of-build) → skip, not fail.
        let heading_map: HashMap<PathBuf, HashSet<String>> = {
            let mut map: HashMap<PathBuf, HashSet<String>> =
                HashMap::with_capacity(all_file_headings.len());
            for fh in &all_file_headings {
                // source_path is already normalised at record time (#977);
                // apply again (idempotent) so any future refactor of the
                // recording path cannot silently break the lookup.
                let key = normalize_path_lexical(&fh.source_path);
                let ids: HashSet<String> = fh.headings.iter().map(|h| h.id.clone()).collect();
                // If two walks produced an entry for the same file (e.g. via
                // the cache-hit replay path), merge the id sets rather than
                // overwriting — both are ground-truth from the same source.
                map.entry(key).or_default().extend(ids);
            }
            map
        };
        for candidate in &all_cross_file_links {
            // target_path is already normalised at record time (#977); apply
            // again for the same idempotence reason as the heading-map keys.
            let target_key = normalize_path_lexical(&candidate.target_path);
            let Some(ids) = heading_map.get(&target_key) else {
                // Target file was never compiled by the bundler → out-of-build
                // target → skip (existence-only degrade, zfb#980 contract).
                continue;
            };
            if !ids.contains(&candidate.fragment) {
                // Target compiled but the fragment is absent → broken anchor.
                all_markdown_diagnostics.push(MarkdownDiagnostic::BrokenLink {
                    severity: candidate.severity,
                    url: candidate.raw_href.clone(),
                    location: Some(SourceLocation::from_path(candidate.source_path.clone())),
                });
            }
        }
    }

    // 2c. Handle markdown diagnostics (transclude errors, imageDimensions
    // warnings, linkValidation findings) collected across all materialise calls.
    //
    // All walks ran to completion first so the full set is reported in one
    // pass — same contract as the broken-links gate below.
    //
    // Ordering note: this block intentionally runs BEFORE the broken-links
    // gate (2d) so that markdown-diagnostic errors (e.g. transclude failures)
    // are always surfaced even when `onBrokenLinks: error` would bail first.
    // Both gates collect after all walks, so neither skips any findings.
    //
    // Severity routing:
    //   Info / Warning  → stderr warn lines with path(:line) prefix; build succeeds.
    //   Error           → bail! listing all findings; build fails.
    //
    // Intentional divergence from #948's original wording:
    //   #948 said "warnings for imageDimensions/transclude", but transclude
    //   failures are Error severity by plugin design (the include node is
    //   dropped when the file cannot be read — the output is broken), so
    //   they FAIL the build.  imageDimensions failures are Warning severity
    //   (the <img> is left unchanged) and only warn.  linkValidation severity
    //   is governed by the `failOnBroken` config flag (Warning by default,
    //   Error when set).  This divergence is intentional and permanent; do
    //   not "fix" it to match the original wording (zfb#953).
    //
    // Fatal findings from this gate and the broken-links gate (2d) are
    // accumulated here and bailed on ONCE after both gates have reported,
    // so a build with both failure classes surfaces the full set in one
    // pass instead of revealing the second class only after the first is
    // fixed.
    let mut fatal_findings: Vec<String> = Vec::new();
    if !all_markdown_diagnostics.is_empty() {
        // Format a location prefix: "path" or "path:line" when available.
        let fmt_location = |d: &MarkdownDiagnostic| -> String {
            let loc = match d {
                MarkdownDiagnostic::BrokenLink { location, .. } => location.as_ref(),
                MarkdownDiagnostic::Generic { location, .. } => location.as_ref(),
            };
            match loc {
                Some(l) => {
                    let path_str = l
                        .path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default();
                    match l.line {
                        Some(line) => format!("{path_str}:{line}"),
                        None => path_str,
                    }
                }
                None => String::new(),
            }
        };
        let fmt_message = |d: &MarkdownDiagnostic| -> String {
            match d {
                MarkdownDiagnostic::BrokenLink { url, .. } => {
                    format!("broken link: {url}")
                }
                MarkdownDiagnostic::Generic { message, .. } => message.clone(),
            }
        };

        let errors: Vec<&MarkdownDiagnostic> = all_markdown_diagnostics
            .iter()
            .filter(|d| d.severity() == DiagnosticSeverity::Error)
            .collect();

        // Emit Info / Warning diagnostics regardless of whether there are
        // errors — the user should see all findings before the build aborts.
        for d in &all_markdown_diagnostics {
            if d.severity() < DiagnosticSeverity::Error {
                let loc = fmt_location(d);
                let msg = fmt_message(d);
                if loc.is_empty() {
                    eprintln!("zfb warn: {msg}");
                } else {
                    eprintln!("zfb warn: {loc}: {msg}");
                }
            }
        }

        if !errors.is_empty() {
            let mut msg = format!(
                "bundler: {} markdown diagnostic error(s) found:\n",
                errors.len()
            );
            for d in &errors {
                let loc = fmt_location(d);
                let text = fmt_message(d);
                if loc.is_empty() {
                    msg.push_str(&format!("  error: {text}\n"));
                } else {
                    msg.push_str(&format!("  {loc}: {text}\n"));
                }
            }
            fatal_findings.push(msg.trim_end().to_string());
        }
    }

    // 2d. Handle broken links collected across all materialise calls.
    //
    // All calls ran to completion first so the full set of broken links is
    // reported in one pass (consistent with the `onBrokenLinks: 'error'`
    // contract in the issue spec). Warnings are emitted to stderr so they
    // are visible to both the CLI user and CI log scanners.
    //
    // Error-mode findings join `fatal_findings` (declared above 2c) rather
    // than bailing here, so both this gate and the markdown-diagnostics
    // gate report before the single combined bail below.
    if !all_broken_links.is_empty() {
        let on_broken = input
            .resolve_markdown_links
            .as_ref()
            .map(|s| s.on_broken_links)
            .unwrap_or(OnBrokenLinks::Warn);
        match on_broken {
            OnBrokenLinks::Ignore => {}
            OnBrokenLinks::Warn => {
                for (file, url) in &all_broken_links {
                    eprintln!(
                        "zfb warn: broken markdown link in {file}: \
                         {url} could not be resolved to a known doc URL"
                    );
                }
            }
            OnBrokenLinks::Error => {
                let mut msg = format!(
                    "bundler: {} broken markdown link(s) found:\n",
                    all_broken_links.len()
                );
                for (file, url) in &all_broken_links {
                    msg.push_str(&format!("  {file}: {url}\n"));
                }
                msg.push_str(
                    "Fix the links or set onBrokenLinks: 'warn' / 'ignore' \
                     in resolveMarkdownLinks config to suppress this error.",
                );
                fatal_findings.push(msg);
            }
        }
    }

    // Single combined bail for 2c + 2d: every fatal finding from both gates
    // is in the error, so one failed build shows the complete picture.
    if !fatal_findings.is_empty() {
        bail!("{}", fatal_findings.join("\n"));
    }

    // 2e. Project-root `mdx-components.tsx` global override map (#616).
    //     A root-level FILE is not materialised by any pass above, so copy
    //     it into the shadow root here. The returned spec is threaded into
    //     `write_entry_module`, which emits the `import` + the
    //     `globalThis.__zfb.mdxComponents` installer. `None` when the file
    //     is absent — keeps output byte-for-byte identical to a project
    //     without the convention.
    let mdx_components_import_spec: Option<String> = match input.mdx_components_file.as_ref() {
        Some(src) => materialise_mdx_components_file(src, shadow, &mat_ctx)
            .context("bundler: failed materialising mdx-components.tsx into shadow")?,
        None => None,
    };

    // 2e-root. Project-root loose-file staging (issue #1692). A wildcard root
    // alias (`"@/*": ["./*"]`) lets esbuild resolve `@/<name>` to a file
    // sitting directly in the project root. With `bundle.exclude` active the
    // tsconfig is rewritten shadow-only (no live-tree fallback), so those loose
    // files must be present in the shadow or the import fails with `Could not
    // resolve` (the real-site #1685 breakage). Gated on exclusions-active so
    // the empty-exclude path stays byte-identical (there the dual-target
    // tsconfig still resolves such imports against the real root), AND on a
    // wildcard root alias actually existing so nothing is staged needlessly.
    if !bundle_exclude.is_empty()
        && has_wildcard_root_alias(
            &project_root,
            &input.tsconfig_paths,
            &input.plugin_alias_entries,
        )
    {
        stage_project_root_loose_files(&input.project_root, shadow, &mat_ctx)
            .context("bundler: failed staging project-root loose files into shadow")?;
    }

    // 2f. Exact user/plugin targets can point at root-level files or paths
    // absent from the conventional source-root walks above. Materialise the
    // complete plugin-aware first-party closure explicitly so `?raw` wrappers and
    // nested Worker URL rewrites are consumed from the shadow. The resolver
    // call in `run_esbuild` remaps an alias to this copy when it exists.
    let is_plugin_preprocessing_excluded = |path: &Path| {
        mat_ctx
            .bundle_exclude
            .is_excluded(path, mat_ctx.project_root)
    };
    for logical_root in &exact_target_staging_dirs {
        let isolated_node_modules =
            project_path_is_inside_node_modules(logical_root, &project_root);
        // Under exclusions there is no isolation root, so node_modules targets
        // fall back to the main (session-aware) shadow writer + `<shadow>` root.
        let target_writer = if isolated_node_modules {
            node_modules_isolation_writer
                .as_ref()
                .unwrap_or(mat_ctx.writer)
        } else {
            mat_ctx.writer
        };
        let dest = shadow_path_for_project_path(
            logical_root,
            &project_root,
            &first_party_root,
            shadow,
            work,
            node_modules_isolation_root,
        );
        let prune_workspace_infra = logical_root.canonicalize().is_ok_and(|canonical| {
            canonical_workspace_package_logical_path(&canonical, &project_root).is_some()
        });
        materialise_isolated_exact_dir(
            logical_root,
            logical_root,
            &dest,
            target_writer,
            &is_plugin_preprocessing_excluded,
            prune_workspace_infra,
        )
        .with_context(|| {
            format!(
                "bundler: stage isolated exact-target directory {}",
                logical_root.display()
            )
        })?;
    }
    for (logical_root, source_root) in &exact_target_staging_alias_dirs {
        // Alias dirs are always node_modules-shaped; under exclusions they fall
        // back to the main shadow writer (no isolation root allocated).
        let target_writer = node_modules_isolation_writer
            .as_ref()
            .unwrap_or(mat_ctx.writer);
        let dest = shadow_path_for_project_path(
            logical_root,
            &project_root,
            &first_party_root,
            shadow,
            work,
            node_modules_isolation_root,
        );
        let prune_workspace_infra = source_root.canonicalize().is_ok_and(|canonical| {
            canonical_workspace_package_logical_path(&canonical, &project_root).is_some()
        });
        materialise_isolated_exact_dir(
            source_root,
            logical_root,
            &dest,
            target_writer,
            &is_plugin_preprocessing_excluded,
            prune_workspace_infra,
        )
        .with_context(|| {
            format!(
                "bundler: stage isolated dependency {} at {}",
                source_root.display(),
                logical_root.display()
            )
        })?;
    }
    for physical in &exact_target_staging_files {
        if is_plugin_preprocessing_excluded(physical) {
            continue;
        }
        let isolated_node_modules = project_path_is_inside_node_modules(physical, &project_root);
        // No isolation root under exclusions → node_modules candidates stage
        // into `<shadow>` at their logical paths via the main shadow writer.
        let target_root = if isolated_node_modules {
            node_modules_isolation_root.unwrap_or(shadow)
        } else {
            shadow
        };
        let target_writer = if isolated_node_modules {
            node_modules_isolation_writer
                .as_ref()
                .unwrap_or(mat_ctx.writer)
        } else {
            mat_ctx.writer
        };
        let to = shadow_path_for_project_path(
            physical,
            &project_root,
            &first_party_root,
            shadow,
            work,
            node_modules_isolation_root,
        );
        let shadow_relative = to
            .strip_prefix(target_root)
            .expect("exact-target staging destination remains inside shadow");
        let mut shadow_parent = target_root.to_path_buf();
        for component in shadow_relative
            .parent()
            .into_iter()
            .flat_map(Path::components)
        {
            shadow_parent.push(component.as_os_str());
            target_writer.ensure_dir(&shadow_parent)?;
        }
        target_writer
            .copy_if_changed(physical, &to)
            .with_context(|| {
                format!(
                    "bundler: copy raw exact-target candidate {}",
                    physical.display()
                )
            })?;
    }
    let excluded_plugin_preprocessing_files = plugin_preprocessing_files
        .iter()
        .filter(|path| is_plugin_preprocessing_excluded(path))
        .cloned()
        .collect::<BTreeSet<_>>();
    // Reuses the widened first-party root computed once for the shadow
    // re-root above (issue #1668); the guard below keys off the same boundary.
    for physical in &plugin_preprocessing_files {
        if excluded_plugin_preprocessing_files.contains(physical) {
            continue;
        }
        let under_project = physical.strip_prefix(&input.project_root).is_ok();
        // Issue #1668 part 2: a workspace-sibling first-party file (outside the
        // project but inside the widened first-party root) is now STAGED
        // uniformly through the same materialise pass as a project file — at its
        // workspace-relative slot in the WORK mirror (see
        // `shadow_path_for_project_path`) — so its `?raw`/glob/nested-worker
        // rewrites land in the shadow just like a project file's. This replaces
        // the pre-#1668 part-1 guard that rejected such siblings.
        if !under_project {
            if !physical.starts_with(&first_party_root) {
                // Beyond the widened first-party boundary — a true escape the
                // graph walk should already have rejected; fail loudly.
                return Err(anyhow!(
                    "bundler: plugin preprocessing file {} escaped {}",
                    physical.display(),
                    input.project_root.display()
                ));
            }
            if path_is_inside_node_modules(physical) {
                // A workspace-hoisted dependency, NOT first-party source: it
                // resolves through the `<work>/node_modules` link, never staged
                // (matching the pre-#1668 real-tree escape for node_modules).
                continue;
            }
            // Otherwise a workspace-sibling first-party file — fall through and
            // stage it uniformly below.
        }
        let workspace_sibling = !under_project;
        let isolated_node_modules = project_path_is_inside_node_modules(physical, &project_root);
        // A workspace sibling is plain first-party source (node_modules siblings
        // `continue`d above), so it stages through the main writer at the WORK
        // root; project files keep the shadow root, and project-local
        // node_modules keep the isolation root. No isolation root under
        // exclusions → node_modules preprocessing files stage into `<shadow>` at
        // their logical paths via the main shadow writer.
        let target_root = if isolated_node_modules {
            node_modules_isolation_root.unwrap_or(shadow)
        } else if workspace_sibling {
            work
        } else {
            shadow
        };
        let target_writer = if isolated_node_modules {
            node_modules_isolation_writer
                .as_ref()
                .unwrap_or(mat_ctx.writer)
        } else {
            mat_ctx.writer
        };
        let to = shadow_path_for_project_path(
            physical,
            &project_root,
            &first_party_root,
            shadow,
            work,
            node_modules_isolation_root,
        );
        let shadow_relative = to
            .strip_prefix(target_root)
            .expect("project staging destination remains inside shadow");
        // This explicit closure exists precisely for files outside the broad
        // source-root walks, so its parent directories may not exist in the
        // shadow yet (notably for hidden/nested plugin targets). Create each
        // component separately so `ShadowWriter::ensure_dir` can remove an
        // intermediate symlink before a deeper `create_dir_all` could follow
        // it out of the shadow. Directories are intentionally absent from the
        // session's file-based visited/prune bookkeeping.
        let mut shadow_parent = target_root.to_path_buf();
        for component in shadow_relative
            .parent()
            .into_iter()
            .flat_map(Path::components)
        {
            shadow_parent.push(component.as_os_str());
            target_writer.ensure_dir(&shadow_parent).with_context(|| {
                format!(
                    "bundler: create plugin preprocessing parent {}",
                    shadow_parent.display()
                )
            })?;
        }
        let materialised = materialise_source_file(
            physical,
            physical,
            &to,
            &is_plugin_preprocessing_excluded,
            if isolated_node_modules {
                true
            } else {
                mat_ctx.copy_mode
            },
            target_writer,
            &mat_ctx.glob_matched_files,
            &mat_ctx.raw_import_edges,
            &mat_ctx.raw_import_aliases,
            &mat_ctx.module_worker_dependencies,
            mat_ctx.project_root,
            &mat_ctx.worker_build_context,
        );
        match materialised {
            Ok(()) => {}
            // Issue #1724: a WHOLESALE mirror enrolment is best-effort — see
            // the enrolment loop's comment. The mirror's raw copy is already
            // on disk and already recorded visited, so falling back to it is
            // exactly the pre-#1724 outcome for a file no import edge reaches.
            Err(error) if mirror_derived_preprocessing_files.contains(physical) => {
                // "Not reached by any import edge" is the JUSTIFICATION for
                // the fallback, not something this code can verify — the
                // wholesale mirror exists precisely because Rust discovery
                // cannot predict what esbuild will reach. So if the file IS
                // reached, #1724 silently reinstates: the raw copy ships with
                // its macros unexpanded. Warn through both channels (the
                // crate's `skip_dangling_symlink_or_fail` idiom: no
                // `tracing_subscriber` is installed in the `zfb` binary, so
                // `tracing::warn!` alone is invisible to real CLI users) so
                // that reinstatement leaves a trace instead of nothing.
                // Deliberately NOT fatal — a mirror root is claimed
                // wholesale, and an unreached fixture must not become a build
                // failure it was not before #1724.
                let msg = format!(
                    "keeping the wholesale sibling mirror's raw copy of {} — preprocessing it \
                     failed ({}). This is only safe if no import edge reaches the file; if one \
                     does, its `import.meta.glob` / `?raw` / module-worker macros ship \
                     UNEXPANDED (issue #1724).",
                    physical.display(),
                    format_args!("{error:#}"),
                );
                tracing::warn!("{msg}");
                eprintln!("zfb warn: {msg}");
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "bundler: materialise plugin preprocessing file {}",
                        physical.display()
                    )
                });
            }
        }
    }

    // 2f. Issue #1695: `import.meta.glob` matched-files fixed-point drain.
    // By this point every materialise pass above has run at least once this
    // call, so `mat_ctx.glob_matched_files` holds the union of every glob
    // match discovered anywhere in the shadow tree so far; drain it to a
    // fixed point. See [`stage_glob_matched_files_to_fixed_point`] for why
    // every match is staged here (including ones inside an already-claimed
    // `SiblingMirrorPlan` region) and the full queue shape.
    stage_glob_matched_files_to_fixed_point(
        &mat_ctx,
        &project_root,
        &first_party_root,
        shadow,
        work,
        node_modules_isolation_root,
    )?;

    // 2g. CSS Modules — run after every explicit exact-target/package copy
    //     so raw staged `.module.css` files cannot overwrite the rewrite.
    //     Paired with `--loader:.module.css=js`, this emits scoped class maps.
    rewrite_css_modules_in_shadow(
        shadow,
        &input.project_root,
        &input.css_module_class_maps,
        &writer,
    )
    .context("bundler: failed rewriting CSS Modules in shadow tree")?;
    // Issue #1697: the sibling half of the same pass — a claimed sibling
    // `.module.css` lands under `work` outside the project mirror `shadow`,
    // which the walk above never visits. Gated on `workspace_rel.is_some()`
    // (⇔ `shadow` is nested under `work`, not equal to it) so a non-workspace
    // build never enters here and stays byte-identical.
    if workspace_rel.is_some() {
        rewrite_css_modules_in_work_mirror(
            work,
            shadow,
            &first_party_root,
            &input.css_module_class_maps,
            &writer,
        )
        .context("bundler: failed rewriting CSS Modules in work mirror")?;
    }
    if let (Some(isolation_root), Some(isolation_writer)) = (
        node_modules_isolation_root,
        node_modules_isolation_writer.as_ref(),
    ) {
        rewrite_css_modules_in_shadow(
            isolation_root,
            &input.project_root,
            &input.css_module_class_maps,
            isolation_writer,
        )
        .context("bundler: failed rewriting CSS Modules in node_modules isolation")?;
    }

    // 3. Hydration shim.
    //
    // Always-write infra file: written unconditionally every call (like
    // the tsconfig and entry.mjs below), so it bypasses the #993
    // ShadowWriter — never recorded as visited, therefore never eligible
    // for the prune pass, which only deletes previously-visited paths.
    let shim_path = shadow.join(SHADOW_HYDRATE_FILENAME);
    fs::write(&shim_path, adapter.hydrate_shim_source()).with_context(|| {
        format!(
            "bundler: failed writing hydration shim to {}",
            shim_path.display()
        )
    })?;

    // 4. Synthetic tsconfig.json honouring the user's `paths`. Rebase
    //    under-project_root alias targets to a shadow-first dual-target so
    //    an aliased import reaches the in-shadow transform (see
    //    `rebase_tsconfig_paths_to_shadow`). This first write is overwritten
    //    by the plugin-merged write inside `run_esbuild` for real builds; it
    //    matters for the mock-subprocess path and as a behaviour-preserving
    //    baseline. esbuild reads the array in order, first existing file
    //    wins, a miss is NOT an error — that fallthrough is what makes the
    //    real-root fallback safe.
    let rebased_paths =
        rebase_tsconfig_paths_to_shadow(&input.tsconfig_paths, &input.project_root, shadow);
    write_synthetic_tsconfig(shadow, &rebased_paths, adapter.jsx_import_source())
        .context("bundler: failed writing synthetic tsconfig.json")?;

    // 5. Synthetic entry.mjs.
    //
    // When `worker_only_routes` is set, narrow the entry's static-import
    // set to that subset and drop `content_imports` + the content snapshot
    // (see `BundlerInput::worker_only_routes` for the contract). This
    // makes prerendered-only routes unreachable from the entry so esbuild's
    // tree-shaker can drop them and their transitive deps (page modules,
    // MDX namespaces, inline data URIs) from the final bundle. The full
    // `routes` vec is preserved for `BundlerOutput::manifest.routes` —
    // build-time bookkeeping (route table, post-build manifest) must keep
    // seeing every discovered route regardless of this filter.
    let entry_routes_filtered_storage: Vec<RouteEntry>;
    let entry_routes_for_write: &[RouteEntry];
    let entry_content_imports_for_write: &[ContentImport];
    let entry_snapshot_for_write: Option<&str>;
    if let Some(filter) = input.worker_only_routes.as_ref() {
        // `entry_key` is Hono-form (`/blog/:slug{.+}`); `worker_only_routes`
        // is also Hono-form (populated from `route.template()`). The
        // string-equality check therefore matches correctly for every route
        // shape, including catch-alls.
        entry_routes_filtered_storage = routes
            .iter()
            .filter(|r| filter.contains(&r.entry_key))
            .cloned()
            .collect();
        entry_routes_for_write = &entry_routes_filtered_storage;
        // Content imports + snapshot are intentionally dropped from the
        // runtime bundle slice; see field docs for the rationale and the
        // listed follow-up if/when an SSR route legitimately needs
        // getCollection(...).
        entry_content_imports_for_write = &[];
        entry_snapshot_for_write = None;
    } else {
        // No filter — preserve the existing single-bundle behavior verbatim.
        entry_routes_for_write = &routes;
        entry_content_imports_for_write = &content_imports;
        entry_snapshot_for_write = input.content_snapshot_json.as_deref();
    }
    write_entry_module(
        shadow,
        entry_routes_for_write,
        &EntryModuleInputs {
            render_to_string_module: adapter.render_to_string_module(),
            content_snapshot_json: entry_snapshot_for_write,
            content_imports: entry_content_imports_for_write,
            site: input.site.as_deref(),
            prefetch_disabled: input.prefetch_disabled,
            // Emitted independently of `content_imports` / `worker_only_routes`:
            // a project may define overrides with zero content entries (#616).
            mdx_components_import_spec: mdx_components_import_spec.as_deref(),
            // Emitted only when at least one client-script entry was discovered
            // (#978). `None` keeps zero-script builds byte-identical.
            base_prefix: input.base_prefix.as_deref(),
        },
    )
    .context("bundler: failed writing entry.mjs")?;

    // 5b. Prune stale shadow files (#993 — session mode only, no-op
    //     otherwise). MUST run before esbuild: a deleted/renamed/newly-
    //     excluded source's compiled artifact would otherwise stay in the
    //     persistent tree and esbuild would bundle a module a fresh build
    //     would not (the #727 wrong-output hazard family). Runs after ALL
    //     materialise passes so the visited set is complete; the
    //     `.zfb-virtual-*.mjs` NamedTempFiles are created inside
    //     `run_esbuild` AFTER this pass and self-delete — they never
    //     interact with the prune.
    writer.prune_stale()?;
    let materialise_ms = materialise_start.map(|t| t.elapsed().as_millis());

    // 6. Resolve and run esbuild (or the mock).
    let esbuild_start = if timing_enabled {
        Some(std::time::Instant::now())
    } else {
        None
    };
    fs::create_dir_all(&outdir)
        .with_context(|| format!("bundler: failed to create outdir {}", outdir.display()))?;
    // Bundle filename — `bundle_basename` lets callers run two bundle()
    // passes in the same outdir (full SSG vs runtime-only) without clobber.
    let bundle_filename: &str = input.bundle_basename.as_deref().unwrap_or("bundle.mjs");
    let bundle_path = outdir.join(bundle_filename);
    let sourcemap_path = outdir.join(format!("{bundle_filename}.map"));

    // Per-route transitive `Module` deps use esbuild's metafile only in dev
    // (#1284/#1287). The same metafile now also owns the Wasm deployment-asset
    // manifest for every real bundle pass, including the production SSG and
    // runtime-only passes. It must be read before `owned_work` (the shadow) is
    // dropped at the end of this function.
    let want_metafile_deps = matches!(input.mode, BundleMode::Development);
    let exclusions_active = !bundle_exclude.is_empty();
    // Mock-subprocess builds bypass esbuild and therefore cannot produce a
    // trustworthy metafile. Real invocations always request one so both bundle
    // passes publish the exact Wasm assets their emitted ESM imports require.
    let metafile_path: Option<PathBuf> = if input.mock_subprocess_output.is_none() {
        Some(shadow.join(".zfb-metafile.json"))
    } else {
        None
    };

    if let Some(mock) = input.mock_subprocess_output.as_ref() {
        fs::write(&bundle_path, mock).with_context(|| {
            format!(
                "bundler: failed to write mock bundle to {}",
                bundle_path.display()
            )
        })?;
    } else {
        run_esbuild(
            &input,
            shadow,
            &first_party_root,
            work,
            &bundle_path,
            metafile_path.as_deref(),
            &bundle_exclude,
            node_modules_isolation_root,
        )?;
    }
    let esbuild_ms = esbuild_start.map(|t| t.elapsed().as_millis());

    // Fail-closed `bundle.exclude` audit (#1558): whenever exclusions are
    // active, verify esbuild's metafile — the only resolver, per this
    // project's lessons-learned — recorded no input resolving to an excluded
    // path under either spelling `audit_metafile_exclusions_at_path` checks.
    // A leaked exclusion here means a live-tree escape hatch (dual-target
    // tsconfig paths, a stray node_modules symlink, esbuild candidate
    // substitution) let excluded content back into the bundle. Runs while
    // the shadow tree still exists (`owned_work` is dropped further below).
    // `metafile_path` is `None` here for the mock-subprocess path even under
    // active exclusions (see above), so this block is a deliberate no-op for
    // mocks, not a hole.
    if exclusions_active {
        if let Some(meta_path) = metafile_path.as_deref() {
            let is_excluded = |abs: &Path| bundle_exclude.is_excluded(abs, &input.project_root);
            crate::metafile_deps::audit_metafile_exclusions_at_path(
                meta_path,
                &is_excluded,
                shadow,
                &input.project_root,
            )
            .context("bundler: bundle.exclude audit failed")?;
        }
    }

    // Fail-closed stage-escape audit (#1706, Stage Escape Guards — Guard (b)):
    // the wholesale `<work>/node_modules` symlink the work mirror sets up
    // (created for EVERY workspace build since #1693 — empty or active
    // exclusions) lets a bare first-party PACKAGE-NAME import climb straight to
    // a LIVE workspace sibling, unprocessed source outside the staged mirror.
    // Unlike the `bundle.exclude` audit above (armed only under active
    // exclusions), this escape exists regardless of `bundle.exclude`, so the
    // audit runs whenever [`zfb_types::stage_escape_audit_eligibility`] (issue
    // #1986) says a first-party stage escape is structurally possible.
    //
    // Issue #1730/#1988: this used to gate on `workspace_rel.is_some()` alone
    // — the widened-stage proxy — which reads a workspace whose
    // `pnpm-workspace.yaml` claims its own root (`packages: ['.',
    // 'packages/*']`) as "not a workspace": building FROM the workspace root
    // makes `first_party_root_for` return `project_root` itself, so
    // `workspace_rel` is `None` even though the wholesale `<work>/node_modules`
    // link this stage sets up (above) reaches first-party siblings exactly as
    // it does from a nested member. SSR has no scan-time guard (a) at all, so
    // this was a completely SILENT escape — the epic's P1 leg
    // (`bundler_root_workspace_stage_escape_audit_disabled_regression.rs`).
    // The eligibility predicate is a strict superset of the old proxy (see its
    // module docs for the full decision table): every currently-armed build
    // stays armed, and a root-claimed workspace with a reachable first-party
    // `node_modules` link now arms too. `work.join("node_modules")` is the
    // wholesale symlink set up above (to `first_party_root`'s live install),
    // so it is what the predicate scans — mirroring the islands/client site's
    // `stage_escape_audit_policy` (`crates/zfb/src/commands/build.rs`), which
    // scans its own stage root's `node_modules` the same way.
    //
    // esbuild runs from `shadow` (its cwd — `cmd.current_dir(shadow)` in
    // `run_esbuild`), so metafile keys resolve against it; `work` is the
    // mirror boundary every legitimately staged input (the project shadow at
    // `work/<workspace_rel>` plus wholesale-mirrored siblings under `work`)
    // lives under. A metafile input that canonicalises to live first-party
    // source outside `work` with no staged spelling — or a staged
    // `node_modules/@scope/pkg` symlink resolving to a live sibling — is the
    // escape and hard-fails the build. `metafile_path` is `None` on the
    // mock-subprocess path, so this is a deliberate no-op for mocks, like the
    // exclusion audit.
    if zfb_types::stage_escape_audit_eligibility(
        &input.project_root,
        &first_party_root,
        &work.join("node_modules"),
    )
    .is_eligible()
    {
        if let Some(meta_path) = metafile_path.as_deref() {
            crate::metafile_deps::audit_metafile_stage_escape_at_path(
                meta_path,
                shadow,
                &[work],
                &first_party_root,
            )
            .context("bundler: SSR work-mirror stage-escape audit failed")?;
        }
    }

    // Read once while the shadow still exists. A malformed or missing metafile
    // remains best-effort for the dev invalidation graph, but is fail-closed
    // for an emitted bundle that references a copied Wasm module.
    let metafile_bytes = metafile_path.as_ref().and_then(|path| fs::read(path).ok());
    let emitted_wasm_assets = match metafile_path.as_deref() {
        Some(meta_path) => emitted_wasm_assets_from_metafile(
            meta_path,
            metafile_bytes.as_deref(),
            shadow,
            &outdir,
            &bundle_path,
        )?,
        None => Vec::new(),
    };

    // Parse the metafile into per-route `Module` edges while the shadow tree
    // still exists. This remains development-only and best-effort: a missing
    // or malformed metafile falls back to the previous empty dependency set.
    let mut route_module_deps: Vec<crate::metafile_deps::RouteModuleDeps> = if want_metafile_deps {
        match metafile_bytes.as_deref() {
            Some(bytes) => {
                let route_refs: Vec<crate::metafile_deps::RouteEntryRef> = routes
                    .iter()
                    .filter(|r| !r.static_html)
                    .map(|r| crate::metafile_deps::RouteEntryRef {
                        source_path: r.source_path.clone(),
                        // The shadow mirrors the project tree by relative path,
                        // so a route's metafile-input key equals its
                        // project-relative source path in forward-slash form.
                        metafile_key: rel_to_forward_slash(&r.source_path),
                    })
                    .collect();
                crate::metafile_deps::route_module_deps(
                    bytes,
                    &route_refs,
                    shadow,
                    &input.project_root,
                )
            }
            None => Vec::new(),
        }
    } else {
        Vec::new()
    };
    augment_route_deps_with_raw_targets(
        &mut route_module_deps,
        &mat_ctx.raw_import_edges.borrow(),
        &input.project_root,
    );
    augment_route_deps_with_worker_targets(
        &mut route_module_deps,
        &mat_ctx.module_worker_dependencies.borrow(),
        &input.project_root,
    );

    let post_start = if timing_enabled {
        Some(std::time::Instant::now())
    } else {
        None
    };
    let manifest = BundleManifest {
        framework: adapter.name().to_string(),
        jsx_import_source: adapter.jsx_import_source().to_string(),
        hydrate_shim_specifier: adapter.hydrate_shim_specifier().to_string(),
        bundle_basename: bundle_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("bundle.mjs")
            .to_string(),
        routes,
    };
    let post_ms = post_start.map(|t| t.elapsed().as_millis());

    // The whole call succeeded — the session's shadow tree exactly matches
    // this call's inputs, so the next call may reuse it (dirty cleared).
    writer.mark_clean();

    // Teardown — the shadow TempDir's recursive delete. Dropped explicitly
    // here (instead of implicitly at scope exit) so the unlink storm is
    // measurable; behavior is identical (this is already the last use).
    // Session mode: `owned_work` is `None` (the persistent tree outlives
    // the call), so teardown is ~0ms by construction.
    let teardown_start = if timing_enabled {
        Some(std::time::Instant::now())
    } else {
        None
    };
    drop(owned_work);
    let teardown_ms = teardown_start.map(|t| t.elapsed().as_millis());

    if timing_enabled {
        eprintln!(
            "[zfb-timing] bundle(): materialise={}ms esbuild={}ms post={}ms teardown={}ms",
            materialise_ms.unwrap_or(0),
            esbuild_ms.unwrap_or(0),
            post_ms.unwrap_or(0),
            teardown_ms.unwrap_or(0),
        );
    }

    Ok(BundlerOutput {
        bundle_path,
        sourcemap_path,
        manifest,
        route_module_deps,
        emitted_wasm_assets,
    })
}

/// Replace the generated-wrapper blind spot in esbuild's metafile with the
/// original terminal target path. A route consumes a raw edge when its own
/// page module is the importer or its metafile-derived module closure already
/// contains that importer. This keeps invalidation precise per route while
/// ensuring a target-only edit dirties the consumer.
fn augment_route_deps_with_raw_targets(
    route_deps: &mut [crate::metafile_deps::RouteModuleDeps],
    raw_edges: &BTreeSet<RawImportEdge>,
    project_root: &Path,
) {
    if raw_edges.is_empty() {
        return;
    }
    let global_mdx_aliases: BTreeSet<PathBuf> =
        path_aliases(&project_root.join(MDX_COMPONENTS_FILENAME)).collect();
    for route in route_deps {
        let route_source = project_root.join(&route.source_path);
        let route_aliases: BTreeSet<PathBuf> = path_aliases(&route_source).collect();
        for edge in raw_edges {
            let importer_aliases: BTreeSet<PathBuf> = path_aliases(&edge.importer).collect();
            let consumes_importer = !importer_aliases.is_disjoint(&global_mdx_aliases)
                || importer_aliases.iter().any(|importer| {
                    route_aliases.contains(importer) || route.module_deps.contains(importer)
                });
            if consumes_importer {
                route.module_deps.extend(path_aliases(&edge.target));
            }
        }
    }
}

/// Add browser-only worker closure paths to the consuming route's invalidation
/// set without adding them to the SSR module graph. Each dependency is
/// projected through the parent importer that owned the rewritten URL; nested
/// worker sources and their first-party imports are already flattened onto
/// that importer by [`rewrite_module_worker_urls`].
fn augment_route_deps_with_worker_targets(
    route_deps: &mut [crate::metafile_deps::RouteModuleDeps],
    worker_dependencies: &BTreeSet<ModuleWorkerDependency>,
    project_root: &Path,
) {
    if worker_dependencies.is_empty() {
        return;
    }
    let global_mdx_aliases: BTreeSet<PathBuf> =
        path_aliases(&project_root.join(MDX_COMPONENTS_FILENAME)).collect();
    for route in route_deps {
        let route_source = project_root.join(&route.source_path);
        let route_aliases: BTreeSet<PathBuf> = path_aliases(&route_source).collect();
        for edge in worker_dependencies {
            let importer_aliases: BTreeSet<PathBuf> = path_aliases(&edge.importer).collect();
            let consumes_importer = !importer_aliases.is_disjoint(&global_mdx_aliases)
                || importer_aliases.iter().any(|importer| {
                    route_aliases.contains(importer) || route.module_deps.contains(importer)
                });
            if consumes_importer {
                route.module_deps.extend(path_aliases(&edge.dependency));
            }
        }
    }
}

/// The subset of esbuild's metafile needed to discover copied deployment
/// assets. `outputs` is deliberately required: a missing section is not a
/// valid manifest for a bundle that contains a Wasm import.
#[derive(Debug, Deserialize)]
struct EsbuildAssetMetafile {
    outputs: BTreeMap<String, serde_json::Value>,
}

/// Build the strict, bundle-relative Wasm asset manifest from esbuild's
/// metafile. A malformed or missing metafile remains non-fatal for a
/// Wasm-free bundle to preserve the existing best-effort dev dependency graph,
/// but it is a hard error once the emitted ESM references a copied `.wasm`
/// file: deployment would otherwise silently omit a required Worker module.
fn emitted_wasm_assets_from_metafile(
    metafile_path: &Path,
    metafile_bytes: Option<&[u8]>,
    metafile_cwd: &Path,
    outdir: &Path,
    bundle_path: &Path,
) -> Result<Vec<PathBuf>> {
    let Some(metafile_bytes) = metafile_bytes else {
        if bundle_references_wasm(bundle_path)? {
            bail!(
                "bundler: wasm asset manifest is unavailable: esbuild did not write {} for a bundle that imports Wasm",
                metafile_path.display()
            );
        }
        return Ok(Vec::new());
    };

    let wasm_output_keys = match wasm_output_keys_from_metafile(metafile_bytes) {
        Ok(keys) => keys,
        Err(error) => {
            if bundle_references_wasm(bundle_path)? {
                return Err(error.context(format!(
                    "bundler: wasm asset manifest is malformed at {} for a bundle that imports Wasm",
                    metafile_path.display()
                )));
            }
            return Ok(Vec::new());
        }
    };

    if wasm_output_keys.is_empty() {
        if bundle_references_wasm(bundle_path)? {
            bail!(
                "bundler: wasm asset manifest at {} listed no .wasm output for a bundle that imports Wasm",
                metafile_path.display()
            );
        }
        return Ok(Vec::new());
    }

    validate_wasm_asset_output_paths(&wasm_output_keys, metafile_cwd, outdir)
}

fn bundle_references_wasm(bundle_path: &Path) -> Result<bool> {
    // A bare `.wasm` substring is not sufficient: user-facing messages,
    // comments, and template literals can all contain one without requiring a
    // deployable module. Reuse the existing static-ESM parser so this guard
    // only trips for a real emitted module dependency.
    let specifiers = crate::module_worker::collect_runtime_import_specifiers_from_file(bundle_path)
        .with_context(|| {
            format!(
                "bundler: failed to inspect emitted bundle {} for Wasm ESM imports",
                bundle_path.display()
            )
        })?;
    Ok(specifiers.into_iter().any(|specifier| {
        Path::new(&specifier)
            .extension()
            .is_some_and(|extension| extension == "wasm")
    }))
}

fn wasm_output_keys_from_metafile(metafile_bytes: &[u8]) -> Result<Vec<String>> {
    let metafile: EsbuildAssetMetafile =
        serde_json::from_slice(metafile_bytes).context("failed to parse esbuild metafile")?;
    Ok(metafile
        .outputs
        .into_keys()
        .filter(|key| Path::new(key).extension().is_some_and(|ext| ext == "wasm"))
        .collect())
}

/// Validate each metafile output path before exposing it to deployment code.
/// Esbuild normally writes output keys relative to its current working
/// directory, but accepts absolute outfile paths too; support both spellings
/// while requiring the resolved, existing asset to remain under `outdir`.
fn validate_wasm_asset_output_paths(
    output_keys: &[String],
    metafile_cwd: &Path,
    outdir: &Path,
) -> Result<Vec<PathBuf>> {
    let canonical_outdir = fs::canonicalize(outdir).with_context(|| {
        format!(
            "bundler: failed to canonicalize wasm asset output directory {}",
            outdir.display()
        )
    })?;
    let mut assets = BTreeSet::new();

    for output_key in output_keys {
        let output_path = resolve_metafile_output_path(output_key, metafile_cwd, outdir);
        let canonical_output = fs::canonicalize(&output_path).with_context(|| {
            format!(
                "bundler: wasm asset listed by esbuild metafile does not exist: {}",
                output_path.display()
            )
        })?;
        let relative = canonical_output.strip_prefix(&canonical_outdir).map_err(|_| {
            anyhow!(
                "bundler: wasm asset listed by esbuild metafile escapes bundle output directory: {} (outdir {})",
                canonical_output.display(),
                canonical_outdir.display()
            )
        })?;
        if relative.as_os_str().is_empty() {
            bail!(
                "bundler: wasm asset listed by esbuild metafile resolves to the bundle output directory itself"
            );
        }
        assets.insert(relative.to_path_buf());
    }

    Ok(assets.into_iter().collect())
}

fn resolve_metafile_output_path(output_key: &str, metafile_cwd: &Path, outdir: &Path) -> PathBuf {
    let output_key = Path::new(output_key);
    if output_key.is_absolute() {
        return normalize_path_lexical(output_key);
    }

    let from_metafile_cwd = normalize_path_lexical(&metafile_cwd.join(output_key));
    if from_metafile_cwd.exists() {
        return from_metafile_cwd;
    }

    // Some esbuild integrations record output keys relative to the outdir.
    // The CLI normally uses the first spelling above, but accepting this form
    // keeps the validation tied to containment rather than path presentation.
    let from_outdir = normalize_path_lexical(&outdir.join(output_key));
    if from_outdir.exists() {
        return from_outdir;
    }

    from_metafile_cwd
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Helper: resolve a possibly-relative path against `project_root`.
struct PathResolver<'a> {
    project_root: &'a Path,
}

impl<'a> PathResolver<'a> {
    fn new(project_root: &'a Path) -> Self {
        Self { project_root }
    }
    fn resolve(&self, p: &Path) -> PathBuf {
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.project_root.join(p)
        }
    }
}

/// Returns `true` when a WalkDir entry is an infra directory that should
/// not be descended into. Used as the predicate for
/// `.filter_entry(|e| !is_pruned_infra_dir(e))` in `materialise_shadow`,
/// `materialise_collection`, and [`crate::glob_expand::glob_match_relative`]
/// (`pub(crate)` so the latter, in its own module, can reuse it).
///
/// Rules (per #428 Fix A):
/// - Non-directories are never pruned (return `false`).
/// - Named infra dirs at any depth: `node_modules`, `.git`, `.next`,
///   `.turbo`, `.vercel` are always skipped.
/// - Any hidden directory (name starts with `.`) at depth > 0 is skipped,
///   covering nested `.cache`, `.wrangler`, `.vite`, `.storybook`, etc.
///   The depth-0 guard ensures the walker root itself is never pruned even
///   if the caller happened to name it `.foo`.
/// - `dist` / `target` are NOT pruned at depth (caveat from #428 — a
///   legitimate sub-directory with those names should still be
///   materialised; top-level pruning is already in the 2a-extra block).
pub(crate) fn is_pruned_infra_dir(entry: &walkdir::DirEntry) -> bool {
    // True ⇒ skip descending into this entry.
    if !entry.file_type().is_dir() {
        return false;
    }
    let name = entry.file_name().to_string_lossy();
    // Always-prune named infra dirs at any depth.
    if matches!(
        name.as_ref(),
        "node_modules" | ".git" | ".next" | ".turbo" | ".vercel"
    ) {
        return true;
    }
    // Per #428 Fix A ("skip ... and dotdirs wherever it appears"): also
    // prune any hidden directory at depth. The existing top-level
    // extra-dirs loop already skips hidden dirs (name.starts_with('.')
    // at L994), so this only changes behaviour *inside* the materialised
    // roots — e.g. `layouts/.storybook/`, `pages/.cache/`,
    // `components/.vite/`. Covers nested `.cache`, `.wrangler`, `.vite`,
    // etc. without naming each one. NOTE: depth-0 is the walker root
    // itself; we deliberately do NOT prune the root even if it is named
    // (e.g.) `.foo`, because the caller chose to walk it.
    if entry.depth() > 0 && name.starts_with('.') {
        return true;
    }
    // Conservative: do NOT prune "dist" / "target" at depth (per #428
    // "Caveats" — a legitimate sub-directory with those names should
    // still be materialised). Top-level dist/target pruning is handled
    // by the existing skip-list in the 2a-extra block.
    false
}

/// Enumerate top-level directories under `project_root` that should be
/// materialised into the shadow tree as "extra dirs" (i.e., directories not
/// handled by the dedicated pages/content/components/layouts passes).
///
/// Uses the `ignore` crate's `WalkBuilder` (the same walker ripgrep uses)
/// so that the consumer's `.gitignore`, `.git/info/exclude`, and global git
/// ignore rules are respected automatically.  `require_git(false)` ensures
/// the filter still applies even when the consumer project is not a git repo.
///
/// Directories that match `known_skip_list` or that start with `.` are also
/// excluded, preserving the previous behaviour of the `fs::read_dir` loop.
fn enumerate_extra_top_level_dirs(project_root: &Path, known_skip_list: &[&str]) -> Vec<PathBuf> {
    use ignore::WalkBuilder;
    let walker = WalkBuilder::new(project_root)
        .max_depth(Some(1)) // immediate children only
        .standard_filters(true) // .gitignore + .git/info/exclude + global gitignore + hidden
        .require_git(false) // honor .gitignore even when consumer isn't a git repo
        .build();
    let mut out = Vec::new();
    for entry in walker.flatten() {
        if entry.path() == project_root {
            continue;
        }
        let ft = match entry.file_type() {
            Some(ft) => ft,
            None => continue,
        };
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        if known_skip_list.iter().any(|k| name == *k) {
            continue;
        }
        out.push(entry.path().to_path_buf());
    }
    out
}

/// Enumerate the [`KNOWN_FIRST_PARTY_STAGING_DIRS`] allowlist entries that
/// exist under `project_root` (issue #1840), as `(live absolute dir,
/// project-relative path)` pairs. DELIBERATELY bypasses the hidden-dir and
/// `.gitignore` filters every generic walker applies — that is the whole
/// point: these paths are conventionally gitignored, yet hold real first-party
/// source that must gain a staged spelling. Symlinked allowlist paths are NOT
/// followed (a symlink here could alias arbitrary trees into the stage; the
/// zudo-doc generator writes real directories).
fn enumerate_first_party_staging_dirs(project_root: &Path) -> Vec<(PathBuf, PathBuf)> {
    KNOWN_FIRST_PARTY_STAGING_DIRS
        .iter()
        .map(|rel| (project_root.join(rel), PathBuf::from(rel)))
        .filter(|(src, _)| {
            fs::symlink_metadata(src).is_ok_and(|meta| meta.is_dir())
                && src
                    .parent()
                    .is_some_and(|p| fs::symlink_metadata(p).is_ok_and(|meta| meta.is_dir()))
        })
        .collect()
}

/// Enumerate the TOP-LEVEL `*.json` meta files inside each
/// [`KNOWN_FIRST_PARTY_STAGING_JSON_DIRS`] dir (issue #1840), as `(live
/// absolute file, project-relative path)` pairs. Same deliberate
/// hidden/gitignore bypass as [`enumerate_first_party_staging_dirs`]; depth-1
/// regular `*.json` files only — nested files, non-JSON files, and symlinks
/// are never staged (narrow compatibility surface).
fn enumerate_first_party_staging_json_files(project_root: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut out = Vec::new();
    for dir_rel in KNOWN_FIRST_PARTY_STAGING_JSON_DIRS {
        let dir = project_root.join(dir_rel);
        // Same no-symlink boundary as the dir helper: `fs::read_dir` would
        // FOLLOW a symlinked `.zfb`, staging meta files from an arbitrary
        // external target.
        if !fs::symlink_metadata(&dir).is_ok_and(|meta| meta.is_dir()) {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|ft| ft.is_file()) {
                continue;
            }
            let name = entry.file_name();
            if !name.to_string_lossy().ends_with(".json") {
                continue;
            }
            out.push((entry.path(), Path::new(dir_rel).join(&name)));
        }
    }
    out.sort();
    out
}

/// Walk `root` with the same `ignore::WalkBuilder` machinery
/// [`enumerate_extra_top_level_dirs`] uses (`.gitignore` / `.git/info/exclude`
/// / global-git honored via `standard_filters`, `require_git(false)` so the
/// filter still applies outside a git repo, hidden entries pruned), but over
/// the WHOLE subtree (not `max_depth(1)`) and yielding FILES — the root-level
/// files included. This is the generalization the sibling wholesale mirror
/// (issue #1692) needs: the project pass enumerates top-level dirs and hands
/// each to `materialise_shadow`; the sibling pass walks a claimed mirror root
/// end-to-end. The `MIRROR_SKIP_DIRS` infra dirs are pruned at any depth, and
/// a `node_modules` component is never descended into (also re-checked
/// per-file by the caller).
fn enumerate_mirror_root_files(root: &Path) -> Vec<PathBuf> {
    use ignore::WalkBuilder;
    let walker = WalkBuilder::new(root)
        .standard_filters(true) // .gitignore + .git/info/exclude + global gitignore + hidden
        .require_git(false) // honor .gitignore even when the sibling isn't a git repo
        .filter_entry(|entry| {
            // Prune the infra skip-list directories at any depth. Non-dirs and
            // the walk root itself are always kept; `standard_filters` already
            // prunes hidden dirs and gitignored entries.
            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                let name = entry.file_name().to_string_lossy();
                if MIRROR_SKIP_DIRS.iter().any(|skip| name == *skip) {
                    return false;
                }
            }
            true
        })
        .build();
    let mut out = Vec::new();
    for entry in walker.flatten() {
        if entry.file_type().is_some_and(|ft| ft.is_file()) {
            out.push(entry.path().to_path_buf());
        }
    }
    out
}

/// Split off a trailing `/*` or `/**` glob segment from an alias `target` so a
/// wildcard target's DIRECTORY component becomes the claim path (issue #1691's
/// claim policy: for `@shared/* -> ../../lib/shared/*` the claim is
/// `../../lib/shared`). A concrete (non-wildcard) target claims itself. The
/// result is lexically normalized so it compares cleanly against the
/// (lexical, non-canonicalized) `first_party_root` / `project_root`.
fn alias_target_claim_path(target: &str) -> PathBuf {
    let trimmed = target
        .strip_suffix("/**")
        .or_else(|| target.strip_suffix("/*"))
        .unwrap_or(target);
    normalize_path_lexical(Path::new(trimmed))
}

fn concrete_alias_targets(specifier: &str, paths: &BTreeMap<String, Vec<String>>) -> Vec<PathBuf> {
    let mut matches = paths
        .iter()
        .filter_map(|(pattern, targets)| {
            if let Some((prefix, suffix)) = pattern.split_once('*') {
                if !specifier.starts_with(prefix)
                    || !specifier.ends_with(suffix)
                    || specifier.len() < prefix.len() + suffix.len()
                {
                    return None;
                }
                let capture = &specifier[prefix.len()..specifier.len() - suffix.len()];
                Some((false, prefix.len() + suffix.len(), capture, targets))
            } else if pattern == specifier {
                Some((true, pattern.len(), "", targets))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    matches.sort_by_key(|candidate| std::cmp::Reverse((candidate.0, candidate.1)));
    let Some((_, _, capture, targets)) = matches.first() else {
        return Vec::new();
    };
    targets
        .iter()
        .map(|target| normalize_path_lexical(Path::new(&target.replacen('*', capture, 1))))
        .collect()
}

fn resolve_concrete_alias_file(candidate: &Path) -> Option<PathBuf> {
    if candidate.is_file() {
        return Some(candidate.to_path_buf());
    }
    let name = candidate.file_name()?;
    for extension in SSR_RESOLVE_EXTENSIONS {
        let mut path = candidate.to_path_buf();
        let mut appended = name.to_os_string();
        appended.push(format!(".{extension}"));
        path.set_file_name(appended);
        if path.is_file() {
            return Some(path);
        }
    }
    if candidate.is_dir() {
        return resolve_concrete_alias_file(&candidate.join("index"));
    }
    None
}

fn workspace_package_root_for(path: &Path, workspace_root: &Path) -> PathBuf {
    let mut dir = path.parent().unwrap_or(path);
    loop {
        if dir.join("package.json").is_file() {
            return dir.to_path_buf();
        }
        if dir == workspace_root {
            return workspace_root.to_path_buf();
        }
        match dir.parent() {
            Some(parent) if parent.starts_with(workspace_root) => dir = parent,
            _ => return workspace_root.to_path_buf(),
        }
    }
}

fn workspace_root_tree(path: &Path, workspace_root: &Path) -> Option<PathBuf> {
    let relative = path.strip_prefix(workspace_root).ok()?;
    let first = relative.components().next()?;
    Some(workspace_root.join(first.as_os_str()))
}

fn runtime_alias_path_has_workspace_membership(
    path: &Path,
    workspace_root: &Path,
    project_root: &Path,
    root_package_claimed: bool,
) -> bool {
    let package_root = workspace_package_root_for(path, workspace_root);
    if package_root == workspace_root {
        root_package_claimed
    } else {
        zfb_types::first_party::workspace_claims_package(project_root, &package_root)
    }
}

fn canonical_runtime_alias_path(
    path: &Path,
    canonical_workspace_root: &Path,
    workspace_root: &Path,
) -> Option<PathBuf> {
    let canonical = path.canonicalize().ok()?;
    let relative = canonical.strip_prefix(canonical_workspace_root).ok()?;
    Some(normalize_path_lexical(&workspace_root.join(relative)))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RuntimeAliasClaimMode {
    Standard,
    ExactNonSourceLeaf,
}

impl RuntimeAliasClaimMode {
    fn for_runtime_target(path: &Path) -> Self {
        if raw_source_extension(path) {
            Self::Standard
        } else {
            Self::ExactNonSourceLeaf
        }
    }
}

fn runtime_alias_path_is_claimable(
    path: &Path,
    canonical_workspace_root: &Path,
    workspace_root: &Path,
    project_root: &Path,
    root_package_claimed: bool,
    bundle_exclude: &BundleExcludeMatcher,
    claim_mode: RuntimeAliasClaimMode,
) -> Result<bool> {
    if !path.starts_with(workspace_root)
        || !runtime_alias_claim_is_allowed(
            path,
            workspace_root,
            project_root,
            bundle_exclude,
            claim_mode,
        )?
    {
        return Ok(false);
    }
    let Some(canonical_logical) =
        canonical_runtime_alias_path(path, canonical_workspace_root, workspace_root)
    else {
        return Ok(false);
    };
    if path_is_inside_node_modules(&canonical_logical)
        || !runtime_alias_claim_is_allowed(
            &canonical_logical,
            workspace_root,
            project_root,
            bundle_exclude,
            claim_mode,
        )?
    {
        return Ok(false);
    }
    if path.starts_with(project_root) {
        return Ok(canonical_logical.starts_with(project_root));
    }
    Ok(runtime_alias_path_has_workspace_membership(
        path,
        workspace_root,
        project_root,
        root_package_claimed,
    ) && runtime_alias_path_has_workspace_membership(
        &canonical_logical,
        workspace_root,
        project_root,
        root_package_claimed,
    ))
}

fn runtime_alias_claim_is_allowed(
    path: &Path,
    workspace_root: &Path,
    project_root: &Path,
    bundle_exclude: &BundleExcludeMatcher,
    claim_mode: RuntimeAliasClaimMode,
) -> Result<bool> {
    let Ok(relative) = path.strip_prefix(workspace_root) else {
        return Ok(false);
    };
    if bundle_exclude.is_excluded(path, project_root)
        || relative.components().any(|component| {
            let Component::Normal(name) = component else {
                return false;
            };
            let name = name.to_string_lossy();
            name.starts_with('.') || MIRROR_SKIP_DIRS.iter().any(|skip| name == *skip)
        })
        || (relative
            .parent()
            .is_some_and(|parent| parent.as_os_str().is_empty())
            && relative
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_reserved_shadow_root_name))
    {
        return Ok(false);
    }

    // Match the wholesale mirror's `.gitignore` posture without enumerating
    // (and therefore accidentally treating as a staging claim) any sibling
    // tree. Nested ignore files are added in root-to-leaf order.
    let mut builder = ignore::gitignore::GitignoreBuilder::new(workspace_root);
    let mut ancestors = path
        .parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .take_while(|ancestor| ancestor.starts_with(workspace_root))
        .collect::<Vec<_>>();
    ancestors.reverse();
    for ancestor in ancestors {
        let ignore_file = ancestor.join(".gitignore");
        if ignore_file.is_file() {
            if let Some(error) = builder.add(ignore_file) {
                return Err(error).context("bundler: parse first-party .gitignore");
            }
        }
    }
    let matcher = builder
        .build()
        .context("bundler: build first-party .gitignore matcher")?;
    let gitignored = matcher.matched_path_or_any_parents(path, false).is_ignore();
    // Issue #1903: a runtime resolver edge identifies one concrete leaf; it
    // does not claim the ignored directory around it. Permit that exact leaf
    // only when it is a regular non-JS/TS file. Source modules remain subject
    // to the wholesale mirror's gitignore posture, while the hidden/infra,
    // reserved-name, bundle.exclude, containment, canonical-path, and
    // workspace-membership checks above/below remain fail-closed.
    Ok(!gitignored
        || (claim_mode == RuntimeAliasClaimMode::ExactNonSourceLeaf
            && path.is_file()
            && !raw_source_extension(path)))
}

fn reject_root_package_relative_tree_escapes(
    files: &BTreeSet<PathBuf>,
    workspace_root: &Path,
) -> Result<()> {
    for importer in files {
        if workspace_package_root_for(importer, workspace_root) != workspace_root {
            continue;
        }
        let Some(importer_tree) = workspace_root_tree(importer, workspace_root) else {
            continue;
        };
        let Ok(specifiers) = collect_runtime_import_specifiers_from_file(importer) else {
            continue;
        };
        for specifier in specifiers {
            let path = specifier
                .split_once(['?', '#'])
                .map(|(path, _)| path)
                .unwrap_or(&specifier);
            if !path.starts_with("../") {
                continue;
            }
            let target =
                normalize_path_lexical(&importer.parent().unwrap_or(workspace_root).join(path));
            if target.starts_with(workspace_root) && !target.starts_with(&importer_tree) {
                bail!(
                    "bundler: parent-escaping relative value import {specifier:?} in {} crosses the staged root-package source-tree boundary {}; use a tsconfig alias spelling instead",
                    importer.display(),
                    importer_tree.display()
                );
            }
        }
    }
    Ok(())
}

fn collect_runtime_alias_claim_graph(
    project_root: &Path,
    first_party_root: &Path,
    tsconfig_paths: &BTreeMap<String, Vec<String>>,
    bundle_exclude: &BundleExcludeMatcher,
    context: &ModuleWorkerBuildContext,
) -> Result<(
    BTreeSet<PathBuf>,
    BTreeSet<RawImportEdge>,
    BTreeSet<PathBuf>,
)> {
    if first_party_root == project_root || tsconfig_paths.is_empty() {
        return Ok((BTreeSet::new(), BTreeSet::new(), BTreeSet::new()));
    }
    let root_package_claimed =
        zfb_types::first_party::workspace_explicitly_claims_root_package(project_root);
    let canonical_root = first_party_root.canonicalize().with_context(|| {
        format!(
            "bundler: canonicalize first-party root {}",
            first_party_root.display()
        )
    })?;
    let mut concrete_entries = BTreeSet::new();
    for entry in WalkDir::new(project_root)
        .follow_links(true)
        .into_iter()
        .filter_entry(|entry| !is_pruned_infra_dir(entry))
        .filter_map(std::result::Result::ok)
    {
        let importer = entry.path();
        if !entry.file_type().is_file()
            || bundle_exclude.is_excluded(importer, project_root)
            || !(raw_source_extension(importer)
                || importer
                    .extension()
                    .is_some_and(|extension| extension == "css"))
        {
            continue;
        }
        let Ok(specifiers) = collect_runtime_import_specifiers_from_file(importer) else {
            continue;
        };
        for specifier in specifiers {
            let alias_specifier = specifier
                .split_once(['?', '#'])
                .map(|(path, _)| path)
                .unwrap_or(&specifier);
            for target in concrete_alias_targets(alias_specifier, tsconfig_paths) {
                let Some(file) = resolve_concrete_alias_file(&target) else {
                    continue;
                };
                if path_is_inside_node_modules(&file)
                    || !file.starts_with(first_party_root)
                    || file.starts_with(project_root)
                {
                    continue;
                }
                if !runtime_alias_path_is_claimable(
                    &file,
                    &canonical_root,
                    first_party_root,
                    project_root,
                    root_package_claimed,
                    bundle_exclude,
                    RuntimeAliasClaimMode::for_runtime_target(&file),
                )? {
                    continue;
                }
                concrete_entries.insert(file);
                break;
            }
        }
    }

    let mut claims = BTreeSet::new();
    let mut raw_import_edges = BTreeSet::new();
    let mut exact_gitignored_leaves = BTreeSet::new();
    let mut pending = concrete_entries;
    while let Some(entry) = pending.pop_first() {
        if entry.starts_with(project_root)
            || !runtime_alias_path_is_claimable(
                &entry,
                &canonical_root,
                first_party_root,
                project_root,
                root_package_claimed,
                bundle_exclude,
                RuntimeAliasClaimMode::for_runtime_target(&entry),
            )?
        {
            continue;
        }
        if !claims.insert(entry.clone()) {
            continue;
        }
        if !raw_source_extension(&entry)
            && !runtime_alias_path_is_claimable(
                &entry,
                &canonical_root,
                first_party_root,
                project_root,
                root_package_claimed,
                bundle_exclude,
                RuntimeAliasClaimMode::Standard,
            )?
        {
            // This path was admitted only by the exact non-source-leaf
            // exception. Materialise it file-by-file, but never let it become
            // a directory-level SiblingMirrorPlan claim.
            exact_gitignored_leaves.insert(entry.clone());
        }
        if raw_source_extension(&entry)
            || entry
                .extension()
                .is_some_and(|extension| extension == "css")
        {
            let discovery = discover_module_preprocessing_with_tsconfig_paths(
                &entry,
                project_root,
                context,
                tsconfig_paths,
            )
            .with_context(|| {
                format!(
                    "bundler: discover runtime alias claim graph from {}",
                    entry.display()
                )
            })?;
            for file in discovery.files {
                if !runtime_alias_path_is_claimable(
                    &file,
                    &canonical_root,
                    first_party_root,
                    project_root,
                    root_package_claimed,
                    bundle_exclude,
                    RuntimeAliasClaimMode::for_runtime_target(&file),
                )? {
                    continue;
                }
                if !claims.contains(&file) {
                    pending.insert(file);
                }
            }
            for edge in discovery.worker_edges {
                if !runtime_alias_path_is_claimable(
                    &edge.importer,
                    &canonical_root,
                    first_party_root,
                    project_root,
                    root_package_claimed,
                    bundle_exclude,
                    RuntimeAliasClaimMode::Standard,
                )? || !runtime_alias_path_is_claimable(
                    &edge.source_path,
                    &canonical_root,
                    first_party_root,
                    project_root,
                    root_package_claimed,
                    bundle_exclude,
                    RuntimeAliasClaimMode::Standard,
                )? {
                    bail!(
                        "bundler: runtime alias module worker from {} reached unclaimed or excluded source {}",
                        edge.importer.display(),
                        edge.source_path.display()
                    );
                }
            }
            for edge in discovery.raw_import_edges {
                if !runtime_alias_path_is_claimable(
                    &edge.importer,
                    &canonical_root,
                    first_party_root,
                    project_root,
                    root_package_claimed,
                    bundle_exclude,
                    RuntimeAliasClaimMode::Standard,
                )? || !runtime_alias_path_is_claimable(
                    &edge.target,
                    &canonical_root,
                    first_party_root,
                    project_root,
                    root_package_claimed,
                    bundle_exclude,
                    RuntimeAliasClaimMode::for_runtime_target(&edge.target),
                )? {
                    bail!(
                        "bundler: runtime alias raw import from {} reached unclaimed or excluded target {}",
                        edge.importer.display(),
                        edge.target.display()
                    );
                }
                raw_import_edges.insert(RawImportEdge {
                    importer: edge.importer,
                    target: edge.target,
                });
            }
        }
        let Ok(specifiers) = collect_runtime_import_specifiers_from_file(&entry) else {
            continue;
        };
        for specifier in specifiers {
            let alias_specifier = specifier
                .split_once(['?', '#'])
                .map(|(path, _)| path)
                .unwrap_or(&specifier);
            for target in concrete_alias_targets(alias_specifier, tsconfig_paths) {
                let Some(file) = resolve_concrete_alias_file(&target) else {
                    continue;
                };
                if path_is_inside_node_modules(&file)
                    || !file.starts_with(first_party_root)
                    || file.starts_with(project_root)
                {
                    continue;
                }
                if !runtime_alias_path_is_claimable(
                    &file,
                    &canonical_root,
                    first_party_root,
                    project_root,
                    root_package_claimed,
                    bundle_exclude,
                    RuntimeAliasClaimMode::for_runtime_target(&file),
                )? {
                    continue;
                }
                if claims.contains(&file) {
                    continue;
                }
                pending.insert(file);
                break;
            }
        }
    }
    reject_root_package_relative_tree_escapes(&claims, first_party_root)?;
    Ok((claims, raw_import_edges, exact_gitignored_leaves))
}

/// Resolve a claim path to its **mirror root** per issue #1691's claim policy:
/// the nearest ancestor directory that directly contains a `package.json`,
/// else the claim's own directory. Returns `None` for anything that is not a
/// wholesale-mirrorable workspace-sibling region:
///
/// - a standalone project (`first_party_root == project_root` — no siblings),
/// - a claim outside `first_party_root`, or under (i.e. local to)
///   `project_root` — those are the project mirror's own concern,
/// - a claim reached through a `node_modules` component (vendored, not source),
/// - a hidden or infrastructure directory as the mirror root (the ignore
///   walker deliberately does not filter its own root),
/// - `first_party_root` itself (never mirrored wholesale), and
/// - a region that would CONTAIN `project_root` (an ancestor-of-project dir):
///   mirroring it would double-stage the project subtree over its own mirror.
///
/// The ancestor search never crosses into `first_party_root` or a
/// project-containing directory, so the returned root is always a disjoint
/// sibling region.
fn resolve_mirror_root(
    claim: &Path,
    project_root: &Path,
    first_party_root: &Path,
) -> Option<PathBuf> {
    if first_party_root == project_root {
        return None;
    }
    let claim = normalize_path_lexical(claim);
    if path_is_inside_node_modules(&claim) {
        return None;
    }
    if !claim.starts_with(first_party_root) || claim.starts_with(project_root) {
        return None;
    }
    let claim_dir = if claim.is_dir() {
        claim.clone()
    } else {
        claim.parent()?.to_path_buf()
    };
    if claim_dir == first_party_root || !claim_dir.starts_with(first_party_root) {
        return None;
    }
    // A claim whose own directory is an ancestor of the project is not a
    // disjoint sibling region (mirroring it would swallow the project mirror).
    if project_root.starts_with(&claim_dir) {
        return None;
    }
    // Nearest ancestor (from the claim dir up to, but excluding,
    // `first_party_root`) that directly contains a `package.json`.
    let mut dir: &Path = claim_dir.as_path();
    loop {
        if dir == first_party_root || project_root.starts_with(dir) {
            break;
        }
        if dir.join("package.json").is_file() {
            return mirror_root_is_stageable(dir).then(|| dir.to_path_buf());
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }
    mirror_root_is_stageable(&claim_dir).then_some(claim_dir)
}

fn mirror_root_is_stageable(root: &Path) -> bool {
    root.file_name().is_some_and(|name| {
        let name = name.to_string_lossy();
        !name.starts_with('.') && !MIRROR_SKIP_DIRS.iter().any(|skip| name == *skip)
    })
}

/// The claim plan (issue #1691/#1692): the set of distinct workspace-sibling
/// **mirror roots** to wholesale-stage into the SSR work mirror, computed ONCE
/// per [`bundle`] from the three claim sources. This is the single source of
/// truth for sibling staging and — via [`Self::claims_path`] — for the CSS
/// discovery and audit predicates the Wave 2/3 sub-issues (#1695–#1697)
/// consume. Empty (and inert) for a standalone project, so a non-workspace
/// build stays byte-identical.
#[derive(Debug, Clone, Default)]
pub struct SiblingMirrorPlan {
    /// Distinct mirror roots, each an absolute, lexically-normalized directory
    /// under `first_party_root`, outside `project_root`, never through a
    /// `node_modules` component and never `first_party_root` itself.
    mirror_roots: BTreeSet<PathBuf>,
}

impl SiblingMirrorPlan {
    /// Compute the plan from the three claim sources of issue #1691:
    ///
    /// - **(a)** every preprocessing-discovery-graph file under
    ///   `first_party_root` outside `project_root` (`discovered_graph_files`
    ///   is the bundler's `plugin_preprocessing_files` set),
    /// - **(c)** every registered virtual module's absolute import there — its
    ///   discovered graph is folded into the same `discovered_graph_files`
    ///   set (the virtual-module discovery feeds `plugin_preprocessing_files`),
    /// - **(b)** every tsconfig / plugin alias target there (wildcard target →
    ///   its directory component, via [`alias_target_claim_path`]).
    ///
    /// Each claim resolves through [`resolve_mirror_root`]; `None` results are
    /// dropped (project-local, out-of-workspace, node_modules, etc.).
    pub fn compute(
        project_root: &Path,
        first_party_root: &Path,
        discovered_graph_files: &BTreeSet<PathBuf>,
        tsconfig_paths: &BTreeMap<String, Vec<String>>,
        plugin_alias_entries: &[(String, String)],
    ) -> Self {
        let mut mirror_roots = BTreeSet::new();
        if first_party_root == project_root {
            return Self { mirror_roots };
        }
        // (a) + (c): discovered graph files (project and sibling); only the
        // siblings survive `resolve_mirror_root`.
        for file in discovered_graph_files {
            if let Some(root) = resolve_mirror_root(file, project_root, first_party_root) {
                mirror_roots.insert(root);
            }
        }
        // (b): alias targets — wildcard targets contribute their directory.
        for target in tsconfig_paths.values().flatten().map(String::as_str).chain(
            plugin_alias_entries
                .iter()
                .map(|(_, target)| target.as_str()),
        ) {
            let claim = alias_target_claim_path(target);
            if let Some(root) = resolve_mirror_root(&claim, project_root, first_party_root) {
                mirror_roots.insert(root);
            }
        }
        Self { mirror_roots }
    }

    /// The distinct mirror roots to wholesale-stage, in deterministic order.
    pub fn mirror_roots(&self) -> impl Iterator<Item = &Path> {
        self.mirror_roots.iter().map(PathBuf::as_path)
    }

    /// Whether the plan claims nothing (a standalone project, or a workspace
    /// whose build reaches no sibling region).
    pub fn is_empty(&self) -> bool {
        self.mirror_roots.is_empty()
    }

    /// Audit predicate for Wave 2/3: is `path` inside a claimed mirror root?
    /// The CSS discovery (#1696/#1697) and any resurrection audit key off
    /// this so they enumerate exactly the regions this plan staged — no
    /// second, drift-prone definition of "a claimed sibling path". NOT
    /// consumed by the issue #1695 glob fixed-point queue: an earlier cut of
    /// that queue used this to skip a match already inside a claimed
    /// region, on the assumption [`mirror_sibling_root`]'s wholesale copy
    /// already made it correct — false for a claimed-region file that
    /// itself uses `import.meta.glob`/`?raw`/a worker URL, since that copy
    /// is a raw byte copy with no macro expansion. The queue now stages
    /// every match unconditionally instead.
    pub fn claims_path(&self, path: &Path) -> bool {
        let path = normalize_path_lexical(path);
        self.mirror_roots.iter().any(|root| path.starts_with(root))
    }
}

/// Wholesale-mirror one claimed sibling `mirror_root` into the SSR work mirror
/// (issue #1692) — the sibling analogue of the project's exact-target dir
/// staging ([`materialise_isolated_exact_dir`]): a RAW file copy of the whole
/// region so esbuild (the sole resolver) can resolve anything inside it —
/// glob directory probing, index files, `package.json` `main`/`imports`,
/// wildcard-alias targets — that the Rust preprocessing discovery could not
/// predict. Source files that DO need `?raw`/glob/worker rewrites are
/// overwritten afterwards by the plugin-preprocessing pass: since issue #1724
/// EVERY mirrored source file is enrolled in `plugin_preprocessing_files`
/// (see the enrolment loop in [`bundle_with_session`]), because a claimed
/// sibling region's macro-bearing file frequently reaches no other claim
/// source and would otherwise ship its literal macro text. Every file lands at its
/// workspace-relative slot via [`shadow_path_for_project_path`]
/// (`work.join(<rel>)`), so it never collides with the nested project mirror.
#[allow(clippy::too_many_arguments)]
fn mirror_sibling_root(
    mirror_root: &Path,
    project_root: &Path,
    first_party_root: &Path,
    shadow: &Path,
    work: &Path,
    node_modules_isolation_root: Option<&Path>,
    writer: &ShadowWriter<'_>,
    bundle_exclude: &BundleExcludeMatcher,
) -> Result<()> {
    for src in enumerate_mirror_root_files(mirror_root) {
        // Defense in depth vs. the walk's own pruning: never mirror through a
        // node_modules component, out of the first-party region, or over the
        // project mirror.
        if path_is_inside_node_modules(&src)
            || !src.starts_with(first_party_root)
            || src.starts_with(project_root)
        {
            continue;
        }
        // The two-tier (issue #1668 part 2) `bundle.exclude` contract still
        // governs a sibling file: an excluded sibling is absent from the
        // shadow, exactly like an excluded project file.
        if bundle_exclude.is_excluded(&src, project_root) {
            continue;
        }
        let dest = shadow_path_for_project_path(
            &src,
            project_root,
            first_party_root,
            shadow,
            work,
            node_modules_isolation_root,
        );
        // Incremental mirror skip (issue #1777). Session mode ONLY, mirroring
        // `materialise_source_file`'s `source_skip` fast path — but stored in
        // the SEPARATE `mirror_skip` namespace (see
        // [`ShadowSession::mirror_skip`]) so this can never satisfy a later
        // same-tick `materialise_source_file` lookup for the same dest. A
        // mirror copy is a pure function of the source's own bytes (no
        // glob/`?raw`/worker expansion ever runs here), so an unchanged
        // `(mtime, size)` with the dest still present is an exact skip.
        let dest_rel = writer.rel_of(&dest).ok();
        if writer.in_session() {
            if let Some(dest_rel) = dest_rel.as_ref() {
                if let Some((mtime, size)) = file_stat(&src) {
                    if let Some(entry) = writer.mirror_skip_get(dest_rel) {
                        let dest_exists = fs::symlink_metadata(&dest).is_ok();
                        if entry.source == src
                            && entry.mtime == mtime
                            && entry.size == size
                            && dest_exists
                        {
                            writer.record_visited(&dest).with_context(|| {
                                format!("record visited (mirror skip) {}", dest.display())
                            })?;
                            continue;
                        }
                    }
                }
            }
        }
        if let Some(parent) = dest.parent() {
            writer.ensure_dir(parent).with_context(|| {
                format!("bundler: create sibling mirror dir {}", parent.display())
            })?;
        }
        writer.copy_if_changed(&src, &dest).with_context(|| {
            format!(
                "bundler: wholesale-mirror sibling file {} -> {}",
                src.display(),
                dest.display()
            )
        })?;
        if let Some(dest_rel) = dest_rel {
            match file_stat(&src) {
                Some((mtime, size)) => writer.mirror_skip_store(
                    dest_rel,
                    MirrorSkipEntry {
                        source: src.clone(),
                        mtime,
                        size,
                    },
                ),
                // Could not stat the source after copying it — no sound
                // skip key (mirrors `store_source_skip_entry`'s posture for
                // the analogous case).
                None => writer.mirror_skip_remove(&dest_rel),
            }
        }
    }
    Ok(())
}

/// Drain [`MaterialiseCtx::glob_matched_files`] to a fixed point (issue
/// #1695): every `import.meta.glob(...)` target discovered anywhere in this
/// `bundle_with_session` call is staged at its own
/// [`shadow_path_for_project_path`] slot through
/// [`materialise_source_file`] — the same pass every other first-party file
/// goes through — plus, since that only expands glob/`?raw`/worker macros
/// and never walks plain `import` specifiers, the target's own ordinary
/// first-party dependency closure (via [`discover_module_preprocessing_with_context`],
/// the same discovery [`bundle_with_session`]'s `exact_target_staging_files`
/// loop uses).
///
/// `import.meta.glob(...)` targets are invisible to the AST-based
/// preprocessing-discovery graph [`SiblingMirrorPlan`] is built from (the
/// discovery walker resolves `import`/`require`/`?raw`/worker specifiers,
/// never a glob macro), so a file reached ONLY through a glob has no claim.
///
/// EVERY match is staged here, deliberately including ones already inside a
/// claimed [`SiblingMirrorPlan`] mirror root — a codex-review finding on the
/// first cut of this queue (which skipped those via `claims_path`) caught
/// that `mirror_sibling_root`'s wholesale copy is a RAW byte copy with no
/// macro expansion, so a claimed-region file that itself uses
/// `import.meta.glob`/`?raw`/a worker URL was left with the literal,
/// unexpanded macro reaching esbuild. Restaging is cheap for the common
/// plain-file case (see below) and correctness-required for that case, so
/// there is no gate left to apply here.
///
/// A queued file's OWN nested glob/raw/worker edges (from
/// `materialise_source_file`) and its own plain-import closure (from the
/// discovery pass) both extend `glob_matched_files` further, so this
/// function loops again; it returns once a full pass adds nothing new. The
/// `BTreeSet` backing `glob_matched_files` keeps each round's processing
/// order deterministic regardless of which materialise pass — or which
/// discovery call — found which file first.
///
/// A queued match the ordinary walks (or `mirror_sibling_root`, or the
/// `plugin_preprocessing_files` loop) already covered is staged again here,
/// but harmlessly: `materialise_source_file` recomputes identical bytes, and
/// both the session incremental-skip cache and the passthrough writer's
/// write-if-changed hashing make the repeat a no-op on disk. Only a target
/// genuinely unreachable any other way — a brand-new sibling region no claim
/// source ever saw, or a project path the ordinary walks did not reach (e.g.
/// gitignored) — depends on this queue for correctness.
///
/// `.mdx` matches are skipped outright: they need the MDX compile pipeline
/// (`materialise_mdx_with_skip`, driven by `materialise_shadow`'s own walk),
/// not `materialise_source_file`'s plain JS/TS transform. A project `.mdx`
/// file the ordinary walk already compiled would have that CORRECT compiled
/// output clobbered by a raw copy/symlink of the uncompiled source if staged
/// here — worse than doing nothing. A glob-matched `.mdx` file no other pass
/// reaches stays an unsupported form for now (mirroring Wave 1's
/// "unsupported form is an explicit failure, not a silent mis-expansion"
/// posture for `import.meta.glob`): esbuild's JSX loader reports a clear
/// parse error naming the uncompiled file rather than the queue silently
/// corrupting an already-correct compile.
///
/// Pruning a match that disappears on a later `ShadowSession` tick needs no
/// bespoke bookkeeping: `mat_ctx.glob_matched_files` starts empty every
/// `bundle_with_session` call (a fresh [`MaterialiseCtx`]), so a file the
/// live glob no longer matches is simply never re-queued, never re-visited
/// this tick, and falls out of `ShadowSession::prev_visited` at the
/// `writer.prune_stale()` call the caller makes after this returns — the
/// same tick-boundary mechanism every other staged file already relies on.
fn stage_glob_matched_files_to_fixed_point(
    mat_ctx: &MaterialiseCtx<'_, '_>,
    project_root: &Path,
    first_party_root: &Path,
    shadow: &Path,
    work: &Path,
    node_modules_isolation_root: Option<&Path>,
) -> Result<()> {
    let mut staged_glob_targets: BTreeSet<PathBuf> = BTreeSet::new();
    loop {
        let pending: Vec<PathBuf> = mat_ctx
            .glob_matched_files
            .borrow()
            .iter()
            .filter(|path| !staged_glob_targets.contains(*path))
            .cloned()
            .collect();
        if pending.is_empty() {
            break;
        }
        for target in pending {
            staged_glob_targets.insert(target.clone());
            // Defense in depth, mirroring `mirror_sibling_root`'s guard: a
            // glob match must be a real first-party file under the widened
            // first-party root, never through `node_modules` (vendored, not
            // source — resolves through the live `node_modules` link) and
            // never anything `shadow_path_for_project_path` would leave
            // pointed at the LIVE tree (its fallback tier for a path outside
            // both `project_root` and `first_party_root`, which must never
            // happen for a glob match but is never worth writing through).
            if !target.is_file()
                || path_is_inside_node_modules(&target)
                || !target.starts_with(first_party_root)
            {
                continue;
            }
            if mat_ctx.bundle_exclude.is_excluded(&target, project_root) {
                continue;
            }
            // See the function doc: `.mdx` needs the MDX pipeline, and
            // staging it here would clobber an already-correct compile.
            if target.extension().and_then(|ext| ext.to_str()) == Some("mdx") {
                continue;
            }
            let to = shadow_path_for_project_path(
                &target,
                project_root,
                first_party_root,
                shadow,
                work,
                node_modules_isolation_root,
            );
            if let Some(parent) = to.parent() {
                mat_ctx.writer.ensure_dir(parent).with_context(|| {
                    format!(
                        "bundler: create import.meta.glob matched-file parent dir {}",
                        parent.display()
                    )
                })?;
            }
            materialise_source_file(
                &target,
                &target,
                &to,
                &|path| {
                    mat_ctx
                        .bundle_exclude
                        .is_excluded(path, mat_ctx.project_root)
                },
                mat_ctx.copy_mode,
                mat_ctx.writer,
                &mat_ctx.glob_matched_files,
                &mat_ctx.raw_import_edges,
                &mat_ctx.raw_import_aliases,
                &mat_ctx.module_worker_dependencies,
                mat_ctx.project_root,
                &mat_ctx.worker_build_context,
            )
            .with_context(|| {
                format!(
                    "bundler: materialise import.meta.glob matched file {}",
                    target.display()
                )
            })?;

            // codex-review P2: `materialise_source_file` only expands
            // glob/`?raw`/worker macros — it never walks plain `import`
            // specifiers, so a matched file's ordinary first-party
            // dependency closure would otherwise never enter the shadow
            // tree, and esbuild would fail to resolve it. Best-effort: a
            // discovery failure here (e.g. a parse error in an unrelated
            // branch of the target's graph) degrades to "stage the matched
            // file alone" rather than failing the whole build — the same
            // "rebuild more, not less" bias `zfb-build`'s policy favors
            // elsewhere; a genuinely broken import inside the target itself
            // still surfaces as a named esbuild resolve error.
            if let Ok(discovery) = discover_module_preprocessing_with_context(
                &target,
                project_root,
                &mat_ctx.worker_build_context,
            ) {
                mat_ctx.raw_import_edges.borrow_mut().extend(
                    discovery
                        .raw_import_edges
                        .into_iter()
                        .map(|edge| RawImportEdge {
                            importer: edge.importer,
                            target: edge.target,
                        }),
                );
                mat_ctx
                    .glob_matched_files
                    .borrow_mut()
                    .extend(discovery.files);
            }
        }
    }
    Ok(())
}

/// Whether `name` is a reserved GENERATED file the bundler writes into the
/// shadow root — the project-root loose-file staging pass (issue #1692) must
/// never overwrite one of these with a user file of the same name (the
/// generated file wins; the user file is skipped with a debug note).
fn is_reserved_shadow_root_name(name: &str) -> bool {
    name == SHADOW_TSCONFIG_FILENAME
        || name == SHADOW_ENTRY_FILENAME
        || name == SHADOW_HYDRATE_FILENAME
        || name == ".zfb-metafile.json"
        || name.starts_with(".zfb-virtual-")
}

/// Stage the project-root LOOSE FILES into the shadow root (issue #1692) — the
/// generalization of [`materialise_mdx_components_file`], which stages exactly
/// one fixed-name root file. A wildcard root alias (`"@/*": ["./*"]`, whose
/// target directory is the project root) lets esbuild resolve `@/<name>` to a
/// file sitting directly in the project root; with `bundle.exclude` active the
/// tsconfig is rewritten shadow-only (no live-tree fallback — see
/// [`rebase_tsconfig_paths_to_shadow_with_exclusions`]), so those loose files
/// must be present in the shadow or the import fails with `Could not resolve`
/// (the real-site #1685 breakage). Direct child DIRECTORIES are already staged
/// by the extra-dirs pass; only depth-1 files are handled here.
///
/// Reserved generated names ([`is_reserved_shadow_root_name`]) are SKIPPED so
/// the generated `tsconfig.json` / `entry.mjs` / hydrate shim / `.zfb-*`
/// infra files always win. `mdx-components.tsx` keeps its own dedicated pass.
/// Hidden dotfiles are skipped (never resolved by a bare `@/<name>` import).
fn stage_project_root_loose_files(
    project_root: &Path,
    shadow: &Path,
    ctx: &MaterialiseCtx<'_, '_>,
) -> Result<()> {
    let entries = match fs::read_dir(project_root) {
        Ok(entries) => entries,
        // A missing/unreadable project root is surfaced by the earlier passes;
        // stay defensive here (the same "rebuild more, not less" bias).
        Err(_) => return Ok(()),
    };
    // Collect the eligible loose files first (deterministic order), then
    // preflight-then-materialise: the global preflight pass never visited the
    // project ROOT's own files (only pages/content/components/layouts/extra),
    // so a `?raw` edge between two root files must be established here before
    // either is materialised — otherwise `read_dir` order could materialise a
    // `.ts` raw PAYLOAD as source (expanding its own macros / rejecting it)
    // before the importer's terminal edge marks it a raw target.
    let mut loose_files: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|ft| ft.is_file()) {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == MDX_COMPONENTS_FILENAME {
            continue; // its own dedicated pass (returns the entry import spec)
        }
        if is_reserved_shadow_root_name(&name) {
            tracing::debug!(
                file = %name,
                "bundler: skipping project-root loose file — reserved generated shadow name"
            );
            continue;
        }
        if name.starts_with('.') {
            continue;
        }
        let src = entry.path();
        if ctx.bundle_exclude.is_excluded(&src, ctx.project_root) {
            continue;
        }
        loose_files.push(src);
    }
    loose_files.sort();

    // Preflight pass — establish every terminal raw-target identity among the
    // root files before any of them is materialised (mirrors the tree-level
    // `preflight_raw_tree` → `materialise_shadow` ordering).
    for src in &loose_files {
        if raw_source_extension(src) {
            preflight_raw_file(src, src, ctx);
        }
    }

    for src in &loose_files {
        let dest = shadow.join(src.file_name().unwrap_or_default());
        if raw_source_extension(src) {
            // A source file reached via `@/<name>` may itself carry `?raw` /
            // glob / worker syntax — run it through the same preprocessing the
            // project source walks apply, so its rewrites land in the shadow.
            materialise_source_file(
                src,
                src,
                &dest,
                &|path| ctx.bundle_exclude.is_excluded(path, ctx.project_root),
                ctx.copy_mode,
                ctx.writer,
                &ctx.glob_matched_files,
                &ctx.raw_import_edges,
                &ctx.raw_import_aliases,
                &ctx.module_worker_dependencies,
                ctx.project_root,
                &ctx.worker_build_context,
            )
            .with_context(|| {
                format!(
                    "bundler: stage project-root loose source file {} -> {}",
                    src.display(),
                    dest.display()
                )
            })?;
        } else {
            ctx.writer.copy_if_changed(src, &dest).with_context(|| {
                format!(
                    "bundler: stage project-root loose file {} -> {}",
                    src.display(),
                    dest.display()
                )
            })?;
        }
    }
    Ok(())
}

/// Whether any tsconfig / plugin wildcard alias target resolves to the project
/// root itself (`"@/*": ["./*"]` and friends) — the precondition for the
/// project-root loose-file staging pass. Concrete (non-wildcard) root aliases
/// are staged file-by-file by [`plan_concrete_target_staging`]; only a
/// wildcard can resolve to an unpredictable set of root files.
fn has_wildcard_root_alias(
    project_root: &Path,
    tsconfig_paths: &BTreeMap<String, Vec<String>>,
    plugin_alias_entries: &[(String, String)],
) -> bool {
    tsconfig_paths
        .values()
        .flatten()
        .map(String::as_str)
        .chain(
            plugin_alias_entries
                .iter()
                .map(|(_, target)| target.as_str()),
        )
        .any(|target| target.contains('*') && alias_target_claim_path(target) == project_root)
}

/// Materialise the project-root `mdx-components.tsx` (sub-issue #616) into
/// the shadow root.
///
/// The file is a top-level **file**, not a directory, so none of the
/// existing materialise passes pick it up: `pages`/`content`/`components`/
/// `layouts` are explicit directories, and [`enumerate_extra_top_level_dirs`]
/// skips non-directories (`if !ft.is_dir() { continue; }`). This step copies
/// it explicitly so esbuild sees an in-shadow importer.
///
/// **Copy, not symlink** — production builds run esbuild with
/// `node_modules_preserve_symlinks = false`, which canonicalises a symlink
/// back to its real path; a symlinked override file would then resolve its
/// `./components/X` imports against the *real* project root instead of the
/// shadow tree, defeating the purpose. A plain copy keeps the importer
/// physically inside the shadow so its relative imports and the synthetic
/// `tsconfig.json#paths` apply. (Mirrors the "extra dirs" pass, which
/// materialises source; only `node_modules` is symlinked.)
///
/// Returns the shadow-relative import specifier (`./mdx-components.tsx`) that
/// the synthetic entry imports, or `None` when the source itself is excluded.
fn materialise_mdx_components_file(
    src: &Path,
    shadow: &Path,
    ctx: &MaterialiseCtx<'_, '_>,
) -> Result<Option<String>> {
    if ctx.bundle_exclude.is_excluded(src, ctx.project_root) {
        return Ok(None);
    }
    let dst = shadow.join(MDX_COMPONENTS_FILENAME);
    // Routed through the #993 writer (NOT an always-write infra file):
    // the override file is user-deletable, so it must take part in the
    // visited/prune bookkeeping — a deleted mdx-components.tsx must
    // vanish from the persistent shadow, else an explicit user import of
    // it would resolve in dev but fail in a fresh build.
    materialise_source_file(
        src,
        src,
        &dst,
        &|path| ctx.bundle_exclude.is_excluded(path, ctx.project_root),
        true,
        ctx.writer,
        &ctx.glob_matched_files,
        &ctx.raw_import_edges,
        &ctx.raw_import_aliases,
        &ctx.module_worker_dependencies,
        ctx.project_root,
        &ctx.worker_build_context,
    )
    .with_context(|| {
        format!(
            "bundler: failed preprocessing mdx-components file {} → {}",
            src.display(),
            dst.display()
        )
    })?;
    Ok(Some(format!("./{MDX_COMPONENTS_FILENAME}")))
}

/// Symlink `from` at `to`, falling back to `fs::copy` on platforms that
/// do not support file symlinks or when the necessary privilege is absent.
///
/// Any pre-existing entry at `to` is removed first so we never attempt to
/// create a symlink over an existing path. The removal error is ignored
/// (the entry may not exist yet on the first materialise pass). This is
/// forward-compatible with a future persistent-shadow refactor where the
/// shadow tree is reused across builds.
///
/// - **Unix**: uses [`std::os::unix::fs::symlink`].
/// - **Windows**: tries [`std::os::windows::fs::symlink_file`]; falls back
///   to `fs::copy` when the call fails (Developer Mode off / missing
///   `SeCreateSymbolicLinkPrivilege` — this matches today's behaviour and
///   incurs no regression on unprivileged Windows contexts).
/// - **Other platforms**: unconditional `fs::copy`.
fn symlink_or_copy(from: &Path, to: &Path) -> std::io::Result<()> {
    // Remove any pre-existing entry at `to` so we never try to symlink
    // over an existing file. Forward-compatible with a future
    // persistent-shadow refactor (out of scope for this epic).
    let _ = fs::remove_file(to);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(from, to)
    }
    #[cfg(windows)]
    {
        // File symlinks on Windows require admin OR Developer Mode OR
        // SeCreateSymbolicLinkPrivilege. Fall back to fs::copy when the
        // privilege is missing so the bundler keeps working in
        // unprivileged contexts — perf parity with today's behaviour.
        match std::os::windows::fs::symlink_file(from, to) {
            Ok(()) => Ok(()),
            Err(_) => fs::copy(from, to).map(|_| ()),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        fs::copy(from, to).map(|_| ())
    }
}

/// Return the logical source identity for a file in a materialise walk.
/// Most inputs already live below `project_root` and retain `from` exactly.
/// A production package-route overlay is the exception: copied user pages
/// physically live in a tempdir, but their relative imports still belong to
/// `project_root/pages/<rel>`. Synthesised package routes have no matching
/// project file and deliberately keep their physical identity.
fn logical_importer_for_walk(src: &Path, dest: &Path, from: &Path, project_root: &Path) -> PathBuf {
    let lexical_from = normalize_path_lexical(from);
    let lexical_root = normalize_path_lexical(project_root);
    if lexical_from.starts_with(&lexical_root) {
        return from.to_path_buf();
    }
    if dest.file_name().is_some_and(|name| name == "pages") {
        if let Ok(rel) = from.strip_prefix(src) {
            let logical = project_root.join("pages").join(rel);
            if logical.is_file() {
                return logical;
            }
        }
    }
    from.to_path_buf()
}

fn raw_source_extension(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|s| s.to_str()),
        // page-extension-drift-guard: allow — the JS-family SOURCE extensions
        // a `?raw` import may name (incl. mjs/cjs/mts/cts), not the routable
        // page allowlist.
        Some("ts")
            | Some("tsx")
            | Some("js")
            | Some("jsx")
            | Some("mjs")
            | Some("cjs")
            | Some("mts")
            | Some("cts")
    )
}

fn path_aliases(path: &Path) -> impl Iterator<Item = PathBuf> {
    let lexical = normalize_path_lexical(path);
    let canonical = path.canonicalize().ok();
    std::iter::once(lexical).chain(canonical)
}

fn raw_target_matches(edges: &BTreeSet<RawImportEdge>, physical: &Path, logical: &Path) -> bool {
    let identities: BTreeSet<PathBuf> = path_aliases(physical)
        .chain(path_aliases(logical))
        .collect();
    edges
        .iter()
        .any(|edge| path_aliases(&edge.target).any(|target| identities.contains(&target)))
}

/// Best-effort first pass used only to establish terminal target identity
/// before the broad SSR mirror visits those files. Errors are intentionally
/// deferred to the real materialise pass: an unused invalid JS file must not
/// become a build error merely because it contains query-looking text.
fn preflight_raw_file(physical: &Path, logical: &Path, ctx: &MaterialiseCtx<'_, '_>) {
    if !raw_source_extension(logical) {
        return;
    }
    let Ok(source) = fs::read_to_string(physical) else {
        return;
    };
    if !source.contains("?raw") {
        return;
    }
    if let Ok(expansion) = expand_raw_imports_with_aliases(
        &source,
        logical,
        ctx.project_root,
        &ctx.raw_import_aliases,
        &|_| false,
    ) {
        ctx.raw_import_edges.borrow_mut().extend(expansion.edges);
    }
}

/// Classify a `walkdir::Error` from a `follow_links(true)` (or
/// `follow_links(copy_mode)` with `copy_mode` true) walk as a SKIPPABLE
/// dangling symlink rather than a genuine read failure: the link itself
/// exists (`symlink_metadata` succeeds and reports a symlink) but stat-ing
/// through it failed with `NotFound` because its target is gone. Returns
/// the link path and its target so callers can warn-and-continue instead
/// of aborting the whole build on one stale link (e.g. a gitignored
/// fixture dir with a link whose target was deleted — issue #1838, where
/// this previously surfaced as a bare `os error 2` with no mention of a
/// symlink). Any other error (permission denied, a non-symlink `NotFound`,
/// etc.) returns `None` so the caller keeps propagating it as fatal.
///
/// The `fs::metadata(path).is_ok()` re-check guards a TOCTOU window: the
/// walk's own `NotFound` only proves the target was unreachable AT THAT
/// INSTANT. If a fresh stat right now says the path resolves fine after
/// all, this was some other transient condition (a race, a flaky
/// filesystem) rather than a genuinely dangling link — treat it as fatal
/// (return `None`) rather than silently skipping a subtree that might
/// still hold real, needed files.
fn dangling_symlink_from_walk_error(err: &walkdir::Error) -> Option<(PathBuf, PathBuf)> {
    if err.io_error().map(|e| e.kind()) != Some(std::io::ErrorKind::NotFound) {
        return None;
    }
    let path = err.path()?;
    if fs::metadata(path).is_ok() {
        return None;
    }
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_symlink() {
        return None;
    }
    let target = fs::read_link(path).ok()?;
    Some((path.to_path_buf(), target))
}

/// Shared per-entry handling for the three `follow_links(true)`-shaped
/// `WalkDir` loops below: on a classified dangling symlink (see
/// [`dangling_symlink_from_walk_error`]), warn loudly through BOTH channels
/// — `tracing::warn!` (so tests can assert on it via `logs_contain`) and
/// `eprintln!` (the channel real `zfb` CLI users actually see: no
/// `tracing_subscriber` is installed anywhere in the `zfb` binary, so a
/// bare `tracing::warn!` alone is a silent no-op there) — and return
/// `Ok(None)` so the caller skips the entry. Any other error is wrapped
/// with `context` and returned as `Err`, matching each walk's original
/// fatal behavior.
///
/// `read_link` only resolves the IMMEDIATE target, so a multi-hop chain
/// (`link1 -> link2 -> missing`) is reported as "cannot be resolved" rather
/// than a (possibly inaccurate) claim that the immediate target itself is
/// missing.
fn skip_dangling_symlink_or_fail<T>(
    err: walkdir::Error,
    context: impl FnOnce() -> String,
) -> Result<Option<T>> {
    if let Some((link, target)) = dangling_symlink_from_walk_error(&err) {
        let msg = dangling_symlink_warning_message(&link, &target);
        tracing::warn!("{msg}");
        eprintln!("zfb warn: {msg}");
        return Ok(None);
    }
    Err(err).with_context(context)
}

/// Text of the dangling-symlink warning shared by both emission channels in
/// [`skip_dangling_symlink_or_fail`] (`tracing::warn!`, captured by tests via
/// `logs_contain`, and the `eprintln!("zfb warn: {msg}")` channel real `zfb`
/// CLI users actually see). Factored out as a pure function so a unit test
/// can pin exactly what reaches the user-visible `eprintln!` form without
/// needing to capture the process's real stderr file descriptor.
fn dangling_symlink_warning_message(link: &Path, target: &Path) -> String {
    format!(
        "dangling symlink: {} -> {} (cannot be resolved); skipping",
        link.display(),
        target.display()
    )
}

fn preflight_raw_tree(src: &Path, dest: &Path, ctx: &MaterialiseCtx<'_, '_>) -> Result<()> {
    if !src.exists() {
        return Ok(());
    }
    for entry in WalkDir::new(src)
        .follow_links(ctx.copy_mode)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| !is_pruned_infra_dir(entry))
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                match skip_dangling_symlink_or_fail(err, || {
                    format!("preflight raw imports under {}", src.display())
                })? {
                    Some(entry) => entry,
                    None => continue,
                }
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let physical = entry.path();
        let logical = if physical.starts_with(src) {
            logical_importer_for_walk(src, dest, physical, ctx.project_root)
        } else {
            physical.to_path_buf()
        };
        preflight_raw_file(physical, &logical, ctx);
    }
    Ok(())
}

/// Materialise a symlinked source directory as real files in copy mode while
/// still applying the JS-side glob/raw transforms to children. `physical_root`
/// is canonical for safe traversal; `logical_root` preserves the project-local
/// symlink spelling used by relative imports and invalidation.
fn materialise_symlinked_dir(
    logical_root: &Path,
    dest: &Path,
    ctx: &MaterialiseCtx<'_, '_>,
    is_excluded: &dyn Fn(&Path) -> bool,
) -> Result<()> {
    let physical_root = logical_root.canonicalize().with_context(|| {
        format!(
            "canonicalize symlinked source dir {}",
            logical_root.display()
        )
    })?;
    ctx.writer.ensure_dir(dest)?;
    for entry in WalkDir::new(&physical_root)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| !is_pruned_infra_dir(e))
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                match skip_dangling_symlink_or_fail(err, || {
                    format!(
                        "walking symlinked source dir {} via {}",
                        logical_root.display(),
                        physical_root.display()
                    )
                })? {
                    Some(entry) => entry,
                    None => continue,
                }
            }
        };
        let physical = entry.path();
        let rel = match physical.strip_prefix(&physical_root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let to = dest.join(rel);
        let logical = logical_root.join(rel);
        if entry.file_type().is_dir() {
            ctx.writer.ensure_dir(&to)?;
        } else if entry.file_type().is_file() {
            if is_excluded(&logical) {
                continue;
            }
            if let Some(parent) = to.parent() {
                ctx.writer.ensure_dir(parent)?;
            }
            if raw_source_extension(&logical) {
                materialise_source_file(
                    physical,
                    &logical,
                    &to,
                    is_excluded,
                    true,
                    ctx.writer,
                    &ctx.glob_matched_files,
                    &ctx.raw_import_edges,
                    &ctx.raw_import_aliases,
                    &ctx.module_worker_dependencies,
                    ctx.project_root,
                    &ctx.worker_build_context,
                )?;
            } else {
                ctx.writer.copy_if_changed(physical, &to)?;
            }
        }
    }
    Ok(())
}

/// Copy an exact alias/package directory into its isolated shadow spelling.
/// Unlike ordinary source walks, package-owned dot-directories are preserved
/// because `package.json#imports` may point at them. Nested dependency and VCS
/// trees remain pruned to keep this explicit staging bounded. Reachable source
/// files are preprocessed separately and overwrite these raw copies.
fn materialise_isolated_exact_dir(
    source_root: &Path,
    logical_root: &Path,
    dest: &Path,
    writer: &ShadowWriter<'_>,
    is_excluded: &dyn Fn(&Path) -> bool,
    prune_workspace_infra: bool,
) -> Result<()> {
    let physical_root = source_root.canonicalize().with_context(|| {
        format!(
            "canonicalize isolated exact-target dir {}",
            source_root.display()
        )
    })?;
    writer.ensure_dir(dest)?;
    for entry in WalkDir::new(&physical_root)
        .follow_links(true)
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() == 0 || !entry.file_type().is_dir() {
                return true;
            }
            let name = entry.file_name().to_string_lossy();
            !matches!(name.as_ref(), "node_modules" | ".git")
                && (!prune_workspace_infra || !MIRROR_SKIP_DIRS.iter().any(|skip| name == *skip))
        })
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => match skip_dangling_symlink_or_fail(err, || {
                format!(
                    "walking isolated exact-target dir {} via {}",
                    source_root.display(),
                    physical_root.display()
                )
            })? {
                Some(entry) => entry,
                None => continue,
            },
        };
        let physical = entry.path();
        let Ok(relative) = physical.strip_prefix(&physical_root) else {
            continue;
        };
        let logical = logical_root.join(relative);
        let to = dest.join(relative);
        if entry.file_type().is_dir() {
            writer.ensure_dir(&to)?;
        } else if entry.file_type().is_file() && !is_excluded(&logical) {
            if let Some(parent) = to.parent() {
                writer.ensure_dir(parent)?;
            }
            writer.copy_if_changed(physical, &to)?;
        }
    }
    Ok(())
}

/// Shared context passed to [`materialise_shadow`] and
/// [`materialise_collection`].  All fields are invariant across every call
/// within a single [`bundle`] invocation — they come from [`BundlerInput`]
/// fields that never change between the pages / content / components /
/// layouts / extra-dirs walks.  Per-call-varying data (source dir, dest
/// dir, routes vec, collection name, etc.) is still passed as explicit
/// parameters.
///
/// Lifetime `'a` is the lifetime of the references borrowed from the
/// [`bundle`] stack frame (i.e. `&BundlerInput` fields and the
/// `bundle_exclude` / `writer` locals); `'s` is the [`ShadowSession`]
/// borrow inside the writer (invariant, so it cannot be folded into `'a`).
struct MaterialiseCtx<'a, 's> {
    /// Effective pipeline knob set shared by all MDX compile calls —
    /// `input.pipeline_spec` with `resolve_source_map` rewritten from
    /// `input.resolve_markdown_links` (see [`bundle`]). Both walkers
    /// construct their pipelines via the single
    /// [`zfb_content::PipelineSpec::build_pipeline`] path — the same one
    /// the snapshot walker uses — which structurally guarantees
    /// byte-identical MDX-cache fingerprints and `content_hash` values
    /// across all surfaces (zfb#910 / #917).
    pipeline_spec: zfb_content::PipelineSpec,
    /// Whether to materialise non-MDX source files as real copies rather
    /// than symlinks (required when esbuild runs without
    /// `--preserve-symlinks`).
    copy_mode: bool,
    /// Shared across every `materialise_shadow` call; `materialise_collection`
    /// does not use it (collections have no `bundle.exclude` filter).
    bundle_exclude: &'a BundleExcludeMatcher,
    /// Project root — used by `materialise_shadow` for the `bundle.exclude`
    /// relativisation step.
    project_root: &'a Path,
    /// Conditional-write seam (#993): passthrough for prod, write-if-
    /// changed + visited bookkeeping for the persistent dev shadow
    /// session. Every shadow write in the materialise passes routes
    /// through this.
    writer: &'a ShadowWriter<'s>,
    /// `import.meta.glob(...)` matched target files discovered while
    /// preprocessing source files in every materialise pass (issue #1695).
    /// `materialise_source_file` extends this with every file
    /// `expand_import_meta_glob_with_matches` matches, regardless of which
    /// pass staged the glob-importing file itself. After every explicit
    /// materialise pass has run, `bundle_with_session` drains this set to a
    /// fixed point: a matched file outside every claimed
    /// [`SiblingMirrorPlan`] mirror root — invisible to that plan, because
    /// glob targets never enter the AST-based preprocessing-discovery graph
    /// the plan is built from — is staged on its own via the same
    /// `materialise_source_file` pass, which may in turn extend this set
    /// with ITS OWN matched files. A `BTreeSet` keeps the drain queue's
    /// iteration order deterministic across ticks.
    glob_matched_files: RefCell<BTreeSet<PathBuf>>,
    /// Raw terminal edges discovered while preprocessing source files in
    /// every materialise pass. Used after esbuild's metafile walk to map the
    /// generated `.zfb-raw-*.mjs` input back to the ORIGINAL target for dev
    /// invalidation.
    raw_import_edges: RefCell<BTreeSet<RawImportEdge>>,
    /// First-party tsconfig alias/baseUrl mappings used by terminal `?raw`
    /// expansion. Kept in logical project-root shape so alias-resolved raw
    /// targets register identically to equivalent relative spellings.
    raw_import_aliases: RawImportAliasContext,
    /// Browser-only worker closure paths found while rewriting source URLs.
    /// They feed invalidation bookkeeping but never become SSR imports.
    module_worker_dependencies: RefCell<BTreeSet<ModuleWorkerDependency>>,
    /// Browser worker transform/resolver semantics shared by every source
    /// rewrite in this SSR shadow.
    worker_build_context: ModuleWorkerBuildContext,
    /// True after `bundle_with_session` preflights every source root as one
    /// graph-wide batch. Direct unit helpers leave this false, causing each
    /// standalone materialise call to preflight its own tree.
    raw_preflight_complete: Cell<bool>,
    /// SHA-256-accurate skip signal for collection `.mdx` (#1151). The set
    /// of every `module_specifier` in the per-tick content snapshot the
    /// bundler already receives (`BundlerInput.content_snapshot_json`) —
    /// each is `mdx://<collection>/<slug>#<hash8>`, the hash re-derived from
    /// the file's current bytes every tick by the snapshot walker. The
    /// content-skip check (see [`materialise_mdx_with_skip`]) requires a
    /// collection file's cached bridge specifier to still appear here before
    /// honouring a skip, so a content edit that preserves `(mtime, size)`
    /// (coarse-mtime FS / `touch -r` / `rsync --times`) flips the hash and
    /// correctly invalidates the skip instead of replaying a stale specifier
    /// (the #1151 broken-`<pre>` bug). `None` when no snapshot was supplied
    /// (passthrough / prod-sessionless never skip; tests without a snapshot
    /// fall back to the legacy `(mtime, size)` key).
    snapshot_specifiers: Option<std::collections::HashSet<String>>,
}

/// Build the [`MaterialiseCtx::snapshot_specifiers`] set (#1151) from the
/// per-tick content snapshot JSON the bundler already holds. Returns the set
/// of every entry's `module_specifier` across all collections, or `None`
/// when no snapshot JSON was supplied. A single flat cross-collection set is
/// sound because every specifier embeds its collection segment
/// (`mdx://<collection>/<slug>#hash`), so cross-collection aliasing is
/// impossible. A parse failure degrades to `None` (legacy `(mtime, size)`)
/// rather than failing the build — the snapshot is an optimisation signal,
/// not a correctness input here.
fn snapshot_specifier_set(json: Option<&str>) -> Option<std::collections::HashSet<String>> {
    let json = json?;
    let snapshot: zfb_content::ContentSnapshot = serde_json::from_str(json).ok()?;
    Some(
        snapshot
            .collections
            .into_values()
            .flat_map(|entries| entries.into_iter().map(|e| e.module_specifier))
            .collect(),
    )
}

/// Recursively copy `src` into `dest`, transforming `.mdx` files via
/// [`compile_mdx_to_jsx_module_cached`] so esbuild can parse them as
/// JSX (the `.mdx` extension is preserved; the bundler uses
/// `--loader:.mdx=jsx` to tell esbuild to treat the rewritten body as
/// JSX).
///
/// `routes` is only populated when called with the **pages** dir — for
/// content/components/layouts the caller passes a throwaway vec.
/// Detected routes are recorded in WalkDir traversal order, then sorted
/// by route string later so the manifest is deterministic.
#[allow(clippy::too_many_arguments)] // 8 params: 2 added by #980 side channels; grouping into a struct would obscure the drain contract
fn materialise_shadow(
    src: &Path,
    dest: &Path,
    routes: &mut Vec<RouteEntry>,
    ctx: &MaterialiseCtx<'_, '_>,
    broken_links_out: &mut Vec<(String, String)>,
    markdown_diagnostics_out: &mut Vec<MarkdownDiagnostic>,
    cross_file_links_out: &mut Vec<CrossFileLinkCandidate>,
    file_headings_out: &mut Vec<FileHeadings>,
) -> Result<()> {
    if !src.exists() {
        // A missing source dir is non-fatal — not every project has e.g.
        // `layouts/`. Just skip; entry.mjs will simply not import from
        // it. This matches the "rebuild more, not less" defensive bias
        // of `zfb-build`'s policy module.
        return Ok(());
    }

    ctx.writer
        .ensure_dir(dest)
        .with_context(|| format!("create dir {}", dest.display()))?;
    // Routes are only collected when the caller passed a `routes` vec
    // they actually intend to fill — by convention, only the call for
    // the pages root does this. We detect "is this the pages call?" by
    // checking the destination directory name (`pages`), not the source
    // name, so a user with a non-conventional source layout (e.g.
    // `src/routes/`) still produces routes correctly because the
    // bundler always materialises the shadow root under `pages/`.
    let is_pages_dir = dest.file_name().map(|s| s == "pages").unwrap_or(false);

    // Single `bundle.exclude` predicate used at BOTH application points in
    // this function — the per-file copy/symlink skip below, and the #665
    // `import.meta.glob` expansion seam (`materialise_source_file` →
    // `expand_import_meta_glob`). One closure means the two can never diverge.
    //
    // It takes an ABSOLUTE path (the shape `expand_import_meta_glob` hands its
    // `is_excluded` predicate, per the #665 contract) and relativises it
    // against `project_root` internally. When `bundle.exclude` is empty the
    // matcher never matches, so this is always-false → skip nothing.
    let is_excluded = |abs: &Path| ctx.bundle_exclude.is_excluded(abs, ctx.project_root);

    // Establish every terminal raw-target identity before the broad mirror
    // visits individual files. This is what prevents an invalid `foo.js?raw`
    // payload from being reparsed merely because it also lives below a source
    // root walked by the SSR shadow.
    if !ctx.raw_preflight_complete.get() {
        preflight_raw_tree(src, dest, ctx)?;
    }

    // Hoist a single feature-aware pipeline outside the
    // walk loop so the always-on Core plugins (CJK-friendly
    // emphasis, heading-links, code-title, syntect) plus the opt-in
    // feature visitors (mermaid, directives, …) all fire on every MDX file
    // the walker visits.  Constructing a `Highlighter` and the boxed
    // visitors per file would be wasteful. Borrow is linear (`&mut`), so
    // a single hoisted pipeline serves every MDX file sequentially.
    // See zfb#127 / #128.
    //
    // Note: `zfb dev` is the bundler in Development mode — it also goes
    // through this path.  The `zfb-render ModuleLoader` is a separate
    // library/embedder path not used by the `zfb` CLI at all.
    //
    // The opt-in `StripMdExtensionPlugin` is appended here when the
    // user enabled `stripMdExt` in `zfb.config.ts` (zfb#127 / #129).
    // The plugin is intentionally NOT in `with_defaults()` because it
    // only makes sense for sites whose authors hand-write
    // `[label](other.md)` style references.
    //
    // When `resolve_source_map` is `Some`, the `ResolveLinksPlugin` is
    // also wired into the mdast phase after the directives step so
    // author-written `[label](./other.mdx)` links rewrite to the
    // rendered route URL. The `source_dir` is updated per-file below.
    let mut pipeline = ctx.pipeline_spec.build_pipeline()?;

    // sort_by_file_name() gives lexicographic order within each directory
    // level, matching walk_collection's explicit files.sort() contract so
    // the two walks feed entries to their respective Pipeline instances in
    // the same order.  Without sorting the OS-supplied readdir order is
    // non-deterministic and HeadingLinksPlugin's slug counter can assign
    // "basic-usage-7" in one walk and "basic-usage" in the other,
    // producing different content_hash values and breaking the
    // mdx://<collection>/<slug>#<hash> bridge lookup (zfb#187).
    for entry in WalkDir::new(src)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| !is_pruned_infra_dir(e))
    {
        let entry = entry.with_context(|| format!("walking {}", src.display()))?;
        let from = entry.path();
        // WalkDir always yields paths under `src`, so `strip_prefix`
        // succeeds in practice. We surface the (impossible) failure as
        // an `anyhow::Error` rather than panicking — symlink trickery
        // or future WalkDir behavioural changes shouldn't crash the
        // build.
        let rel = from.strip_prefix(src).map_err(|_| {
            anyhow!(
                "bundler: walked entry {} is not under source root {}",
                from.display(),
                src.display()
            )
        })?;
        let to = dest.join(rel);

        if entry.file_type().is_dir() {
            ctx.writer
                .ensure_dir(&to)
                .with_context(|| format!("create dir {}", to.display()))?;
            continue;
        }
        if !entry.file_type().is_file() {
            // A symlinked *subdir* under the source root. `WalkDir`'s
            // `follow_links(false)` yields it as a non-recursed symlink
            // entry (neither `is_dir()` nor `is_file()`). In the default
            // symlink path that is fine — esbuild canonicalises the parent
            // copy/symlink back to the real tree and finds the subtree
            // there. But under `copy_mode` the parent root is materialised
            // as real copies and esbuild stays anchored at the shadow, so a
            // symlinked subdir would silently stay unmirrored (esbuild would
            // canonicalise it back out, defeating the in-shadow transforms).
            // Explicitly copy the symlinked subtree as real files in that
            // mode. Low-frequency, but a hole otherwise.
            if ctx.copy_mode && entry.path_is_symlink() && from.is_dir() {
                materialise_symlinked_dir(from, &to, ctx, &is_excluded).with_context(|| {
                    format!(
                        "bundler: failed materialising symlinked subdir {} -> {} under copy_mode",
                        from.display(),
                        to.display()
                    )
                })?;
            }
            continue;
        }

        let logical_from = logical_importer_for_walk(src, dest, from, ctx.project_root);

        // `bundle.exclude` skip (#664 / #672). A matched file is never
        // materialised into the shadow tree — so esbuild can never resolve
        // it — AND, because we `continue` before the route-recording block
        // below, an excluded page yields no route (correct: an excluded
        // source must not exist anywhere in the build). The predicate takes
        // the file's absolute path (`from`); empty `bundle.exclude` makes it
        // always-false, so this skip never fires and behaviour is identical
        // to a build without the knob.
        if is_excluded(&logical_from) {
            continue;
        }

        // Pre-compile MDX, leaving the .mdx extension in place.
        let ext = from.extension().and_then(|s| s.to_str());
        let is_mdx = ext == Some("mdx");
        let is_md = ext == Some("md");

        // `.html` page sources bypass the JS bundle entirely. We record
        // them in the manifest so plugin consumers (postBuild, routes
        // manifest) can see them, but we do NOT copy them into the shadow
        // tree or include them in entry.mjs — the renderer reads the
        // source file directly and writes the body verbatim to dist/.
        let is_html_page = ext == Some("html") && is_pages_dir;

        if is_html_page {
            // Record in routes for the manifest, but do not copy to shadow.
            if let Some(route) = derive_route(rel) {
                let abs_src = logical_from.clone();
                let project_rel = abs_src
                    .strip_prefix(ctx.project_root)
                    .map(|p| p.to_path_buf())
                    .unwrap_or(abs_src);
                routes.push(RouteEntry {
                    route: route.clone(),
                    source_path: project_rel,
                    // Hono-form so `worker_only_routes` filter matches.
                    entry_key: bracket_to_hono(&route),
                    static_html: true,
                    // Path under `pages_dir` (the walk's `rel`) — carried from
                    // the walk so the entry import is overlay-location-agnostic
                    // (#1193). Static-HTML routes never enter the JS bundle, so
                    // this is recorded only for manifest symmetry.
                    rel_under_pages: rel.to_path_buf(),
                });
            }
            continue;
        }

        if is_mdx {
            // Incremental-materialise this `.mdx` via the shared core
            // (zfb#1148). This pass produces NO bridge import (the bridge
            // map is built by `materialise_collection`); it only mirrors
            // the compiled shadow `.mdx` for esbuild's resolver. Crucially,
            // the SAME source `src/mdx/foo.mdx` is also materialised by the
            // collection pass to `content/<name>/foo.mdx`, so without an
            // incremental skip here that second walk re-compiled every
            // content file each dev tick (the residual bottleneck after the
            // collection pass went incremental). The skip cache is keyed by
            // DEST, so this pass's entry is independent of the collection
            // pass's. `import.meta.glob` soundness is untouched — only the
            // `.mdx` branch is incremental; `.tsx`/`.js`/`.css`/glob
            // importers below stay full and re-enumerate the on-disk file
            // set each tick. Body is stripped with the local
            // `strip_yaml_frontmatter` (this pass has no snapshot-parity
            // constraint — it emits no bridge specifier).
            let raw =
                fs::read_to_string(from).with_context(|| format!("read mdx {}", from.display()))?;
            let body = strip_yaml_frontmatter(&raw).to_string();
            materialise_mdx_with_skip(
                from,
                &to,
                ctx,
                &mut pipeline,
                &body,
                // No bridge import on this pass — a throwaway sink; the
                // closure always returns `NoBridge`, so nothing is pushed.
                &mut Vec::new(),
                broken_links_out,
                markdown_diagnostics_out,
                cross_file_links_out,
                file_headings_out,
                |_compiled| ImportDecision::NoBridge,
            )?;
        } else if is_md && is_pages_dir {
            // .md page: compile via the MDX pipeline then wrap in a minimal
            // HTML shell.  The compiled body is written to a `_`-prefixed
            // sibling so `derive_route` skips it; the shell module at the
            // original `.md` shadow path becomes the page module esbuild
            // bundles and the router serves.
            pipeline.reset_per_entry();
            if ctx.pipeline_spec.resolve_source_map.is_some() {
                pipeline.set_resolve_links_source_file(from.to_path_buf());
            }
            let raw = fs::read_to_string(from)
                .with_context(|| format!("read md page {}", from.display()))?;
            // Extract frontmatter for title / lang, then compile the body.
            let (frontmatter_value, md_body) = match zfb_frontmatter::extract(from, &raw) {
                Ok(uf) => (uf.value, uf.body.unwrap_or_default()),
                Err(err) => {
                    // Frontmatter parse failure: strip the malformed
                    // delimited block and feed only the body to the MDX
                    // pipeline, recording no frontmatter values. Warn so
                    // the user notices — silently falling back to slug /
                    // default lang would otherwise hide a typo in their
                    // YAML.
                    tracing::warn!(
                        path = %from.display(),
                        error = %err,
                        "md page frontmatter failed to parse; \
                         falling back to slug title and default lang"
                    );
                    (
                        serde_json::Value::Null,
                        strip_yaml_frontmatter(&raw).to_string(),
                    )
                }
            };
            // Same process-global compile cache as the `.mdx` branch
            // above (zfb#905).
            let compiled = compile_mdx_to_jsx_module_cached(
                &md_body,
                from,
                Some(MdxModuleCache::process_global()),
                Some(&mut pipeline),
            )
            .with_context(|| format!("compile md page {}", from.display()))?;
            for diag in pipeline.take_broken_links() {
                broken_links_out.push((from.display().to_string(), diag.url));
            }
            // Drain generic markdown diagnostics adjacent to broken-links
            // drain (zfb#953).
            markdown_diagnostics_out.extend(pipeline.take_markdown_diagnostics());
            // Drain cross-file anchor-check side channels (#980).
            cross_file_links_out.extend(pipeline.take_cross_file_link_candidates());
            file_headings_out.extend(pipeline.take_file_headings());
            // Derive slug from the relative path so the title fallback matches
            // the URL: `about.md` → "about", `index.md` → "index",
            // `blog/post.md` → "post".
            let slug_fallback = rel
                .with_extension("")
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("index")
                .to_string();
            // Body file: `_zfb_md_body_<stem>.jsx` in the same shadow dir.
            // Starts with `_` so `derive_route` ignores it.
            let stem = from.file_stem().and_then(|s| s.to_str()).unwrap_or("page");
            let body_filename = format!("_zfb_md_body_{stem}.jsx");
            let body_shadow_path = to
                .parent()
                .map(|p| p.join(&body_filename))
                .unwrap_or_else(|| PathBuf::from(&body_filename));
            ctx.writer
                .write_if_changed(&body_shadow_path, compiled.jsx_source.as_bytes())
                .with_context(|| format!("write md body to {}", body_shadow_path.display()))?;
            // Shell module at the original `.md` shadow path.
            // Prefix the body import with "./" so esbuild resolves it as a
            // relative path (bare names are interpreted as package specifiers).
            let body_import = format!("./{body_filename}");
            let shell = render_md_page_shell(&frontmatter_value, &slug_fallback, &body_import);
            ctx.writer
                .write_if_changed(&to, shell.as_bytes())
                .with_context(|| format!("write md page shell to {}", to.display()))?;
        } else {
            // Non-MDX source: copy/symlink, expanding eager
            // `import.meta.glob(...)` in JS/TS files first. The SAME
            // `bundle.exclude` predicate used by the per-file skip above is
            // threaded into the glob expansion (#665's `is_excluded` seam) so
            // an excluded file is never emitted as a static import — which
            // would otherwise make esbuild error on the generated import.
            materialise_source_file(
                from,
                &logical_from,
                &to,
                &is_excluded,
                ctx.copy_mode,
                ctx.writer,
                &ctx.glob_matched_files,
                &ctx.raw_import_edges,
                &ctx.raw_import_aliases,
                &ctx.module_worker_dependencies,
                ctx.project_root,
                &ctx.worker_build_context,
            )?;
        }

        // Routes only collected from the pages root.
        if is_pages_dir {
            if let Some(route) = derive_route(rel) {
                let abs_src = logical_from.clone();
                let project_rel = abs_src
                    .strip_prefix(ctx.project_root)
                    .map(|p| p.to_path_buf())
                    .unwrap_or(abs_src);
                routes.push(RouteEntry {
                    route: route.clone(),
                    source_path: project_rel,
                    // Hono-form so `worker_only_routes` filter matches.
                    entry_key: bracket_to_hono(&route),
                    static_html: false,
                    // Path under `pages_dir` (the walk's `rel`) — carried from
                    // the walk so the entry import is overlay-location-agnostic
                    // (#1193, load-bearing).
                    rel_under_pages: rel.to_path_buf(),
                });
            }
        }
    }

    // Sort routes so that Hono registers more-specific routes first.
    //
    // Hono dispatches requests in registration order. Without explicit
    // ordering a fully-dynamic route like `/[lang]/[slug]` (→ `/:lang/:slug`)
    // registered BEFORE `/blog/[slug]` would steal `/blog/hello` by matching
    // it as (lang=blog, slug=hello).
    //
    // The key is the route's per-segment rank vector, compared
    // lexicographically left to right:
    //
    //   static = 0  <  dynamic `[p]` = 1  <  catchall `[...p]` / `[[...p]]` = 2
    //
    // Two overlapping patterns always agree on a common prefix and first
    // differ at a segment where one is looser than the other, so ordering
    // by the first differing rank registers the more-specific route first.
    // Alphabetical order breaks remaining ties (stable and deterministic);
    // rank-tied routes never overlap (their static segments differ).
    //
    // An aggregate count key (the previous design:
    // `(−static, +dynamic, +catchall)`) is NOT sufficient here: it ranked
    // `/docs/[...rest]` (1 static, 1 catchall) before
    // `/docs/[version]/[page]` (1 static, 2 dynamic), letting the catchall
    // steal `/docs/v1/intro` from the deeper dynamic route (or 404 it when
    // the catchall's `paths()` lacks the entry). The per-segment vector
    // compares rank 2 (catchall) against rank 1 (dynamic) at segment index
    // 1 and orders the dynamic descendant first, matching zfb-router's
    // per-segment sort. Probed against Hono 4.12.x: registration order is
    // what decides between the two patterns.
    //
    // Required and optional catchalls share rank 2 — they can never
    // coexist at the same prefix (scan-time conflict), so a finer
    // ordering between them is unreachable.
    //
    // Example ordering for the routing-rendering fixture:
    //   /              → []     — empty vector sorts first (no overlaps)
    //   /about         → [0]    — tie with /blog broken alphabetically
    //   /blog          → [0]
    //   /blog/page/[p] → [0, 0, 1]
    //   /blog/[slug]   → [0, 1]
    //   /docs/[id]     → [0, 1]
    //   /docs/[...s]   → [0, 2] — after /docs/[id] (rank 2 > 1 at index 1)
    //   /[lang]/[slug] → [1, 1] — least specific (dynamic at index 0)
    fn route_sort_key(route: &str) -> Vec<u8> {
        route
            .split('/')
            .filter(|seg| !seg.is_empty())
            .map(|seg| {
                if (seg.starts_with("[[...") && seg.ends_with("]]"))
                    || (seg.starts_with("[...") && seg.ends_with(']'))
                {
                    2
                } else if seg.starts_with('[') && seg.ends_with(']') {
                    1
                } else {
                    0
                }
            })
            .collect()
    }
    routes.sort_by(|a, b| {
        let ka = route_sort_key(&a.route);
        let kb = route_sort_key(&b.route);
        ka.cmp(&kb).then_with(|| a.route.cmp(&b.route))
    });
    // Detect route collisions before silently de-duplicating. Two
    // pages producing the same route from different source extensions
    // (e.g. `index.tsx` and `index.md`) is an authoring bug — surface
    // it with both source paths in the message rather than letting
    // one win arbitrarily.
    for w in routes.windows(2) {
        if w[0].route == w[1].route && w[0].source_path != w[1].source_path {
            return Err(anyhow!(
                "bundler: route collision: {} is produced by both {} and {}",
                w[0].route,
                w[0].source_path.display(),
                w[1].source_path.display(),
            ));
        }
    }
    routes.dedup_by(|a, b| a.route == b.route);
    Ok(())
}

/// MDX files in this codebase commonly carry YAML frontmatter (the
/// `---\n…\n---` block). `compile_mdx_to_jsx_module_cached` does not
/// strip it — that's the caller's job per `mdx_jsx_emit` docs. Mirror
/// the behaviour `zfb-content::collection` uses so the bundler speaks
/// the same dialect.
fn strip_yaml_frontmatter(input: &str) -> &str {
    let trimmed = input.trim_start_matches('\u{feff}');
    if !trimmed.starts_with("---") {
        return input;
    }
    let after_open = &trimmed[3..];
    // Frontmatter open must be followed by a newline.
    let rest_start = after_open
        .find('\n')
        .map(|i| i + 1)
        .unwrap_or(after_open.len());
    let body = &after_open[rest_start..];
    // Look for a `\n---` close marker that itself ends a line.
    if let Some(close_idx) = body.find("\n---") {
        let after_close = &body[close_idx + 4..];
        // Skip optional `\r`/`\n` after the close marker.
        let after_close = after_close.trim_start_matches(['\r', '\n']);
        return after_close;
    }
    input
}

/// Walk the shadow tree and rewrite every `*.module.css` file into a JS
/// module exporting its scoped class-name map as the default export.
///
/// The shadow tree mirrors the project's directory layout
/// (`pages/`, `components/`, `layouts/`, `content/`, plus any extra
/// project-root dir such as `styles/`), so a shadow file at
/// `<shadow>/<rel>` corresponds to the original `<project_root>/<rel>`.
/// We reconstruct that original path and look it up in `class_maps`
/// (which `zfb-css` keys by the absolute `.module.css` path).
///
/// Behaviour per file:
///
/// - **Map present** — write `export default { "orig": "scoped", … }`.
///   Keys are emitted in sorted order for deterministic output. A
///   user's `import styles from "./x.module.css"; styles.foo` then
///   resolves to the scoped class string.
/// - **Map absent** — write `export default {};`. This happens when
///   the CSS pipeline did not see the file (e.g. it is reached only
///   through a deep TSX→TSX import chain the flat scanner misses, or
///   the caller never ran the CSS pipeline). The build does not crash;
///   `styles.foo` is `undefined` and the markup ships without that
///   class — a graceful degradation rather than a hard failure.
///
/// Either way the file content becomes valid JS, so the
/// `--loader:.module.css=js` esbuild flag always parses successfully.
/// The raw CSS bytes are not needed here — the scoped CSS is emitted
/// separately by `CssPipeline::build_emitter` into the hashed global
/// stylesheet.
fn rewrite_css_modules_in_shadow(
    shadow: &Path,
    project_root: &Path,
    class_maps: &HashMap<PathBuf, HashMap<String, String>>,
    writer: &ShadowWriter<'_>,
) -> Result<()> {
    // FIX #553: use `entry.path().is_file()` instead of
    // `entry.file_type().is_file()` so .module.css symlinks materialised
    // by symlink_or_copy in materialise_shadow are not skipped.
    // With the old `entry.file_type().is_file()` gate, WalkDir (running
    // with follow_links(false)) reports a symlink-to-file as
    // is_symlink()==true / is_file()==false, so every .module.css symlink
    // was silently skipped. `Path::is_file()` follows symlinks, so it
    // returns true for symlinks-to-files and false for broken symlinks,
    // directories, and symlinks-to-directories — correctly handling the
    // node_modules symlink structures that shadow trees can contain.
    for entry in WalkDir::new(shadow).follow_links(false) {
        let entry = entry.with_context(|| format!("walking shadow {}", shadow.display()))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        // Match the `.module.css` double-extension exactly.
        if !path
            .file_name()
            .and_then(|s| s.to_str())
            .map(|n| n.ends_with(".module.css"))
            .unwrap_or(false)
        {
            continue;
        }

        // Reconstruct the original project path: shadow-relative path
        // joined onto the project root.
        let rel = path.strip_prefix(shadow).map_err(|_| {
            anyhow!(
                "bundler: shadow file {} is not under shadow root {}",
                path.display(),
                shadow.display()
            )
        })?;
        let original = project_root.join(rel);

        let names = class_maps.get(&original);
        let js = render_css_module_js(names);

        // FIX #553 (critical): never write THROUGH the symlink (that
        // would corrupt the user's source file in the project root).
        // `write_if_changed*` preserves this contract in both modes:
        // passthrough removes the entry first unconditionally; session
        // mode removes it whenever the path's last recorded provenance
        // is not "bytes we wrote" (a fresh symlink drops the record).
        //
        // `_no_visit` variant (#993): this walk visits the WHOLE shadow
        // tree, including files whose source was deleted this tick —
        // marking those visited would shield them from the prune pass.
        // Legit `.module.css` files were already marked visited by their
        // materialise pass.
        writer
            .write_if_changed_no_visit(path, js.as_bytes())
            .with_context(|| {
                format!(
                    "bundler: failed writing CSS Modules JS shim to {}",
                    path.display()
                )
            })?;
    }
    Ok(())
}

/// Sibling variant of [`rewrite_css_modules_in_shadow`] (issue #1697). A
/// claimed sibling `.module.css` — staged by the wholesale mirror (#1692) or
/// the glob fixed-point queue (#1695) — lands at its own workspace-relative
/// slot under `work`, OUTSIDE the nested project mirror `shadow`, so the
/// walk above never visits it. Walk `work` again, pruning the already-handled
/// `shadow` subtree, and reconstruct each file's original path via
/// `first_party_root` (not `project_root`) — the reconstruction
/// `rewrite_css_modules_in_shadow` uses would be wrong here: a sibling
/// file's original absolute path lives outside `project_root` entirely.
///
/// `class_maps` is the SAME map the project walk uses: #1696's
/// `compute_css_module_class_maps` already keys a claimed sibling's entry by
/// its absolute physical path (under `first_party_root`), so no second map
/// is needed — a lookup miss here means "unmapped" and is handled exactly
/// like an unmapped project file (`export default {};`). That also covers a
/// `.module.css` staged only via the #1695 glob queue outside any claimed
/// mirror root (invisible to `SiblingMirrorPlan`, so it can never have a map
/// entry): it still needs a valid JS shim, since the paired
/// `--loader:.module.css=js` esbuild flag applies to the extension
/// regardless of claim status.
///
/// The caller gates this on `workspace_rel.is_some()`: without a workspace
/// `shadow == work`, so pruning `shadow` here would prune the walk root
/// itself and this becomes an inert no-op anyway — the gate just skips the
/// wasted walk, keeping non-workspace builds byte-identical.
fn rewrite_css_modules_in_work_mirror(
    work: &Path,
    shadow: &Path,
    first_party_root: &Path,
    class_maps: &HashMap<PathBuf, HashMap<String, String>>,
    writer: &ShadowWriter<'_>,
) -> Result<()> {
    for entry in WalkDir::new(work)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| e.path() != shadow)
    {
        let entry = entry.with_context(|| format!("walking work mirror {}", work.display()))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        // Match the `.module.css` double-extension exactly, same as the
        // project-mirror walk.
        if !path
            .file_name()
            .and_then(|s| s.to_str())
            .map(|n| n.ends_with(".module.css"))
            .unwrap_or(false)
        {
            continue;
        }

        // Reconstruct the original sibling path: work-relative path joined
        // onto the WORKSPACE root, not the project root.
        let rel = path.strip_prefix(work).map_err(|_| {
            anyhow!(
                "bundler: work-mirror file {} is not under work root {}",
                path.display(),
                work.display()
            )
        })?;
        let original = first_party_root.join(rel);

        let names = class_maps.get(&original);
        let js = render_css_module_js(names);

        // See FIX #553 on `rewrite_css_modules_in_shadow`: never write
        // through a symlink, and use the `_no_visit` variant since this walk
        // (like that one) covers the whole tree including files whose source
        // was deleted this tick.
        writer
            .write_if_changed_no_visit(path, js.as_bytes())
            .with_context(|| {
                format!(
                    "bundler: failed writing CSS Modules JS shim to {}",
                    path.display()
                )
            })?;
    }
    Ok(())
}

/// Render the JS source for a CSS Modules shim file.
///
/// `Some(map)` → `export default { "orig": "scoped", … };` with keys
/// sorted for deterministic output. `None` → `export default {};`.
fn render_css_module_js(names: Option<&HashMap<String, String>>) -> String {
    match names {
        Some(map) if !map.is_empty() => {
            let mut sorted: Vec<(&String, &String)> = map.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(b.0));
            let body = sorted
                .into_iter()
                .map(|(k, v)| format!("  {}: {}", json_str(k), json_str(v)))
                .collect::<Vec<_>>()
                .join(",\n");
            format!("export default {{\n{body}\n}};\n")
        }
        _ => "export default {};\n".to_string(),
    }
}

/// One MDX entry materialised under `shadow/content/<collection>/`,
/// recorded so the synthetic `entry.mjs` can:
///
/// 1. `import * as __zfb_content_<i> from "./<shadow_rel_path>"` for
///    each entry, and
/// 2. register `[<specifier>, __zfb_content_<i>.default]` in the
///    `globalThis.__zfb.content` bridge map keyed on the
///    `mdx://<collection>/<slug>#<hash>` form returned by
///    [`compile_mdx_to_jsx_module_cached`].
///
/// `shadow_rel_path` is always forward-slash-separated and starts with
/// `content/<name>/...` so the import string composed from it stays
/// portable across OSes.
#[derive(Debug, Clone)]
struct ContentImport {
    /// Specifier baked by `compile_mdx_to_jsx_module_cached` —
    /// `mdx://<collection_seg>/<slug_seg>#<hash>`. This is the same
    /// value that `EntrySnapshot.module_specifier` carries on the JS
    /// side, so a `bridge.get(entry.module_specifier)` call inside
    /// `<CollectionEntry.Content>` resolves to its compiled module.
    specifier: String,
    /// Path relative to the shadow root, in forward-slash form (e.g.
    /// `content/docs/getting-started/installation.mdx`). Used both as
    /// the `import` target string and as the deterministic sort key.
    shadow_rel_path: String,
}

/// On-disk state of one source/dependency file captured at materialise
/// time and re-compared on a later tick's skip check (zfb#1148):
/// `Some((mtime, size))` when the file was present, `None` when a
/// recorded read found it absent (a `Missing` transclude / link target).
/// The skip holds only while the live stat reproduces this exact state —
/// a present file must stat to the same `(mtime, size)`; an absent file
/// must still fail to stat.
type FileStat = Option<(std::time::SystemTime, u64)>;

/// Capture the current on-disk `(mtime, size)` of `path`, or `None` if it
/// cannot be stat'd (absent / permission / I-O). The same observation is
/// taken at store time and at skip-check time, so the comparison is
/// apples-to-apples: a vanished present-file or an appeared absent-file
/// both flip the value and correctly invalidate the skip.
fn file_stat(path: &Path) -> FileStat {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok().map(|t| (t, m.len())))
}

/// One content `.md`/`.mdx` file's cached materialise result, keyed in
/// [`ShadowSession::content_skip`] by its DESTINATION shadow-relative
/// path. Lets a later session tick reuse this file's bridge import (when
/// any) + cross-file-check contribution instead of re-reading /
/// re-compiling / re-writing it (zfb#1148), as long as the stored
/// SOURCE's `(mtime, size)` AND every recorded dependency's on-disk state
/// are unchanged — AND, for collection files (those carrying a bridge
/// [`import`](Self::import)), the cached specifier still appears in the
/// per-tick snapshot specifier set (zfb#1151, the SHA-256-accurate gate; see
/// [`materialise_mdx_with_skip`]).
///
/// Stored for EVERY successfully-compiled content file (the thorough
/// dep-mtime variant), not only dep-free ones: the external reads this
/// file's compile recorded through the `ReadRecorder` are captured in
/// `deps` and re-checked on the skip path, so a file with deps is skipped
/// while those deps are byte-stable and re-materialised the moment any of
/// them changes. A file with zero recorded reads is the trivial
/// all-deps-unchanged case → still skippable. Used by both
/// [`materialise_collection`] (bridge pass) and [`materialise_shadow`]'s
/// `.mdx` branch (the `src/` extra-top-level-dir pass) via the shared
/// [`materialise_mdx_with_skip`] helper.
///
/// NOTE: `deps` covers ONLY recorder-tracked filesystem reads — transclude
/// `:::include` targets, link-validation existence/anchor probes, and
/// image-dimension probes. It does NOT cover `resolve_source_map`
/// (`resolveMarkdownLinks`): `ResolveLinksPlugin` rewrites `./other.mdx`
/// links from an IN-MEMORY route→URL map and records nothing, so a
/// resolve-links page's `deps` are empty. A route-map change is instead
/// caught by the pipeline config-fingerprint wipe (see
/// [`ShadowSession::config_fingerprint`]), not by `deps`.
#[derive(Debug, Clone)]
struct ContentSkipEntry {
    /// Absolute SOURCE path this dest was materialised from. The dest is
    /// the cache KEY, but the skip check stats the SOURCE — and the same
    /// dest could in principle be reused for a different source after a
    /// restructure, so the entry pins which source it describes and the
    /// caller re-confirms it matches before honouring the skip.
    source: PathBuf,
    /// Source-file mtime at the time the cached result was produced. With
    /// `size`, the file's own half of the skip key.
    mtime: std::time::SystemTime,
    /// Source-file byte length at cache time.
    size: u64,
    /// Each external file this compile recorded a read of through the
    /// `ReadRecorder` (transclude `:::include` targets, link-validation
    /// existence/anchor probes, image-dimension probes — NOT
    /// resolve-links, which records nothing; see the type-level note),
    /// paired with its on-disk state at materialise time (see
    /// [`FileStat`]). On a skip check EVERY dep must re-stat to the same
    /// state, else the dependent file takes the full path. Soundness: any
    /// edit to a recorded dep bumps its mtime (or changes its size), so
    /// the dependent file re-validates / re-rewrites; a previously-absent
    /// dep that appears flips `None`→`Some` and likewise invalidates.
    /// Empty for files with no recorded reads (then the skip reduces to
    /// the file's own `(mtime, size)` check).
    deps: Vec<(PathBuf, FileStat)>,
    /// Shadow-relative dest path (forward-slash form) the compiled JSX
    /// was written to, e.g. `content/docs/intro.mdx`. Re-derived as the
    /// absolute shadow path to confirm the file still exists before a
    /// skip, and recorded as "visited" on skip so the prune pass does not
    /// delete it. Equals the rendered cache key.
    shadow_rel_path: String,
    /// The exact bridge import the full compile produced — REPLAYED
    /// verbatim on a skip so the snapshot↔bridge byte-for-byte specifier
    /// parity holds (the snapshot walker bakes the matching
    /// `module_specifier`; recomputing the specifier on skip risks a
    /// drift for `idStripSuffix` / EN-sibling collections, so we never
    /// recompute — we replay). `None` for the `materialise_shadow` pass,
    /// which produces no bridge import (the `src/` walk only mirrors the
    /// shadow `.mdx` for esbuild's resolver; the bridge map is built by
    /// the collection pass).
    import: Option<ContentImport>,
    /// This file's `FileHeadings` contribution — replayed into
    /// `file_headings_out` on skip so the build-wide cross-file anchor
    /// check (which runs every tick) still sees this file's headings. At
    /// most one entry for a single compile (`Vec` to mirror the
    /// drain shape; empty when the file has no anchor-addressable
    /// headings).
    headings: Vec<FileHeadings>,
    /// This file's cross-file fragment-link candidates — replayed into
    /// `cross_file_links_out` on skip for the same reason: a link FROM a
    /// changed file TO a heading in this skipped file must still resolve.
    cross_links: Vec<CrossFileLinkCandidate>,
}

/// One NON-MDX source/asset file's cached materialise state, keyed in
/// [`ShadowSession::source_skip`] by its DESTINATION shadow-relative path
/// (zfb#1148). Lets a later tick skip the plain copy/symlink of a file
/// whose own `(mtime, size)` is unchanged.
///
/// `materialise_source_file` either applies a cross-file transform
/// (`import.meta.glob`, `?raw`, or a module-worker URL), then writes, OR makes
/// a plain copy/symlink (a pure function of the file's own bytes). So a file
/// is skippable iff its `(mtime, size)` is unchanged AND it does not depend on
/// one of those external inputs. The persistent shadow already holds
/// the correct copy/symlink from the last full pass; a skip only
/// re-marks the dest visited (so the prune keeps it) — no read, no copy,
/// no expand.
#[derive(Debug, Clone)]
struct SourceSkipEntry {
    /// Absolute SOURCE path this dest was materialised from. The dest is
    /// the cache KEY; the skip check stats the SOURCE and re-confirms it
    /// matches before honouring the skip.
    source: PathBuf,
    /// Source-file mtime at materialise time. With `size`, the skip key.
    mtime: std::time::SystemTime,
    /// Source-file byte length at materialise time.
    size: u64,
    /// Whether this file's text contained `import.meta.glob` at materialise
    /// time (always `false` for binary/asset files that skip the UTF-8
    /// pre-read entirely). A glob file's expansion depends on the live
    /// project tree, so it is NEVER skipped — it gets an entry, but the
    /// skip-check's `!has_glob` gate refuses it, re-expanding every tick. A
    /// file flipping to/from using a glob changes its mtime → full path →
    /// `has_glob` re-detected.
    has_glob: bool,
    /// Whether this importer generated one or more terminal raw modules.
    /// Their bytes depend on another file, so the importer must be
    /// reprocessed every persistent-shadow tick even when its own stat is
    /// unchanged.
    has_raw: bool,
    /// Whether this source owns a module-worker URL rewrite. Its emitted query
    /// hashes a separate browser-only graph, so the importer must be
    /// reprocessed even when its own stat is unchanged.
    has_worker: bool,
}

/// One sibling-mirror file's cached materialise state, keyed in
/// [`ShadowSession::mirror_skip`] by its DESTINATION shadow-relative path
/// (issue #1777). [`mirror_sibling_root`] always performs a plain raw byte
/// copy — no glob/`?raw`/worker expansion ever runs on this pass (those
/// rewrites are the LATER preprocessing-materialise pass's job) — so a
/// mirrored file is skippable purely on its own `(mtime, size)` being
/// unchanged, with no `has_glob`/`has_raw`/`has_worker` gate to carry.
#[derive(Debug, Clone)]
struct MirrorSkipEntry {
    /// Absolute SOURCE path this dest was mirrored from. The dest is the
    /// cache KEY; the skip check stats the source and re-confirms it
    /// matches before honouring the skip.
    source: PathBuf,
    /// Source-file mtime at mirror time. With `size`, the skip key.
    mtime: std::time::SystemTime,
    /// Source-file byte length at mirror time.
    size: u64,
}

/// What to do with the bridge import after a full MDX compile inside
/// [`materialise_mdx_with_skip`]. The caller decides because the bridge
/// import is pass-specific:
/// - the collection pass builds the `mdx://…` import (and may suppress it
///   when the compiled JSX would break esbuild — the defensive skip);
/// - the `materialise_shadow` `src/` pass never produces a bridge import.
enum ImportDecision {
    /// Collection pass, healthy JSX: push this import and store a
    /// skippable entry carrying it (replayed verbatim on a later skip).
    Bridge(ContentImport),
    /// `materialise_shadow` pass: no bridge import, but the file is still
    /// cached (entry carries `import: None`) so a later tick can skip it.
    NoBridge,
    /// Collection pass, defensive broken-JSX skip: the file was written to
    /// the shadow but contributes no bridge import and must NOT be cached
    /// — every tick recompiles (and re-warns) until the source is fixed.
    DoNotCache,
}

/// Shared incremental-materialise core for a single `.md`/`.mdx` content
/// file (zfb#1148), used by BOTH [`materialise_collection`] (bridge pass)
/// and [`materialise_shadow`]'s `.mdx` branch (the `src/`
/// extra-top-level-dir pass). The same source is materialised into two
/// distinct shadow dests each tick, so the skip cache is keyed by the
/// DESTINATION shadow-relative path — each pass gets its own entry.
///
/// Returns `true` when the file was SKIPPED (cached outputs replayed, no
/// read/compile/write), `false` when it was fully materialised.
///
/// The caller supplies `body` (already frontmatter-stripped — the two
/// passes strip differently, and the collection pass needs byte-parity
/// with the snapshot walker) and `import_decision`, a closure that — on
/// the full path, after the compile — inspects the `CompiledMdx` and
/// decides the bridge-import disposition (see [`ImportDecision`]).
///
/// On a skip the helper: replays the cached import (if any) into
/// `imports`, extends `file_headings_out` / `cross_file_links_out` with
/// the cached records (so the build-wide cross-file anchor check still
/// sees this file), and marks the dest shadow path visited so the prune
/// pass keeps it. It deliberately does NOT replay per-file
/// `broken_links_out` / `markdown_diagnostics_out`: those are intra-file
/// warnings for an unchanged file and need not be re-emitted each tick.
#[allow(clippy::too_many_arguments)]
fn materialise_mdx_with_skip(
    from: &Path,
    to: &Path,
    ctx: &MaterialiseCtx<'_, '_>,
    pipeline: &mut zfb_content::pipeline::Pipeline,
    body: &str,
    imports: &mut Vec<ContentImport>,
    broken_links_out: &mut Vec<(String, String)>,
    markdown_diagnostics_out: &mut Vec<MarkdownDiagnostic>,
    cross_file_links_out: &mut Vec<CrossFileLinkCandidate>,
    file_headings_out: &mut Vec<FileHeadings>,
    import_decision: impl FnOnce(&CompiledMdx) -> ImportDecision,
) -> Result<bool> {
    // The cache is keyed by the DEST shadow-relative path. Compute it
    // once (used both for the lookup/store key and for the
    // visited-on-skip mark). In session mode `rel_of` must succeed (the
    // dest is always under the shadow root); in passthrough we never skip
    // so the key is unused.
    let dest_rel = ctx.writer.rel_of(to).ok();

    // ---- Skip path (session mode only — passthrough/prod never skips) ----
    //
    // Collection `.mdx` (entries carrying a bridge `import`) are gated
    // SHA-256-accurately via the snapshot specifier set (#1151, see below):
    // a content edit that preserves `(mtime, size)` flips the snapshot hash
    // and correctly invalidates the skip. The SOURCE / `materialise_shadow`
    // pass (`import: None`) and the `source_skip` path retain the
    // `(mtime, size)` key by design — they have no snapshot↔bridge invariant,
    // so a stale plain-copy is at worst byte-stale, never a broken `<pre>`.
    // That residual `(mtime, size)` limitation (coarse-mtime FS /
    // `touch -r` / `rsync --times`) is the accepted, documented one; prod
    // never skips (sessionless).
    if ctx.writer.in_session() {
        if let Some(dest_rel) = dest_rel.as_ref() {
            if let Some((mtime, size)) = file_stat(from) {
                if let Some(entry) = ctx.writer.content_skip_get(dest_rel) {
                    // Never false-reuse (rule 5): the entry must describe
                    // THIS source, the source's own `(mtime, size)` must
                    // match, every recorded dep must re-stat unchanged, and
                    // the cached dest shadow file must still exist. Any
                    // mismatch / failed stat / vanished dest → full path.
                    let dest_exists = ctx
                        .writer
                        .shadow_root
                        .join(
                            entry
                                .shadow_rel_path
                                .replace('/', std::path::MAIN_SEPARATOR_STR),
                        )
                        .exists();
                    let deps_unchanged = entry
                        .deps
                        .iter()
                        .all(|(dep_path, recorded)| file_stat(dep_path) == *recorded);
                    // #1151: SHA-accurate gate for collection `.mdx`. A
                    // collection entry carries a bridge `import` whose
                    // `.specifier` is byte-identical to the snapshot's
                    // `module_specifier` (`mdx://col/slug#hash`). The skip is
                    // valid only while that exact specifier still appears in
                    // the current snapshot — i.e. the file's live content
                    // re-hashes to the same value. A content edit preserving
                    // `(mtime, size)` (or a transclude-dep content change the
                    // snapshot's inlined re-hash reflects) flips the hash, so
                    // the stored specifier drops out of the set and we fall
                    // through to a full recompile instead of replaying a
                    // stale bridge specifier. Additive: it can only turn a
                    // would-be skip into a recompile, never the reverse.
                    // Source files (`import: None`) and snapshot-absent runs
                    // skip this gate and keep the legacy `(mtime, size)` key.
                    let snapshot_hash_ok = match (&entry.import, &ctx.snapshot_specifiers) {
                        (Some(import), Some(specifiers)) => specifiers.contains(&import.specifier),
                        (Some(_), None) => {
                            // In production dev session mode the snapshot is
                            // always built first, so this is a should-not-happen
                            // state — but the no-snapshot fallback is a legitimate,
                            // supported path (passthrough, and unit tests), so we
                            // must NOT panic. Emit a debug signal so a future
                            // refactor that drops the dev snapshot leaves a trace
                            // (the collection skip would silently revert to the
                            // weaker (mtime,size) key, un-fixing #1151), then fall
                            // back to the legacy key.
                            tracing::debug!(
                                "#1151: collection content_skip entry but no snapshot specifier \
                                 set in session mode — falling back to the weaker (mtime,size) key"
                            );
                            true
                        }
                        (None, _) => true,
                    };
                    if entry.source == from
                        && entry.mtime == mtime
                        && entry.size == size
                        && deps_unchanged
                        && snapshot_hash_ok
                        && dest_exists
                    {
                        if let Some(import) = &entry.import {
                            imports.push(import.clone());
                        }
                        file_headings_out.extend(entry.headings.iter().cloned());
                        cross_file_links_out.extend(entry.cross_links.iter().cloned());
                        // Mark the un-rewritten shadow file visited so the
                        // prune pass keeps it (rule 4).
                        ctx.writer.record_visited(to).with_context(|| {
                            format!("record visited (content skip) {}", to.display())
                        })?;
                        return Ok(true);
                    }
                }
            }
        }
    }

    // ---- Full path: compile, write, and (re)store the skip entry. ----

    // Reset per-document state (e.g. HeadingLinksPlugin's slug counter)
    // before each new MDX file (zfb#187).
    pipeline.reset_per_entry();
    // Update per-file source context for ResolveLinksPlugin (file path
    // arms the zfb#1030 URL-space fallback for non-index pages).
    if ctx.pipeline_spec.resolve_source_map.is_some() {
        pipeline.set_resolve_links_source_file(from.to_path_buf());
    }

    // Process-global compile cache (zfb#905) + dep-path signal (zfb#1148).
    let (compiled, recorded_deps) = compile_mdx_to_jsx_module_cached_with_deps(
        body,
        from,
        Some(MdxModuleCache::process_global()),
        Some(pipeline),
    )
    .with_context(|| format!("compile mdx {}", from.display()))?;
    // Drain broken-link diagnostics and record them with the file path.
    for diag in pipeline.take_broken_links() {
        broken_links_out.push((from.display().to_string(), diag.url));
    }
    // Drain generic markdown diagnostics adjacent to broken-links (zfb#953).
    markdown_diagnostics_out.extend(pipeline.take_markdown_diagnostics());
    // Drain cross-file anchor-check side channels (#980). Snapshot the
    // out-param lengths BEFORE extending so we can slice out exactly THIS
    // file's contribution for the skip cache — the cached entry must
    // replay this file's own headings / cross-links, not the whole
    // accumulator (zfb#1148).
    let cross_links_base = cross_file_links_out.len();
    let headings_base = file_headings_out.len();
    cross_file_links_out.extend(pipeline.take_cross_file_link_candidates());
    file_headings_out.extend(pipeline.take_file_headings());
    ctx.writer
        .write_if_changed(to, compiled.jsx_source.as_bytes())
        .with_context(|| format!("write compiled mdx to {}", to.display()))?;

    // Caller decides the bridge-import disposition for this compile.
    let decision = import_decision(&compiled);

    // Maintain the skip cache (session mode only; no-op in passthrough).
    // The dest_rel key is required to store — without it we cannot cache.
    let import = match decision {
        ImportDecision::Bridge(import) => {
            imports.push(import.clone());
            Some(import)
        }
        ImportDecision::NoBridge => None,
        ImportDecision::DoNotCache => {
            // Broken-JSX defensive skip: never cache; drop any stale entry
            // so a later tick recompiles.
            if let Some(dest_rel) = dest_rel.as_ref() {
                ctx.writer.content_skip_remove(dest_rel);
            }
            return Ok(false);
        }
    };

    if ctx.writer.in_session() {
        if let Some(dest_rel) = dest_rel {
            match file_stat(from) {
                Some((mtime, size)) => {
                    let headings = file_headings_out[headings_base..].to_vec();
                    let cross_links = cross_file_links_out[cross_links_base..].to_vec();
                    // Stat each recorded dep NOW (right after the compile
                    // observed it) so the stored state matches what the
                    // compile actually read.
                    let deps: Vec<(PathBuf, FileStat)> = recorded_deps
                        .into_iter()
                        .map(|p| {
                            let st = file_stat(&p);
                            (p, st)
                        })
                        .collect();
                    let shadow_rel_path = path_to_posix_string(&dest_rel);
                    ctx.writer.content_skip_store(
                        dest_rel,
                        ContentSkipEntry {
                            source: from.to_path_buf(),
                            mtime,
                            size,
                            deps,
                            shadow_rel_path,
                            import,
                            headings,
                            cross_links,
                        },
                    );
                }
                // Could not stat the file itself — cannot build a sound
                // skip key: drop any stale entry.
                None => ctx.writer.content_skip_remove(&dest_rel),
            }
        }
    }

    Ok(false)
}

/// Walk one content collection's source root and materialise its
/// entries into `dest`, compiling MDX to JSX on the fly via
/// [`compile_mdx_to_jsx_module_cached`] and recording every entry in
/// `imports` for the bridge installer (#506).
///
/// Mirrors the single-root [`materialise_shadow`] behaviour for files
/// (.mdx → JSX-rewritten with .mdx extension preserved; everything
/// else copied verbatim) but:
///
/// - Always operates in "content" mode — never collects routes.
/// - Records `(specifier, shadow_rel_path)` pairs so the synthetic
///   `entry.mjs` can emit one `import * as __zfb_content_<i>` line
///   per MDX entry plus the matching bridge map.
/// - Uses the configured collection `name` as the shadow-tree prefix
///   (`content/<name>/<rel>`), giving each collection its own subtree
///   so two collections that share a slug don't collide.
///
/// A missing source root is non-fatal — the caller may have a stale
/// config entry pointing at a deleted directory; the build should
/// proceed with zero entries from that collection rather than aborting.
#[allow(clippy::too_many_arguments)]
fn materialise_collection(
    src: &Path,
    dest: &Path,
    collection_name: &str,
    imports: &mut Vec<ContentImport>,
    ctx: &MaterialiseCtx<'_, '_>,
    include: Option<&[String]>,
    exclude: Option<&[String]>,
    id_strip_suffix: Option<&str>,
    broken_links_out: &mut Vec<(String, String)>,
    markdown_diagnostics_out: &mut Vec<MarkdownDiagnostic>,
    cross_file_links_out: &mut Vec<CrossFileLinkCandidate>,
    file_headings_out: &mut Vec<FileHeadings>,
) -> Result<()> {
    if !src.exists() {
        return Ok(());
    }
    ctx.writer
        .ensure_dir(dest)
        .with_context(|| format!("create dir {}", dest.display()))?;
    if !ctx.raw_preflight_complete.get() {
        preflight_raw_tree(src, dest, ctx)?;
    }

    // Compile the include / exclude globs once per collection. The
    // shared `CollectionFilter` MUST match `CollectionConfig::*` on the
    // snapshot side byte-for-byte — both surfaces feed the same JSON
    // bridge, so any divergence in which files survive (or which slug
    // each gets) silently turns every consumer-side `getCollection` /
    // `<Content />` lookup into a `<pre data-zfb-content-fallback>`
    // block.
    let filter = zfb_content::collection::CollectionFilter::new(include, exclude, id_strip_suffix)
        .with_context(|| {
            format!(
                "bundler: failed to compile collection filter for `{}`",
                collection_name
            )
        })?;
    let has_glob_filter = include.map(|p| !p.is_empty()).unwrap_or(false)
        || exclude.map(|p| !p.is_empty()).unwrap_or(false);
    // `bundle.exclude` is global to the SSR graph and separate from the
    // collection's snapshot/bridge include-exclude filter above. Keep one
    // predicate for every non-Markdown collection materialisation seam so a
    // plugin-aware file skipped later cannot already have been written here.
    let is_bundle_excluded = |path: &Path| ctx.bundle_exclude.is_excluded(path, ctx.project_root);
    // Re-derive the same suffix `CollectionFilter` would have stored
    // (empty / whitespace → None) so the bundler's specifier rewrite
    // and the walker's rewrite agree on whether to strip.
    let strip_suffix = id_strip_suffix.map(str::trim).filter(|s| !s.is_empty());

    // Hoist a single `Pipeline::with_defaults_and_theme()` outside the
    // walk loop so the seven default plugins fire on every collection
    // MDX file. See `materialise_shadow` for the rationale; the same
    // applies here. Two walks → two hoisted pipelines (one per walker),
    // which is cheaper than one per file. See zfb#127 / #128.
    //
    // The opt-in `StripMdExtensionPlugin` is appended when the user
    // enabled `stripMdExt` (zfb#127 / #129). The flag is threaded in
    // by the caller so the page walker and the collection walker
    // honour the same setting.
    //
    // When `resolve_source_map` is `Some`, the `ResolveLinksPlugin` is
    // also wired after the directives step in the mdast phase. The
    // `source_dir` is updated per-file inside the walk loop.
    let mut pipeline = ctx.pipeline_spec.build_pipeline()?;

    // sort_by_file_name() gives lexicographic order within each directory
    // level, matching walk_collection's explicit files.sort() contract so
    // the two walks feed entries to their respective Pipeline instances in
    // the same order (zfb#187).
    for entry in WalkDir::new(src)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| !is_pruned_infra_dir(e))
    {
        let entry = entry.with_context(|| format!("walking {}", src.display()))?;
        let from = entry.path();
        let rel = from.strip_prefix(src).map_err(|_| {
            anyhow!(
                "bundler: walked entry {} is not under collection root {}",
                from.display(),
                src.display()
            )
        })?;
        let to = dest.join(rel);

        if entry.file_type().is_dir() {
            ctx.writer
                .ensure_dir(&to)
                .with_context(|| format!("create dir {}", to.display()))?;
            continue;
        }
        if !entry.file_type().is_file() {
            // Symlinked subdir under copy_mode — copy the real subtree so it
            // stays mirrored in the shadow (see the matching block in
            // `materialise_shadow`).
            if ctx.copy_mode && entry.path_is_symlink() && from.is_dir() {
                materialise_symlinked_dir(from, &to, ctx, &is_bundle_excluded).with_context(
                    || {
                        format!(
                        "bundler: failed materialising symlinked subdir {} -> {} under copy_mode",
                        from.display(),
                        to.display()
                    )
                    },
                )?;
            }
            continue;
        }

        // Apply include / exclude globs to recognised content extensions
        // only (md / mdx / tsx). Non-content siblings (images, css,
        // json, …) pass through unchanged — they live in the shadow
        // tree purely for esbuild's resolver and never reach the
        // snapshot or bridge map, so filtering them would just diverge
        // from the walker's coverage.
        let is_content_ext = matches!(
            from.extension().and_then(|s| s.to_str()),
            // page-extension-drift-guard: allow — CONTENT-collection extensions
            // for the shadow-tree glob filter, not the routable page allowlist.
            Some("md") | Some("mdx") | Some("tsx")
        );
        if is_content_ext && has_glob_filter {
            let rel_posix = path_to_posix_string(rel);
            if !filter.matches_relative(&rel_posix) {
                // Filtered out — neither materialise the shadow file
                // nor record a bridge import. The walker on the
                // snapshot side reaches the identical decision via
                // `CollectionFilter::matches`, keeping the two
                // surfaces in lock-step.
                continue;
            }
        }

        let is_markdown = matches!(
            from.extension().and_then(|s| s.to_str()),
            Some("md") | Some("mdx")
        );
        if is_bundle_excluded(from) {
            if is_markdown {
                bail!(
                    "bundler: bundle.exclude matched collection Markdown `{}` in `{}`; \
                     excluding collection .md/.mdx sources is unsupported because the content \
                     snapshot and SSR bridge must stay synchronized. Use the collection's \
                     `exclude` filter instead",
                    from.display(),
                    collection_name
                );
            }
            // Every non-Markdown file is an SSR-resolver companion and must
            // obey the global policy before its own source is written;
            // `materialise_source_file` only applies the predicate to nested
            // glob/raw targets, not to `from` itself.
            continue;
        }
        if is_markdown {
            // Read + frontmatter-strip here in the caller because the
            // collection pass needs byte-parity with the snapshot walker:
            // use `zfb_content::frontmatter::extract` rather than the
            // local `strip_yaml_frontmatter` helper so the body fed into
            // the compiler is **byte-for-byte identical** to the body
            // `zfb_content::collection::walk_collection` (→ `build_snapshot`)
            // passes. The two helpers differ on leading-newline handling —
            // `strip_yaml_frontmatter` greedily trims `\r`/`\n` after the
            // closing `---`, dropping the blank-line separator between
            // frontmatter and body — which yields a different compiled-JSX
            // content_hash and therefore a different `mdx://…#<hash>`
            // specifier than what the snapshot bakes. The bridge map and
            // the snapshot's `module_specifier` field MUST agree on the
            // hash byte-for-byte; otherwise every `bridge.get(spec)` lookup
            // misses and the page renders the raw-markdown fallback.
            let raw =
                fs::read_to_string(from).with_context(|| format!("read mdx {}", from.display()))?;
            let body = match zfb_frontmatter::extract(from, &raw) {
                Ok(uf) => uf.body.unwrap_or_default(),
                Err(_) => {
                    // Frontmatter parse failures fall back to the local
                    // stripper — the snapshot's `walk_collection` would
                    // surface the same error up its CollectionError path,
                    // so missing this file in the bridge is a no-op (the
                    // snapshot entry is missing too).
                    strip_yaml_frontmatter(&raw).to_string()
                }
            };
            let rel_str = path_to_posix_string(rel);
            let shadow_rel_path = format!("content/{}/{}", collection_name, rel_str);
            // The shared incremental-materialise core does the skip-check /
            // compile / write / store. The closure decides the bridge
            // import after the compile: the collection pass produces an
            // `mdx://…` import (with `idStripSuffix` applied to the slug so
            // the bundler's bridge key matches the snapshot's stripped
            // `module_specifier`), UNLESS the compiled JSX would break
            // esbuild — the defensive skip, which omits the import and
            // refuses to cache so the page falls back to
            // `<pre data-zfb-content-fallback>` and every tick re-warns.
            materialise_mdx_with_skip(
                from,
                &to,
                ctx,
                &mut pipeline,
                &body,
                imports,
                broken_links_out,
                markdown_diagnostics_out,
                cross_file_links_out,
                file_headings_out,
                |compiled| {
                    if jsx_likely_breaks_downstream_parser(&compiled.jsx_source) {
                        eprintln!(
                            "zfb bundler: skipping MDX content bridge for {} — compiled JSX contains bare `{{\\letter}}` expressions that esbuild rejects. The page will render via the <pre data-zfb-content-fallback> shape.",
                            from.display(),
                        );
                        return ImportDecision::DoNotCache;
                    }
                    // Apply `idStripSuffix` to the specifier's slug segment
                    // via the shared `zfb-content` helper so the bundler's
                    // bridge-map key matches the snapshot's
                    // `EntrySnapshot::module_specifier` after stripping —
                    // the snapshot↔bridge byte-for-byte invariant.
                    let specifier = zfb_content::collection::maybe_strip_specifier_suffix(
                        &compiled.specifier,
                        strip_suffix,
                    );
                    ImportDecision::Bridge(ContentImport {
                        specifier,
                        shadow_rel_path,
                    })
                },
            )?;
        } else {
            // Non-MDX source in a content collection: same eager
            // `import.meta.glob(...)` expansion as the page/component pass.
            materialise_source_file(
                from,
                from,
                &to,
                &is_bundle_excluded,
                ctx.copy_mode,
                ctx.writer,
                &ctx.glob_matched_files,
                &ctx.raw_import_edges,
                &ctx.raw_import_aliases,
                &ctx.module_worker_dependencies,
                ctx.project_root,
                &ctx.worker_build_context,
            )?;
        }
    }
    Ok(())
}

/// Heuristic check for compiled JSX that downstream parsers (esbuild's
/// JSX pass, then SWC at render-time) will reject. The emitter that
/// `compile_mdx_to_jsx_module_cached` drives does not yet know about
/// `remark-math` style fences, so LaTeX content from a `$$...$$` block
/// can leak into the JSX as bare expression containers like `{\infty}`
/// or `{-\infty}` — JS does not accept `\letter` as an identifier
/// outside a string literal, so the entire bundle aborts on the first
/// such file.
///
/// The check walks the JSX one byte at a time, tracking string-literal
/// state (`'`, `"`, and template `` ` ``), and flags any `{` that —
/// outside a string — is followed (optionally by a single `-`) by a
/// backslash + ASCII letter. That mirrors the `{\foo}` / `{-\foo}`
/// shape LaTeX leakage produces, while ignoring legitimate JSX such as
/// `{"…\n…"}` (curly opens a JSX expression, immediately enters a
/// string literal whose content can contain anything).
///
/// Outside strings, `\letter` is genuinely unparseable by JS — there is
/// no escape sequence at the expression level — so a true match is a
/// reliable signal that the file would crash esbuild. False positives
/// are bounded: the only consequence of a skip is a fallback
/// `<pre data-zfb-content-fallback>` block on that page, matching the
/// pre-S4e behaviour.
fn jsx_likely_breaks_downstream_parser(jsx: &str) -> bool {
    let bytes = jsx.as_bytes();
    let mut in_string: Option<u8> = None;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];

        // Line and block comments — `\letter` inside a comment is harmless.
        if in_line_comment {
            if c == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if in_block_comment {
            if c == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                in_block_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }

        // String-literal handling (single, double, template).
        if let Some(q) = in_string {
            // Escape: skip the next byte regardless of what it is.
            if c == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if c == q {
                in_string = None;
            }
            i += 1;
            continue;
        }

        // Comment starts.
        if c == b'/' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'/' => {
                    in_line_comment = true;
                    i += 2;
                    continue;
                }
                b'*' => {
                    in_block_comment = true;
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }

        // String literal opener.
        if c == b'"' || c == b'\'' || c == b'`' {
            in_string = Some(c);
            i += 1;
            continue;
        }

        // The pattern of interest: `{\letter` or `{-\letter`, outside
        // strings and outside comments.
        if c == b'{' {
            let mut j = i + 1;
            // optional unary minus before the backslash (matches `{-\foo}`).
            if j < bytes.len() && bytes[j] == b'-' {
                j += 1;
            }
            if j + 1 < bytes.len() && bytes[j] == b'\\' && bytes[j + 1].is_ascii_alphabetic() {
                return true;
            }
        }

        i += 1;
    }
    false
}

/// Materialise a non-MDX source file into the shadow tree, applying the eager
/// `import.meta.glob(...)`, terminal `?raw`, and module-worker URL pre-passes
/// to JS/TS sources first.
///
/// Zero-regression contract: a file that contains none of the three cheap
/// syntax signals (`import.meta.glob`, `?`, or `Worker`) takes the exact
/// copy/symlink path as before — no parse, byte-identical output. A transform
/// hit writes a REAL file so its rewritten body lands in the shadow tree.
/// `file_dir` for a glob anchor is the source file's own directory
/// (`from.parent()`), so matched relative paths line up with what esbuild later
/// resolves through the shadow.
///
/// `is_excluded` is threaded straight through to
/// [`expand_import_meta_glob_with_matches`] (Wave 1 passes a no-op
/// `&|_| false`; Wave 2 / #672 supplies the real `bundle.exclude`
/// predicate).
///
/// `glob_matched_files` collects every target file
/// [`expand_import_meta_glob_with_matches`] matches (issue #1695) — every
/// call site shares the SAME `MaterialiseCtx::glob_matched_files` set, so
/// `bundle_with_session`'s post-pass fixed-point drain sees every match
/// discovered anywhere in this call's materialise passes, not just the ones
/// under whichever source root happened to be walked.
///
/// `copy_mode` forces a real `fs::copy` for the non-transformed fallback
/// instead of a symlink. When esbuild runs without `--preserve-symlinks`
/// (branch 4 — see [`esbuild_will_preserve_symlinks`]) a symlinked source
/// file is canonicalised back to the real tree, so any sibling in-shadow
/// transform (`.module.css` rewrite, expanded glob barrel) it reaches via a
/// relative import would be read from the *original* untransformed file. A
/// real copy keeps the importer physically inside the shadow. The
/// `import.meta.glob` real-write path below is unaffected (it already writes
/// a real file).
///
/// `physical_from` supplies bytes/stat identity. `logical_from` is the
/// project-side importer identity used for relative glob/raw/worker
/// resolution; the two differ for copied package-route overlays and followed
/// symlink dirs.
#[allow(clippy::too_many_arguments)] // Physical/logical identity must stay explicit at every write call.
fn materialise_source_file(
    physical_from: &Path,
    logical_from: &Path,
    to: &Path,
    is_excluded: &dyn Fn(&Path) -> bool,
    copy_mode: bool,
    writer: &ShadowWriter<'_>,
    glob_matched_files: &RefCell<BTreeSet<PathBuf>>,
    raw_import_edges: &RefCell<BTreeSet<RawImportEdge>>,
    raw_import_aliases: &RawImportAliasContext,
    module_worker_dependencies: &RefCell<BTreeSet<ModuleWorkerDependency>>,
    project_root: &Path,
    worker_build_context: &ModuleWorkerBuildContext,
) -> Result<()> {
    // Incremental NON-MDX skip (zfb#1148). In session mode ONLY
    // (passthrough/prod never skips): a plain copy/symlink is a pure
    // function of the file's own bytes, so an unchanged `(mtime, size)`
    // reuses the dest already in the persistent shadow — no read, no copy,
    // no cross-file transform. Skippable iff a dest-keyed entry matches the
    // source `(mtime, size)`, the dest still exists, AND the file does not use
    // a glob/raw/worker transform (their output depends on the live project
    // tree). The `(mtime, size)` stat
    // is the only I/O on the skip path — the win on large ancillary trees
    // (`doc/`, `sub-packages/`, `static/`, …) that this otherwise
    // re-copies/re-symlinks every tick.
    //
    // LIMITATION (#1151, accepted — and intentionally retained here): unlike
    // the collection `.mdx` skip (now SHA-accurate via the snapshot specifier
    // set, see `materialise_mdx_with_skip`), the SOURCE path keeps the
    // `(mtime, size)` key. A content edit preserving both (coarse-mtime FS,
    // `touch -r` / `rsync --times`) is falsely skipped — but a source file
    // has NO snapshot↔bridge invariant, so the worst case is a byte-stale
    // plain copy/symlink, never a broken `<pre>` page. Standard
    // mtime-incrementality; prod never skips. There is no free SHA signal for
    // non-collection files (they aren't in the content snapshot), so hashing
    // here would cost a real per-tick read — do NOT add it without
    // re-evaluating that cost.
    let dest_rel = writer.rel_of(to).ok();
    if writer.in_session() {
        if let Some(dest_rel) = dest_rel.as_ref() {
            if let Some((mtime, size)) = file_stat(physical_from) {
                if let Some(entry) = writer.source_skip_get(dest_rel) {
                    // Never false-reuse: entry must describe THIS source,
                    // its `(mtime, size)` must match, it must not be a glob
                    // file, and the dest must still exist on disk (regular
                    // file OR symlink — `symlink_metadata` does not follow,
                    // so a present link entry counts; its target is `physical_from`,
                    // which we just confirmed exists).
                    let dest_exists = fs::symlink_metadata(to).is_ok();
                    if entry.source == physical_from
                        && entry.mtime == mtime
                        && entry.size == size
                        && !entry.has_glob
                        && !entry.has_raw
                        && !entry.has_worker
                        && dest_exists
                    {
                        // Mark the un-touched dest visited so the prune pass
                        // keeps it (works for both copies and symlinks).
                        writer.record_visited(to).with_context(|| {
                            format!("record visited (source skip) {}", to.display())
                        })?;
                        return Ok(());
                    }
                }
            }
        }
    }

    // ---- Full path: the original materialise + (re)store the skip entry. ----
    if raw_target_matches(&raw_import_edges.borrow(), physical_from, logical_from) {
        if copy_mode {
            writer.copy_if_changed(physical_from, to).with_context(|| {
                format!(
                    "copy terminal raw target {} -> {}",
                    physical_from.display(),
                    to.display()
                )
            })?;
        } else {
            writer
                .symlink_if_absent(physical_from, to)
                .with_context(|| {
                    format!(
                        "symlink terminal raw target {} -> {}",
                        physical_from.display(),
                        to.display()
                    )
                })?;
        }
        store_source_skip_entry(writer, dest_rel, physical_from, false, false, false);
        return Ok(());
    }

    let is_js_like = raw_source_extension(logical_from);
    // `has_glob` is the skip gate: a file that uses `import.meta.glob` must
    // never be skipped (its expansion depends on other files). It is `true`
    // only when the file is JS-like, reads as UTF-8, AND contains the
    // literal substring — exactly the predicate that selects the expand
    // branch below. Binary/asset files (non-UTF-8 or non-JS) are always
    // `false`.
    let mut has_glob = false;
    let mut has_raw = false;
    let mut has_worker = false;
    if is_js_like {
        // Cheap pre-read of the file is only worthwhile when it might contain
        // the macro. `fs::read_to_string` fails on non-UTF-8; in that case
        // (binary masquerading as .js, etc.) fall back to copy.
        if let Ok(source) = fs::read_to_string(physical_from) {
            let has_query_syntax = source.contains('?');
            let has_worker_syntax = source.contains("Worker");
            if source.contains("import.meta.glob") || has_query_syntax || has_worker_syntax {
                let mut expanded = source;
                if expanded.contains("import.meta.glob") {
                    has_glob = true;
                    let file_dir = logical_from.parent().unwrap_or_else(|| Path::new("."));
                    let glob_expansion =
                        expand_import_meta_glob_with_matches(&expanded, file_dir, is_excluded)
                            .with_context(|| {
                                format!("expand import.meta.glob in {}", logical_from.display())
                            })?;
                    // #1695: every matched target feeds the fixed-point
                    // staging queue `bundle_with_session` drains after every
                    // materialise pass — this is the ONLY place a glob's
                    // matched files become known, so a shared accumulator is
                    // the sole way the queue learns about them.
                    glob_matched_files
                        .borrow_mut()
                        .extend(glob_expansion.matched_files);
                    expanded = glob_expansion.expanded_source;
                }

                if has_query_syntax {
                    let raw_expansion = expand_raw_imports_with_aliases(
                        &expanded,
                        logical_from,
                        project_root,
                        raw_import_aliases,
                        is_excluded,
                    )
                    .with_context(|| {
                        format!("expand ?raw imports in {}", logical_from.display())
                    })?;
                    has_raw = !raw_expansion.generated_modules.is_empty();
                    if has_raw {
                        let generated_dir = to.parent().unwrap_or_else(|| Path::new("."));
                        for module in &raw_expansion.generated_modules {
                            let generated_path = generated_dir.join(&module.filename);
                            writer
                                .write_if_changed(&generated_path, module.source.as_bytes())
                                .with_context(|| {
                                    format!(
                                        "write generated raw module {} for {}",
                                        generated_path.display(),
                                        logical_from.display()
                                    )
                                })?;
                        }
                        raw_import_edges
                            .borrow_mut()
                            .extend(raw_expansion.edges.iter().cloned());
                    }
                    expanded = raw_expansion.expanded_source;
                }

                if has_worker_syntax {
                    let worker_rewrite = rewrite_module_worker_urls_with_context(
                        &expanded,
                        logical_from,
                        project_root,
                        worker_build_context,
                    )
                    .with_context(|| {
                        format!("rewrite module-worker URLs in {}", logical_from.display())
                    })?;
                    has_worker = !worker_rewrite.worker_edges.is_empty();
                    if has_worker {
                        let mut dependencies = module_worker_dependencies.borrow_mut();
                        dependencies.extend(worker_rewrite.dependencies.iter().cloned());
                        dependencies.extend(worker_rewrite.config_dependencies.iter().cloned());
                    }
                    expanded = worker_rewrite.expanded_source;
                }

                if has_glob || has_raw || has_worker {
                    // Cross-file transforms are recomputed from the LIVE project
                    // tree every call. `write_if_changed` still suppresses
                    // byte-identical writes while generated-module paths are
                    // marked visited for persistent-shadow pruning.
                    writer
                        .write_if_changed(to, expanded.as_bytes())
                        .with_context(|| format!("write expanded source to {}", to.display()))?;
                    store_source_skip_entry(
                        writer,
                        dest_rel,
                        physical_from,
                        has_glob,
                        has_raw,
                        has_worker,
                    );
                    return Ok(());
                }
            }
        }
    }
    if copy_mode {
        // Force a real copy so esbuild (running WITHOUT --preserve-symlinks)
        // reads this file — and any in-shadow transform it relatively imports
        // — from the shadow tree, not the canonicalised original.
        writer.copy_if_changed(physical_from, to).with_context(|| {
            format!(
                "copy (copy_mode) {} -> {}",
                physical_from.display(),
                to.display()
            )
        })?;
    } else {
        writer
            .symlink_if_absent(physical_from, to)
            .with_context(|| {
                format!(
                    "symlink_or_copy {} -> {}",
                    physical_from.display(),
                    to.display()
                )
            })?;
    }
    store_source_skip_entry(
        writer,
        dest_rel,
        physical_from,
        has_glob,
        has_raw,
        has_worker,
    );
    Ok(())
}

/// Store (or, on a failed source stat, drop) the NON-MDX source/asset
/// skip entry for `from` materialised to dest `dest_rel` (zfb#1148).
/// Session mode only (no-op in passthrough — `dest_rel` is `None` /
/// the writer has no session). `has_glob` records whether this file used
/// `import.meta.glob` so the skip-check can refuse to skip it.
///
/// Also EVICTS any [`ShadowSession::mirror_skip`] entry at the same
/// `dest_rel` (issue #1777 codex-review finding). This function only ever
/// runs after `materialise_source_file` took its full (non-skip) path,
/// which means it just WROTE to `dest_rel` — e.g. expanding a
/// `?raw`/glob/worker sibling that `mirror_sibling_root` had earlier
/// wholesale-mirrored raw into the same slot. Without this eviction, a
/// stale `mirror_skip` entry recorded before this write would still
/// describe the source's `(mtime, size)` as unchanged; if the file later
/// falls OUT of the reachable/preprocessing set on some later tick (so
/// `materialise_source_file` is no longer called for it) while its own
/// bytes stay untouched, the mirror pass's skip check would find that
/// stale entry, see the dest still present, and skip its raw re-copy —
/// permanently preserving this tick's EXPANDED bytes instead of the raw
/// mirror copy a fresh build would produce there. Evicting here is a no-op
/// when no such entry exists (the common case), so this stays cheap.
fn store_source_skip_entry(
    writer: &ShadowWriter<'_>,
    dest_rel: Option<PathBuf>,
    from: &Path,
    has_glob: bool,
    has_raw: bool,
    has_worker: bool,
) {
    if !writer.in_session() {
        return;
    }
    let Some(dest_rel) = dest_rel else { return };
    writer.mirror_skip_remove(&dest_rel);
    match file_stat(from) {
        Some((mtime, size)) => writer.source_skip_store(
            dest_rel,
            SourceSkipEntry {
                source: from.to_path_buf(),
                mtime,
                size,
                has_glob,
                has_raw,
                has_worker,
            },
        ),
        // Could not stat the source — cannot build a sound skip key.
        None => writer.source_skip_remove(&dest_rel),
    }
}

/// Compiled matcher for `BundlerInput::bundle_exclude`.
///
/// Patterns are project-relative gitignore-style globs (e.g.
/// `components/*.stories.tsx`, `components/**/*.stories.tsx`). The match key
/// is a candidate file's path RELATIVE TO `project_root`, in POSIX form, so a
/// pattern written in `zfb.config.ts` lines up with how the user thinks about
/// the tree regardless of host OS path separators.
///
/// `literal_separator(true)` gives gitignore/Vite `*`-vs-`**` semantics: `*`
/// stops at `/`, `**` recurses — the same option `glob_match_relative` uses
/// for `import.meta.glob`, so the two surfaces agree on what a pattern means.
///
/// An empty pattern list compiles to a matcher that never matches, so an
/// unset / empty `bundle.exclude` is byte-identical to a build without the
/// knob (skip nothing) by construction — not by relying on an empty set to
/// "happen to" match nothing.
#[derive(Debug)]
struct BundleExcludeMatcher {
    set: globset::GlobSet,
    /// The workspace first-party root (issue #1668 part 2), set only when the
    /// project is a claimed pnpm-workspace member — i.e. when
    /// [`zfb_types::first_party_root_for`] widens the boundary past
    /// `project_root`. A workspace-sibling file (under this root but outside
    /// `project_root`) then matches patterns against its WORKSPACE-relative
    /// path, the second tier of the exclusion contract. `None` for a
    /// standalone project keeps single-tier project-relative matching — the
    /// exact pre-#1668 behavior.
    workspace_root: Option<PathBuf>,
}

impl BundleExcludeMatcher {
    /// Compile the patterns. Invalid globs surface as an `anyhow::Error` (the
    /// config came from the user; a typo should be a clear build error, not a
    /// silent no-op).
    fn new(patterns: &[String]) -> Result<Self> {
        let mut builder = globset::GlobSetBuilder::new();
        for pat in patterns {
            let glob = globset::GlobBuilder::new(pat)
                .literal_separator(true)
                .build()
                .map_err(|e| anyhow!("zfb bundler: invalid bundle.exclude pattern {pat:?}: {e}"))?;
            builder.add(glob);
        }
        let set = builder
            .build()
            .map_err(|e| anyhow!("zfb bundler: failed to compile bundle.exclude globset: {e}"))?;
        Ok(Self {
            set,
            workspace_root: None,
        })
    }

    /// Enable the issue #1668 part 2 workspace-sibling tier of
    /// [`Self::is_excluded`].
    ///
    /// Records the widened first-party root when — and ONLY when —
    /// `project_root` is a claimed member of an enclosing pnpm workspace
    /// (`first_party_root_for` returns an ancestor). A standalone project
    /// leaves `workspace_root` at `None`, so the sibling tier stays inert and
    /// matching is byte-identical to the single-tier pre-#1668 behavior.
    fn with_workspace_root(mut self, project_root: &Path) -> Self {
        let normalized = normalize_path_lexical(project_root);
        let first_party = zfb_types::first_party_root_for(project_root);
        self.workspace_root = (first_party != normalized).then_some(first_party);
        self
    }

    fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    /// `true` when `abs` (an absolute path on disk) matches any compiled
    /// pattern under the TWO-TIER contract (issue #1668 part 2):
    ///
    /// - **Tier 1 — project files.** A path under `project_root` matches its
    ///   PROJECT-relative POSIX path (the documented, unchanged behavior — an
    ///   existing `src/**` pattern keeps meaning "under the project's `src/`",
    ///   even for a nested workspace host).
    /// - **Tier 2 — workspace siblings.** A path under the workspace
    ///   first-party root but OUTSIDE `project_root` matches its
    ///   WORKSPACE-relative POSIX path. This tier is active only when the
    ///   project is a claimed pnpm-workspace member (see
    ///   [`Self::with_workspace_root`]); a sibling exclusion pattern is
    ///   therefore written workspace-relative (e.g. `lib/shared/secret.ts`),
    ///   and a project-relative pattern like `src/**` can never accidentally
    ///   match a sibling whose workspace-relative path lies outside `src/`.
    ///
    /// A path under NEITHER root cannot be expressed as a relative pattern, so
    /// it is never excluded — matching the user's mental model that
    /// `bundle.exclude` patterns are anchored at the project (and, for
    /// siblings, the workspace) root.
    fn is_excluded(&self, abs: &Path, project_root: &Path) -> bool {
        if self.is_empty() {
            return false;
        }
        // Tier 1 — project-relative.
        if let Ok(rel) = abs.strip_prefix(project_root) {
            return self.rel_posix_matches(rel);
        }
        // Tier 2 — workspace-sibling → workspace-relative.
        if let Some(workspace_root) = self.workspace_root.as_deref() {
            if let Ok(rel) = abs.strip_prefix(workspace_root) {
                return self.rel_posix_matches(rel);
            }
        }
        false
    }

    fn rel_posix_matches(&self, rel: &Path) -> bool {
        let rel_posix = path_to_posix_string(rel);
        if rel_posix.is_empty() {
            return false;
        }
        self.set.is_match(&rel_posix)
    }
}

/// Derive a URL route from a path **relative to** `pages_dir`.
///
/// Returns `None` for non-page files (e.g. an accidental `.txt` inside
/// `pages/`). Recognised page extensions are
/// [`zfb_types::ROUTABLE_PAGE_EXTENSIONS`] (`.tsx`, `.ts`, `.jsx`, `.js`,
/// `.mdx`, `.md`, `.html`) — the same constant `zfb-router`'s `scan_pages`
/// reads, so the two layers cannot silently drift apart again (issue #1742
/// / epic #1990). Files starting with `_` are treated as private (skipped)
/// to match the conventional Next/Astro/Remix behaviour, and the
/// conventional non-page sidecars ([`zfb_types::is_page_sidecar_file`] —
/// `*.d.ts`, `*.test.*`, `*.spec.*`) are skipped too, matching
/// `scan_pages`. Skipping them in only one of the two layers would put the
/// bundler back in the drifted state #1742 describes, from the other side:
/// the sidecar would be bundled as a production route the router never
/// emitted, and fail on its missing default export.
fn derive_route(rel: &Path) -> Option<String> {
    let ext = rel.extension().and_then(|s| s.to_str())?;
    if !zfb_types::ROUTABLE_PAGE_EXTENSIONS.contains(&ext) {
        return None;
    }
    // Skip `_private.tsx` style.
    if rel
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.starts_with('_'))
        .unwrap_or(false)
    {
        return None;
    }
    if zfb_types::is_page_sidecar_file(rel) {
        return None;
    }
    // Strip extension.
    let no_ext = rel.with_extension("");
    let mut parts: Vec<String> = no_ext
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.last().map(|s| s.as_str()) == Some("index") {
        parts.pop();
    }
    if parts.is_empty() {
        return Some("/".to_string());
    }
    Some(format!("/{}", parts.join("/")))
}

/// Rebase the user's `compilerOptions.paths` targets so an alias whose
/// target lives under the project root resolves to the **shadow** copy
/// first (where the in-shadow `import.meta.glob` / `.module.css`
/// transforms live). With an empty `bundle.exclude` the original real-root
/// target is kept as a fallback; with any exclusion policy active the
/// fallback is suppressed (see the shadow-only switch in
/// [`rebase_tsconfig_paths_to_shadow_with_exclusions`]).
///
/// Input targets are ALREADY ABSOLUTE (real-root) — `read_tsconfig_paths`
/// in `crates/zfb/src/commands/build.rs` (and the test helpers that mirror
/// it) absolutise each target against the project root. So we do NOT treat
/// the prefix as relative here; we strip `project_root` off it and re-root
/// the remainder under `shadow`.
///
/// For each target string:
/// - Split a trailing `/*` glob suffix the SAME way
///   [`resolve_tsconfig_path_target`] does (`rsplit_once("/*")`), so the
///   wildcard is preserved verbatim and only the prefix is re-rooted.
/// - **Prefix under `project_root`** → emit `["<shadow>/<rel>[/*]"]`. With
///   an empty `bundle.exclude`, the original real-abs target is appended as
///   a second array element esbuild tries next (a shadow miss — gitignored
///   file, unmirrored top-level file — falls through to the real path). With
///   exclusions active the shadow entry is the SOLE target: absence from the
///   shadow is the exclusion.
/// - **Prefix NOT under `project_root`** (plugin/virtual/external targets,
///   which already point at `<shadow>/.zfb-virtual-*` temp files or an
///   out-of-tree path) → pass through UNCHANGED so the merge step's
///   user-wins-on-collision semantics and the exact-match contract
///   (`bundler_exact_match_resolution`) are preserved.
///
/// Idempotent: a target already rooted under `shadow` (a re-entrant call,
/// or a plugin temp file) is left as-is and never gets a second shadow
/// prefix appended. The emitted array is de-duplicated, and on a
/// case-insensitive FS where the shadow and real targets would collapse to
/// the same path we drop the redundant duplicate.
fn rebase_tsconfig_paths_to_shadow(
    paths: &BTreeMap<String, Vec<String>>,
    project_root: &Path,
    shadow: &Path,
) -> BTreeMap<String, Vec<String>> {
    // Sibling tier inert (first_party/work == project/shadow): the baseline
    // write is overwritten by the plugin-merged rebase in `run_esbuild` for
    // real builds; it only matters for the mock-subprocess path.
    rebase_tsconfig_paths_to_shadow_with_exclusions(
        paths,
        project_root,
        shadow,
        project_root,
        shadow,
        None,
        None,
    )
}

// page-extension-drift-guard: allow — esbuild's extensionless-import RESOLVE
// order (includes css/json, excludes md/mdx/html), not the page allowlist.
const SSR_RESOLVE_EXTENSIONS: [&str; 6] = ["tsx", "ts", "jsx", "js", "css", "json"];

fn has_trailing_path_separator(value: &str) -> bool {
    #[cfg(windows)]
    {
        value.ends_with('/') || value.ends_with('\\')
    }
    #[cfg(not(windows))]
    {
        value.ends_with('/')
    }
}

fn preserve_trailing_path_separator(mut value: String, original: &str) -> String {
    let separator = original.chars().last().filter(|ch| {
        #[cfg(windows)]
        {
            matches!(ch, '/' | '\\')
        }
        #[cfg(not(windows))]
        {
            *ch == '/'
        }
    });
    if let Some(separator) = separator {
        if !has_trailing_path_separator(&value) {
            value.push(separator);
        }
    }
    value
}

fn package_entry_candidate_path(package_dir: &Path, value: &str) -> Option<PathBuf> {
    if value.contains('*') {
        return None;
    }
    let relative = value.strip_prefix("./").unwrap_or(value);
    // esbuild's virtual file-system Join keeps a rooted main-field spelling
    // inside the package. `Path::join` would instead discard `package_dir`.
    #[cfg(windows)]
    let relative = relative.trim_start_matches(['/', '\\']);
    #[cfg(not(windows))]
    let relative = relative.trim_start_matches('/');
    Some(normalize_path_lexical(&package_dir.join(relative)))
}

fn insert_existing_file(candidates: &mut BTreeSet<PathBuf>, candidate: PathBuf) {
    if candidate.is_file() {
        candidates.insert(candidate);
    }
}

/// Collect every existing file esbuild 0.25.12 can reach from a load-as-file
/// probe. The order is intentionally not represented here: it varies for CSS
/// imports and paths inside node_modules. The isolated shadow contains every
/// allowed candidate and esbuild applies the correct contextual order itself.
fn collect_ssr_file_candidates(candidate: &Path, candidates: &mut BTreeSet<PathBuf>) {
    insert_existing_file(candidates, candidate.to_path_buf());
    let Some(name) = candidate.file_name() else {
        return;
    };
    for extension in SSR_RESOLVE_EXTENSIONS {
        let mut path = candidate.to_path_buf();
        let mut appended = name.to_os_string();
        appended.push(format!(".{extension}"));
        path.set_file_name(appended);
        insert_existing_file(candidates, path);
    }

    let rewrites: &[&str] = match candidate
        .extension()
        .and_then(|extension| extension.to_str())
    {
        // page-extension-drift-guard: allow — the compiled-to-source rewrite
        // table (`.js` may really be `.ts` on disk), not the page allowlist.
        Some("js") | Some("jsx") => &["ts", "tsx"],
        Some("mjs") => &["mts"],
        Some("cjs") => &["cts"],
        _ => &[],
    };
    for extension in rewrites {
        insert_existing_file(candidates, candidate.with_extension(extension));
    }
}

fn collect_ssr_directory_index_candidates(target: &Path, candidates: &mut BTreeSet<PathBuf>) {
    if !target.is_dir() {
        return;
    }
    collect_ssr_file_candidates(&target.join("index"), candidates);
}

/// Collect package main-field candidates without choosing a field or
/// extension winner. esbuild ignores `exports` for these absolute tsconfig
/// substitutions. Each effective main field may win under a different
/// contextual extension order, so every reachable file is staged.
fn collect_package_directory_entry_candidates(
    target: &Path,
    effective_main_fields: &[&str],
    candidates: &mut BTreeSet<PathBuf>,
) {
    if !target.is_dir() {
        return;
    }

    let package_json = target.join("package.json");
    let Ok(bytes) = fs::read(&package_json) else {
        return;
    };
    let Ok(package): Result<serde_json::Value, _> = serde_json::from_slice(&bytes) else {
        return;
    };

    for field in effective_main_fields {
        let Some(entry) = package.get(*field).and_then(serde_json::Value::as_str) else {
            continue;
        };
        if let Some(candidate) = package_entry_candidate_path(target, entry) {
            collect_ssr_file_candidates(&candidate, candidates);
            collect_ssr_directory_index_candidates(&candidate, candidates);
        }
    }
}

fn concrete_ssr_target_candidates(
    target: &str,
    effective_main_fields: &[&str],
) -> BTreeSet<PathBuf> {
    let target_path = normalize_path_lexical(Path::new(target));
    let mut candidates = BTreeSet::new();

    if has_trailing_path_separator(target) {
        // For an absolute raw substitution ending in a separator, Go's
        // filepath Dir/Base split gives esbuild `<dir>/<basename>` as the
        // load-as-file spelling before package-main/index probing.
        if let Some(basename) = target_path.file_name() {
            collect_ssr_file_candidates(&target_path.join(basename), &mut candidates);
        }
    } else {
        collect_ssr_file_candidates(&target_path, &mut candidates);
    }
    collect_package_directory_entry_candidates(
        &target_path,
        effective_main_fields,
        &mut candidates,
    );
    collect_ssr_directory_index_candidates(&target_path, &mut candidates);
    candidates
}

fn project_path_is_inside_node_modules(path: &Path, project_root: &Path) -> bool {
    path.strip_prefix(project_root)
        .is_ok_and(path_is_inside_node_modules)
}

fn path_is_inside_node_modules(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == std::ffi::OsStr::new("node_modules"))
}

/// Map a logical path to its physical location in the re-rooted SSR shadow
/// (issue #1668 part 1). Tiers, in order:
///
/// - under `project_root`, inside `node_modules` → the isolation root (or the
///   `.zfb-exact-isolation` slot in the project mirror).
/// - under `project_root` otherwise → the project mirror (`shadow`).
/// - under `first_party_root` but OUTSIDE `project_root` (a workspace sibling)
///   → its workspace-relative slot in the work mirror (`work_root/<rel>`).
///   Issue #1668 part 2 STAGES such siblings through this tier (the
///   materialise loop, the tsconfig rebase, and the plugin-alias remap all
///   route sibling targets here), so their in-shadow rewrites land.
/// - anything else → the real path (the unchanged live-tree escape).
///
/// Callers that only ever pass under-`project_root` paths may pass
/// `first_party_root == project_root` (with any `work_root`) to make the
/// first-party tier unreachable — its behavior is then byte-identical to the
/// pre-#1668 two-tier mapping.
pub(crate) fn shadow_path_for_project_path(
    path: &Path,
    project_root: &Path,
    first_party_root: &Path,
    shadow: &Path,
    work_root: &Path,
    node_modules_isolation_root: Option<&Path>,
) -> PathBuf {
    if let Ok(relative) = path.strip_prefix(project_root) {
        return if project_path_is_inside_node_modules(path, project_root) {
            match node_modules_isolation_root {
                Some(isolation_root) => isolation_root.join(relative),
                None => shadow.join(".zfb-exact-isolation").join(relative),
            }
        } else if relative.as_os_str().is_empty() {
            shadow.to_path_buf()
        } else {
            shadow.join(relative)
        };
    }
    if first_party_root != project_root {
        if let Ok(relative) = path.strip_prefix(first_party_root) {
            if !relative.as_os_str().is_empty() {
                return work_root.join(relative);
            }
        }
    }
    path.to_path_buf()
}

fn node_modules_package_root(path: &Path, project_root: &Path) -> Option<PathBuf> {
    let relative = path.strip_prefix(project_root).ok()?;
    let components = relative.components().collect::<Vec<_>>();
    let node_modules_index = components
        .iter()
        .rposition(|component| component.as_os_str() == std::ffi::OsStr::new("node_modules"))?;
    let first_package = components.get(node_modules_index + 1)?;
    let package_end = if first_package.as_os_str().to_string_lossy().starts_with('@') {
        node_modules_index + 3
    } else {
        node_modules_index + 2
    };
    if components.len() < package_end {
        return None;
    }

    let mut root = project_root.to_path_buf();
    for component in &components[..package_end] {
        root.push(component.as_os_str());
    }
    Some(root)
}

fn bare_package_name(specifier: &str) -> Option<String> {
    let specifier = specifier.split(['?', '#']).next().unwrap_or(specifier);
    if specifier.is_empty()
        || specifier.starts_with('.')
        || specifier.starts_with('/')
        || specifier.starts_with("node:")
        || specifier.contains("://")
    {
        return None;
    }
    let mut components = specifier.split('/');
    let first = components.next()?;
    if first.starts_with('@') {
        Some(format!("{first}/{}", components.next()?))
    } else {
        Some(first.to_string())
    }
}

fn node_modules_package_name(package_root: &Path, project_root: &Path) -> Option<String> {
    let relative = package_root.strip_prefix(project_root).ok()?;
    let components = relative.components().collect::<Vec<_>>();
    let node_modules_index = components
        .iter()
        .rposition(|component| component.as_os_str() == std::ffi::OsStr::new("node_modules"))?;
    let first = components
        .get(node_modules_index + 1)?
        .as_os_str()
        .to_str()?;
    if first.starts_with('@') {
        let second = components
            .get(node_modules_index + 2)?
            .as_os_str()
            .to_str()?;
        Some(format!("{first}/{second}"))
    } else {
        Some(first.to_string())
    }
}

/// Resolve a relative/absolute source import that explicitly enters the
/// project's logical `node_modules` tree. Package-route overlay modules use
/// this shape so their entrypoint is loaded from the staged dependency view
/// without relying on package `exports` to expose the private route source.
fn node_modules_dependency_from_path_specifier(
    specifier: &str,
    importer: &Path,
    project_root: &Path,
) -> Option<(String, PathBuf)> {
    let specifier = specifier.split(['?', '#']).next().unwrap_or(specifier);
    let specifier_path = Path::new(specifier);
    let target = if specifier_path.is_absolute() {
        normalize_path_lexical(specifier_path)
    } else if specifier.starts_with("./") || specifier.starts_with("../") {
        normalize_path_lexical(&importer.parent()?.join(specifier_path))
    } else {
        return None;
    };
    let package_root = node_modules_package_root(&target, project_root)?;
    if !package_root.is_dir() {
        return None;
    }
    let package_name = node_modules_package_name(&package_root, project_root)?;
    Some((package_name, package_root))
}

fn resolve_installed_package_dir(
    importer: &Path,
    package_name: &str,
    project_root: &Path,
) -> Option<PathBuf> {
    let mut directory = importer.parent()?;
    while directory.starts_with(project_root) {
        let candidate = directory.join("node_modules").join(package_name);
        if candidate.is_dir() {
            return Some(normalize_path_lexical(&candidate));
        }
        if directory == project_root {
            break;
        }
        directory = directory.parent()?;
    }
    None
}

fn package_source_identity(path: &Path) -> PathBuf {
    path.canonicalize()
        .unwrap_or_else(|_| normalize_path_lexical(path))
}

/// Classify an installed package before it is copied into the staged
/// dependency view. Ordinary npm/pnpm packages retain their historical
/// behavior. A symlink which canonicalises directly into first-party source,
/// however, is a workspace-package claim and is accepted only when all three
/// authorities agree: the installed owner name, pnpm workspace membership,
/// and the nearest importing package's dependency manifest.
fn workspace_package_source_is_eligible(
    manifest_importer: &Path,
    package_name: &str,
    source_dependency: &Path,
    project_root: &Path,
) -> bool {
    let first_party_root = zfb_types::first_party_root_for(project_root);
    if first_party_root == project_root {
        return true;
    }
    let Ok(canonical_source) = source_dependency.canonicalize() else {
        return false;
    };

    let importer_package = nearest_package_root(manifest_importer, &first_party_root)
        .and_then(|root| fs::read(root.join("package.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let declared_value = importer_package.as_ref().and_then(|package| {
        ["dependencies", "optionalDependencies", "peerDependencies"]
            .iter()
            .find_map(|section| {
                package
                    .get(section)
                    .and_then(serde_json::Value::as_object)
                    .and_then(|dependencies| dependencies.get(package_name))
            })
    });
    let declared_workspace = declared_value
        .and_then(serde_json::Value::as_str)
        .is_some_and(|requirement| requirement.starts_with("workspace:"));

    // A normal npm/pnpm package remains under a node_modules component after
    // canonicalisation (including pnpm's content-addressable store). Preserve
    // its historical staging unless the importer explicitly says workspace:*,
    // in which case an outside-workspace target is an invalid installed link.
    if path_is_inside_node_modules(&canonical_source) && !declared_workspace {
        return true;
    }
    let Some(_logical_source) =
        canonical_workspace_package_logical_path(&canonical_source, project_root)
    else {
        return false;
    };

    let Ok(package_bytes) = fs::read(canonical_source.join("package.json")) else {
        return false;
    };
    let Ok(package): Result<serde_json::Value, _> = serde_json::from_slice(&package_bytes) else {
        return false;
    };
    package.get("name").and_then(serde_json::Value::as_str) == Some(package_name)
        && declared_value.is_some()
}

/// Convert a canonical installed-link target back to the workspace's logical
/// spelling before applying pnpm membership. This is load-bearing on macOS,
/// where temp paths commonly canonicalise from `/var/...` to `/private/var/...`.
fn canonical_workspace_package_logical_path(
    canonical_source: &Path,
    project_root: &Path,
) -> Option<PathBuf> {
    let first_party_root = zfb_types::first_party_root_for(project_root);
    if first_party_root == project_root {
        return None;
    }
    let canonical_first_party_root = first_party_root.canonicalize().ok()?;
    let relative = canonical_source
        .strip_prefix(&canonical_first_party_root)
        .ok()?;
    let logical_source = normalize_path_lexical(&first_party_root.join(relative));
    zfb_types::first_party::workspace_claims_package(project_root, &logical_source)
        .then_some(logical_source)
}

fn nearest_package_root(importer: &Path, boundary: &Path) -> Option<PathBuf> {
    let canonical_boundary = boundary.canonicalize().ok();
    let boundary = canonical_boundary
        .as_deref()
        .filter(|canonical| importer.starts_with(canonical))
        .unwrap_or(boundary);
    let mut directory = if importer.is_dir() {
        importer
    } else {
        importer.parent()?
    };
    while directory.starts_with(boundary) {
        if directory.join("package.json").is_file() {
            return Some(directory.to_path_buf());
        }
        if directory == boundary {
            break;
        }
        directory = directory.parent()?;
    }
    None
}

/// Return whether `package_name` already has an equivalent staged package at
/// one of the locations Node resolution will search from `importer`.
///
/// pnpm represents dependency cycles with symlinks in each package's physical
/// `node_modules`. Following that graph while constructing the shadow view can
/// otherwise turn `a -> b -> a` into an unbounded sequence of logical paths:
/// `a/node_modules/b/node_modules/a/...`. Once the same physical package is
/// already staged at a reachable ancestor, another nested copy is redundant.
/// Comparing the canonical source as well as the logical search location keeps
/// distinct versions and genuinely non-hoisted placements intact.
fn staged_equivalent_dependency_is_reachable(
    importer: &Path,
    package_name: &str,
    source_dependency: &Path,
    project_root: &Path,
    staged_package_sources: &BTreeMap<PathBuf, PathBuf>,
) -> bool {
    let source_identity = package_source_identity(source_dependency);
    let Some(mut directory) = importer.parent() else {
        return false;
    };
    while directory.starts_with(project_root) {
        let candidate = normalize_path_lexical(&directory.join("node_modules").join(package_name));
        if staged_package_sources
            .get(&candidate)
            .is_some_and(|staged_source| *staged_source == source_identity)
        {
            return true;
        }
        if directory == project_root {
            break;
        }
        let Some(parent) = directory.parent() else {
            break;
        };
        directory = parent;
    }
    false
}

/// Whether `specifier` is claimed by an alias key (a tsconfig `paths` key, a
/// plugin alias, or a plugin virtual module). Wildcard keys (`foo/*`) match any
/// specifier with the surrounding prefix/suffix; non-wildcard keys match exactly.
fn specifier_matches_alias_key(specifier: &str, key: &str) -> bool {
    match key.find('*') {
        Some(star) => {
            let prefix = &key[..star];
            let suffix = &key[star + 1..];
            // A universal catch-all (`*`) is a fallback alias — node_modules
            // resolution is still expected under it — so it must NOT suppress
            // staging. Only prefix/suffix-anchored wildcards claim a specifier.
            (!prefix.is_empty() || !suffix.is_empty())
                && specifier.len() >= prefix.len() + suffix.len()
                && specifier.starts_with(prefix)
                && specifier.ends_with(suffix)
        }
        None => specifier == key,
    }
}

/// Whether `specifier` is resolved by the alias system rather than by
/// `node_modules`. Such a specifier must NOT seed `node_modules` staging: the
/// alias machinery owns its resolution, and when its target is excluded the
/// correct outcome is a hard resolution failure — NOT a silent fall-back to a
/// coincidentally same-named installed package (#1557, regression guarded by
/// `excluded_exact_bare_alias_cannot_fall_back_to_same_named_package`). Without
/// this the #1645 source-graph seed would re-stage that fall-back package.
fn specifier_is_alias_claimed(specifier: &str, alias_claimed_keys: &BTreeSet<String>) -> bool {
    alias_claimed_keys
        .iter()
        .any(|key| specifier_matches_alias_key(specifier, key))
}

/// Whether staging `package_name` at the project-level `node_modules` would
/// expose a fallback for any alias-owned specifier in that package namespace.
/// A universal `*` remains a fallback mapping, but exact subpaths and bounded
/// wildcards claim the package containing them as well as exact package keys.
fn package_namespace_is_alias_claimed(
    package_name: &str,
    alias_claimed_keys: &BTreeSet<String>,
) -> bool {
    alias_claimed_keys.iter().any(|key| {
        if key == "*" {
            return false;
        }
        if specifier_matches_alias_key(package_name, key) {
            return true;
        }
        let Some(star) = key.find('*') else {
            return bare_package_name(key).as_deref() == Some(package_name);
        };
        let prefix = &key[..star];
        if prefix.is_empty() {
            // A suffix-only wildcard can claim a subpath beneath any package.
            return true;
        }
        let representative = key.replacen('*', "__zfb_alias_probe__", 1);
        bare_package_name(&representative).as_deref() == Some(package_name)
    })
}

/// Whether the WHOLE package `package_name` is marked external. esbuild resolves
/// nothing for a fully-external package, so staging + closure-walking + copying
/// its tree into the shadow is pure waste (issue #1645). An external entry uses
/// esbuild's own `foo`/`foo/*`/`@scope/*` matching (reused via
/// `specifier_matches_alias_key`); a subpath-only external (`foo/bar`) does NOT
/// make the whole `foo` package external, so it correctly does not match here.
fn package_is_external(package_name: &str, external_specifiers: &[String]) -> bool {
    external_specifiers
        .iter()
        .any(|entry| specifier_matches_alias_key(package_name, entry))
}

/// Apply the exclusion filter and #1644's canonical/reachable cycle guard to a
/// single resolved dependency candidate, record it in the staged view, and
/// enqueue it for closure walking. Shared by the synthetic-entry seed, the
/// file-seed loop, and the package closure walk so the three cannot drift — the
/// drift between #1644's guard and an incomplete seed set was itself issue #1645.
#[allow(clippy::too_many_arguments)]
fn stage_dependency_candidate(
    importer: &Path,
    manifest_importer: &Path,
    package_name: &str,
    logical_dependency: PathBuf,
    source_dependency: PathBuf,
    project_root: &Path,
    bundle_exclude: &BundleExcludeMatcher,
    staging_dirs: &mut BTreeSet<PathBuf>,
    staging_alias_dirs: &mut BTreeMap<PathBuf, PathBuf>,
    staged_package_sources: &mut BTreeMap<PathBuf, PathBuf>,
    visited: &BTreeSet<PathBuf>,
    pending: &mut BTreeMap<PathBuf, PathBuf>,
) {
    if bundle_exclude.is_excluded(&logical_dependency, project_root) {
        return;
    }
    if !workspace_package_source_is_eligible(
        manifest_importer,
        package_name,
        &source_dependency,
        project_root,
    ) {
        return;
    }
    if staged_equivalent_dependency_is_reachable(
        importer,
        package_name,
        &source_dependency,
        project_root,
        staged_package_sources,
    ) {
        return;
    }
    if logical_dependency == source_dependency {
        staging_dirs.insert(logical_dependency.clone());
    } else {
        staging_alias_dirs.insert(logical_dependency.clone(), source_dependency.clone());
    }
    if !visited.contains(&logical_dependency) {
        staged_package_sources.insert(
            logical_dependency.clone(),
            package_source_identity(&source_dependency),
        );
        pending.insert(logical_dependency, source_dependency);
    }
}

/// Collect staged-dependency seed files from the project's materialized module
/// graph — every page/component/layout/content source (and extra top-level
/// source dirs) esbuild will bundle. With `bundle.exclude` active the live
/// `<shadow>/node_modules` symlink is never created, so a bare import reachable
/// from these sources resolves ONLY if its package was staged as a real copy;
/// seeding the sources here lets the closure discover those packages (issue
/// #1645). Every source file is itself a seed, so no transitive first-party
/// discovery is needed. Explicitly supplied roots may live outside the project
/// (notably the build package-route overlay); their files still seed project
/// dependencies, while `node_modules`, infra dirs, and excluded paths are
/// skipped.
fn collect_project_source_module_graph_seed_files(
    roots: &[PathBuf],
    project_root: &Path,
    bundle_exclude: &BundleExcludeMatcher,
    logical_source_roots: &[(PathBuf, PathBuf)],
    out: &mut BTreeSet<PathBuf>,
    logical_importers: &mut BTreeMap<PathBuf, PathBuf>,
) {
    let is_seed_source = |path: &Path| {
        raw_source_extension(path)
            || path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("css"))
    };
    for root in roots {
        for entry in WalkDir::new(root)
            .follow_links(true)
            .into_iter()
            .filter_entry(|entry| !is_pruned_infra_dir(entry))
            .filter_map(std::result::Result::ok)
        {
            let path = entry.path();
            if !entry.file_type().is_file() || !is_seed_source(path) {
                continue;
            }
            if path_is_inside_node_modules(path) || bundle_exclude.is_excluded(path, project_root) {
                continue;
            }
            out.insert(path.to_path_buf());
            if !path.starts_with(project_root) {
                if let Some(logical_importer) =
                    logical_source_roots
                        .iter()
                        .find_map(|(physical_root, logical_root)| {
                            path.strip_prefix(physical_root)
                                .ok()
                                .map(|relative| logical_root.join(relative))
                        })
                {
                    logical_importers.insert(path.to_path_buf(), logical_importer);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn extend_node_modules_dependency_staging(
    project_root: &Path,
    node_modules_dir: Option<&Path>,
    bundle_exclude: &BundleExcludeMatcher,
    resolve_from_canonical_package: bool,
    root_entry_dependency_seed_files: &BTreeSet<PathBuf>,
    root_entry_dependency_logical_importers: &BTreeMap<PathBuf, PathBuf>,
    synthetic_entry_import_specifiers: &BTreeSet<String>,
    alias_claimed_specifier_keys: &BTreeSet<String>,
    external_specifiers: &[String],
    staging_dirs: &mut BTreeSet<PathBuf>,
    staging_alias_dirs: &mut BTreeMap<PathBuf, PathBuf>,
) {
    let canonical_project_root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let first_party_root = zfb_types::first_party_root_for(project_root);
    let workspace_node_modules_dir = (first_party_root != project_root)
        .then(|| first_party_root.join("node_modules"))
        .filter(|candidate| candidate.is_dir());
    // Preserve package-local precedence, then consult the same workspace
    // install root that the WORK mirror exposes to esbuild. This second tier
    // is what makes a nested host's hoisted workspace:* link stageable even
    // when it has no project-local node_modules entry.
    let resolve_configured_package = |package_name: &str| {
        resolve_vendored_package_dir(node_modules_dir, package_name).or_else(|| {
            resolve_vendored_package_dir(workspace_node_modules_dir.as_deref(), package_name)
        })
    };
    // Staged-dependency-view contract seed set: every package root already
    // staged as an exact target whose own bare dependencies must therefore be
    // present in the isolated view too. Two seed kinds:
    //   1. `node_modules` install roots (the original behaviour).
    //   2. FIRST-PARTY project package roots — a package.json under
    //      `project_root` but outside any `node_modules` (staged by
    //      `plan_concrete_target_staging`'s first-party branch). Without this
    //      seed a first-party package's bare deps were never closure-walked.
    let initial = staging_dirs
        .iter()
        .filter_map(|path| {
            node_modules_package_root(path, project_root)
                .or_else(|| first_party_staged_package_root(path, project_root))
        })
        .collect::<BTreeSet<_>>();
    let mut pending = initial
        .iter()
        .map(|path| (path.clone(), path.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut staged_package_sources = initial
        .iter()
        .map(|path| (path.clone(), package_source_identity(path)))
        .collect::<BTreeMap<_, _>>();
    let mut visited = BTreeSet::new();

    // Packages the generated `entry.mjs` and hydration shim import but which
    // appear in no project source file (issue #1645). Resolve them from a
    // synthetic project-root importer so the closure stages them like any other
    // bare dependency; subpath specifiers collapse to their owning package via
    // `bare_package_name`, so `@takazudo/zfb-runtime/server` also stages the
    // `/client-router` subpath the islands runtime injects.
    let synthetic_importer = project_root.join(SHADOW_ENTRY_FILENAME);
    for specifier in synthetic_entry_import_specifiers {
        // Even a generated import must not resurrect an excluded alias target's
        // same-named installed package (#1557), and an externalized framework
        // is never staged (#1645).
        if specifier_is_alias_claimed(specifier, alias_claimed_specifier_keys) {
            continue;
        }
        let Some(package_name) = bare_package_name(specifier) else {
            continue;
        };
        if package_is_external(&package_name, external_specifiers) {
            continue;
        }
        let (logical_dependency, source_dependency) = if let Some(dependency) =
            resolve_installed_package_dir(&synthetic_importer, &package_name, project_root)
        {
            (dependency.clone(), dependency)
        } else if let Some(dependency) = resolve_configured_package(&package_name) {
            (
                project_root.join("node_modules").join(&package_name),
                dependency,
            )
        } else {
            continue;
        };
        stage_dependency_candidate(
            &synthetic_importer,
            &synthetic_importer,
            &package_name,
            logical_dependency,
            source_dependency,
            project_root,
            bundle_exclude,
            staging_dirs,
            staging_alias_dirs,
            &mut staged_package_sources,
            &visited,
            &mut pending,
        );
    }

    for seed in root_entry_dependency_seed_files {
        if !seed.is_file() || bundle_exclude.is_excluded(seed, project_root) {
            continue;
        }
        let Ok(specifiers) = collect_runtime_import_specifiers_from_file(seed) else {
            // An unused invalid alternate must remain esbuild-contextual.
            continue;
        };
        // Overlay pages are physical copies under a per-build temp directory,
        // but their imports retain project-page semantics. Package-route
        // overlays have an exact logical `project/pages/<rel>` importer so
        // their staged relative node_modules import resolves correctly. Other
        // out-of-root sources keep the synthetic project entry fallback used
        // for ordinary bare dependency discovery.
        let logical_importer = if seed.starts_with(project_root) {
            seed.as_path()
        } else if let Some(importer) = root_entry_dependency_logical_importers.get(seed) {
            importer.as_path()
        } else {
            synthetic_importer.as_path()
        };
        for specifier in &specifiers {
            // Alias-claimed specifiers are resolved by the alias system; staging
            // a same-named installed package would resurrect the excluded-target
            // fall-back #1557 forbids (issue #1645).
            if specifier_is_alias_claimed(specifier, alias_claimed_specifier_keys) {
                continue;
            }
            let (package_name, logical_dependency, source_dependency) =
                if let Some(package_name) = bare_package_name(specifier) {
                    if package_is_external(&package_name, external_specifiers) {
                        continue;
                    }
                    let (logical_dependency, source_dependency) = if let Some(dependency) =
                        resolve_installed_package_dir(logical_importer, &package_name, project_root)
                    {
                        (dependency.clone(), dependency)
                    } else if let Some(dependency) = resolve_configured_package(&package_name) {
                        (
                            project_root.join("node_modules").join(&package_name),
                            dependency,
                        )
                    } else {
                        continue;
                    };
                    (package_name, logical_dependency, source_dependency)
                } else if let Some((package_name, dependency)) =
                    node_modules_dependency_from_path_specifier(
                        specifier,
                        logical_importer,
                        project_root,
                    )
                {
                    (package_name, dependency.clone(), dependency)
                } else {
                    continue;
                };
            if bundle_exclude.is_empty()
                && !source_dependency.canonicalize().is_ok_and(|canonical| {
                    canonical_workspace_package_logical_path(&canonical, project_root).is_some()
                })
            {
                continue;
            }
            stage_dependency_candidate(
                logical_importer,
                logical_importer,
                &package_name,
                logical_dependency,
                source_dependency,
                project_root,
                bundle_exclude,
                staging_dirs,
                staging_alias_dirs,
                &mut staged_package_sources,
                &visited,
                &mut pending,
            );
        }
    }

    while let Some((logical_root, source_root)) = pending.pop_first() {
        if !visited.insert(logical_root.clone()) || !source_root.is_dir() {
            continue;
        }
        let Ok(physical_root) = source_root.canonicalize() else {
            continue;
        };
        let source_relative = source_root
            .strip_prefix(project_root)
            .or_else(|_| source_root.strip_prefix(&canonical_project_root))
            .ok();
        let expected_physical = source_relative
            .map(|relative| canonical_project_root.join(relative))
            .unwrap_or_else(|| source_root.clone());
        let package_was_symlinked = logical_root != source_root
            || normalize_path_lexical(&expected_physical) != physical_root;
        let mut importers = Vec::new();
        for entry in WalkDir::new(&physical_root)
            .follow_links(true)
            .into_iter()
            .filter_entry(|entry| {
                entry.depth() == 0
                    || !entry.file_type().is_dir()
                    || !matches!(
                        entry.file_name().to_string_lossy().as_ref(),
                        "node_modules" | ".git"
                    )
            })
            .filter_map(std::result::Result::ok)
        {
            let path = entry.path();
            let dependency_source = raw_source_extension(path)
                || path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("css"));
            if !entry.file_type().is_file() || !dependency_source {
                continue;
            }
            let Ok(specifiers) = collect_runtime_import_specifiers_from_file(path) else {
                // An unused invalid alternative must remain esbuild-contextual.
                continue;
            };
            let Ok(relative) = path.strip_prefix(&physical_root) else {
                continue;
            };
            importers.push((logical_root.join(relative), path.to_path_buf(), specifiers));
        }
        let external_imports = package_external_import_names(&physical_root);
        if !external_imports.is_empty() {
            importers.push((
                logical_root.join("package.json"),
                physical_root.join("package.json"),
                external_imports,
            ));
        }

        for (logical_importer, physical_importer, specifiers) in importers {
            for package_name in specifiers
                .iter()
                .filter_map(|specifier| bare_package_name(specifier))
            {
                // A fully-external dependency is never resolved by esbuild, so
                // there is nothing to stage or closure-walk (#1645).
                if package_is_external(&package_name, external_specifiers) {
                    continue;
                }
                let canonical_dependency = (resolve_from_canonical_package
                    && package_was_symlinked)
                    .then(|| {
                        resolve_installed_package_dir(
                            &physical_importer,
                            &package_name,
                            &canonical_project_root,
                        )
                    })
                    .flatten();
                let (mut logical_dependency, source_dependency) = if let Some(dependency) =
                    canonical_dependency
                {
                    (
                        logical_root.join("node_modules").join(&package_name),
                        dependency,
                    )
                } else if let Some(dependency) =
                    resolve_installed_package_dir(&logical_importer, &package_name, project_root)
                {
                    (dependency.clone(), dependency)
                } else if let Some(dependency) = resolve_configured_package(&package_name) {
                    // The configured EXTERNAL vendored node_modules
                    // (`BundlerInput::node_modules_dir`) is a closure source too:
                    // a bare dep that lives only in the vendor tree (outside
                    // `project_root`) is aliased to a logical node_modules path so
                    // materialisation routes the staged copy through the isolation
                    // root, keeping it out of the live `<shadow>/node_modules`
                    // symlink.
                    (
                        logical_root.join("node_modules").join(&package_name),
                        dependency,
                    )
                } else {
                    continue;
                };
                if bundle_exclude.is_excluded(&logical_dependency, project_root) {
                    continue;
                }
                // A transitive package may be physically hoisted even though
                // its importer genuinely depends on it. If its namespace is
                // alias-owned, copying that hoisted location into the shadow
                // would let an excluded first-party alias fall back to the npm
                // package. Keep the physical source, but expose it only beneath
                // this importer package so ordinary Node resolution still finds
                // it for the dependency that requested it (#1646).
                if !bundle_exclude.is_empty()
                    && package_namespace_is_alias_claimed(
                        &package_name,
                        alias_claimed_specifier_keys,
                    )
                {
                    logical_dependency = logical_root.join("node_modules").join(&package_name);
                }
                stage_dependency_candidate(
                    &logical_importer,
                    &physical_importer,
                    &package_name,
                    logical_dependency,
                    source_dependency,
                    project_root,
                    bundle_exclude,
                    staging_dirs,
                    staging_alias_dirs,
                    &mut staged_package_sources,
                    &visited,
                    &mut pending,
                );
            }
        }
    }
}

/// A first-party package directory staged as an exact target: its `package.json`
/// lives under `project_root`, outside any `node_modules`, and is not the
/// project root itself (the root is never treated as a closure-walkable package —
/// that would drag the whole project source in). Seeding it lets the closure
/// walk discover the package's own bare dependencies, which the staged
/// dependency-view contract requires present even though a first-party package
/// never came from a `node_modules` install path.
fn first_party_staged_package_root(path: &Path, project_root: &Path) -> Option<PathBuf> {
    if project_path_is_inside_node_modules(path, project_root) {
        return None;
    }
    let root = containing_project_package_root(path, project_root)?;
    (root != project_root).then_some(root)
}

/// A concrete exact target that is a file under the project root but not under
/// any first-party package below the root. These root-level entry files are
/// closure-walk roots for dependency staging, but the project root itself is
/// still not treated as a package root.
fn root_level_staged_entry_file(path: &Path, project_root: &Path) -> Option<PathBuf> {
    if !path.is_file() || !path.starts_with(project_root) {
        return None;
    }
    if project_path_is_inside_node_modules(path, project_root) {
        return None;
    }
    match containing_project_package_root(path, project_root) {
        Some(package_root) if package_root != project_root => None,
        _ => Some(path.to_path_buf()),
    }
}

/// Resolve a bare package inside the configured external vendored node_modules
/// (`BundlerInput::node_modules_dir`). This stages the whole package directory as
/// a candidate; esbuild remains the sole resolver, choosing inside the staged
/// copy. Returns `None` when no vendor dir is configured or the package is absent.
fn resolve_vendored_package_dir(
    node_modules_dir: Option<&Path>,
    package_name: &str,
) -> Option<PathBuf> {
    let candidate = node_modules_dir?.join(package_name);
    candidate
        .is_dir()
        .then(|| normalize_path_lexical(&candidate))
}

fn containing_project_package_root(path: &Path, project_root: &Path) -> Option<PathBuf> {
    let mut directory = if path.is_dir() { path } else { path.parent()? };
    while directory.starts_with(project_root) {
        if directory.join("package.json").is_file() {
            return Some(directory.to_path_buf());
        }
        if directory == project_root {
            break;
        }
        directory = directory.parent()?;
    }
    None
}

fn collect_package_import_target_values(value: &serde_json::Value, targets: &mut Vec<String>) {
    match value {
        serde_json::Value::String(target) => targets.push(target.clone()),
        serde_json::Value::Array(values) => {
            for value in values {
                collect_package_import_target_values(value, targets);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                collect_package_import_target_values(value, targets);
            }
        }
        _ => {}
    }
}

fn package_external_import_names(package_root: &Path) -> Vec<String> {
    let Ok(bytes) = fs::read(package_root.join("package.json")) else {
        return Vec::new();
    };
    let Ok(package): Result<serde_json::Value, _> = serde_json::from_slice(&bytes) else {
        return Vec::new();
    };
    let Some(imports) = package.get("imports") else {
        return Vec::new();
    };
    let mut targets = Vec::new();
    collect_package_import_target_values(imports, &mut targets);
    targets
        .into_iter()
        .filter(|target| !target.starts_with("./"))
        .filter_map(|target| bare_package_name(&target))
        .collect()
}

fn collect_package_scope_staging_files(package_root: &Path, files: &mut BTreeSet<PathBuf>) {
    let package_json = package_root.join("package.json");
    let Ok(bytes) = fs::read(&package_json) else {
        return;
    };
    files.insert(package_json);
    let Ok(package): Result<serde_json::Value, _> = serde_json::from_slice(&bytes) else {
        return;
    };
    let Some(imports) = package.get("imports") else {
        return;
    };
    let mut targets = Vec::new();
    collect_package_import_target_values(imports, &mut targets);
    for target in targets {
        if !target.starts_with("./") {
            continue;
        }
        if target.contains('*') {
            let Ok(glob) = globset::GlobBuilder::new(target.trim_start_matches("./"))
                // Node package-import pattern captures may span path separators.
                .literal_separator(false)
                .build()
            else {
                continue;
            };
            let matcher = glob.compile_matcher();
            for entry in WalkDir::new(package_root)
                .follow_links(true)
                .into_iter()
                .filter_entry(|entry| {
                    entry.depth() == 0
                        || !entry.file_type().is_dir()
                        || !matches!(
                            entry.file_name().to_string_lossy().as_ref(),
                            "node_modules" | ".git"
                        )
                })
                .filter_map(std::result::Result::ok)
            {
                let path = entry.path();
                if entry.file_type().is_file()
                    && path
                        .strip_prefix(package_root)
                        .is_ok_and(|relative| matcher.is_match(path_to_posix_string(relative)))
                {
                    files.insert(path.to_path_buf());
                }
            }
            continue;
        }
        let Some(candidate) = package_entry_candidate_path(package_root, &target) else {
            continue;
        };
        collect_ssr_file_candidates(&candidate, files);
        collect_ssr_directory_index_candidates(&candidate, files);
    }
}

/// Stage the complete allowed candidate set for one concrete (non-wildcard)
/// alias target into the shadow, so esbuild can resolve inside the shadow with
/// no live-tree fallback. Rust only ever collects the candidate SET here — it
/// never predicts which one esbuild picks.
///
/// `force_stage` gates the whole pass: concrete user tsconfig mappings only
/// need explicit staging once `bundle.exclude` makes the live-real fallback
/// unsafe (caller passes `!bundle_exclude.is_empty()`), while plugin aliases
/// are always staged (they may point at hidden/unwalked files). When
/// `force_stage` is false this is a no-op, keeping the empty-`bundle.exclude`
/// path byte-identical to a build without the knob.
///
/// The excluded target itself is never staged: its absence from the shadow IS
/// the exclusion. esbuild then fails to resolve it, naming the path — the
/// metafile audit (wired separately) is the authoritative backstop.
fn plan_concrete_target_staging(
    target: &str,
    project_root: &Path,
    bundle_exclude: &BundleExcludeMatcher,
    effective_main_fields: &[&str],
    force_stage: bool,
    staging_files: &mut BTreeSet<PathBuf>,
    staging_dirs: &mut BTreeSet<PathBuf>,
) {
    if !force_stage {
        return;
    }

    let target_path = normalize_path_lexical(Path::new(target));
    if bundle_exclude.is_excluded(&target_path, project_root) {
        return;
    }

    let candidates = concrete_ssr_target_candidates(target, effective_main_fields);
    staging_files.extend(
        candidates
            .iter()
            .filter(|candidate| !bundle_exclude.is_excluded(candidate, project_root))
            .cloned(),
    );

    // A directory target can use package `imports` and other package-local
    // resolution that the Rust preprocessing discovery intentionally does
    // not emulate. Stage its complete allowed tree, except for a whole-root
    // alias whose ordinary source walks already provide the bounded mirror.
    if target_path.is_dir() && target_path != project_root {
        staging_dirs.insert(target_path.clone());
    }

    // Never write through `<shadow>/node_modules` because it may be a symlink
    // to the live dependency tree. Isolate and stage the containing package so
    // relative imports remain available while excluded candidates stay absent.
    let mut package_roots = BTreeSet::new();
    for candidate in candidates.iter().chain(std::iter::once(&target_path)) {
        if let Some(package_root) = node_modules_package_root(candidate, project_root) {
            if package_root.is_dir() {
                staging_dirs.insert(package_root.clone());
                package_roots.insert(package_root);
            }
        } else if let Some(package_root) = containing_project_package_root(candidate, project_root)
        {
            if package_root != project_root {
                staging_dirs.insert(package_root.clone());
            }
            package_roots.insert(package_root);
        }
    }
    for package_root in package_roots {
        collect_package_scope_staging_files(&package_root, staging_files);
    }
    staging_files.retain(|file| !bundle_exclude.is_excluded(file, project_root));
}

fn rebase_tsconfig_paths_to_shadow_with_exclusions(
    paths: &BTreeMap<String, Vec<String>>,
    project_root: &Path,
    shadow: &Path,
    // Issue #1668 part 2: the widened first-party root + WORK mirror root, so a
    // workspace-sibling alias target rebases to its staged copy in the work
    // mirror. Pass `project_root` + `shadow` to make the sibling tier inert
    // (the pre-#1668 two-tier project/plugin mapping).
    first_party_root: &Path,
    work_root: &Path,
    bundle_exclude: Option<&BundleExcludeMatcher>,
    node_modules_isolation_root: Option<&Path>,
) -> BTreeMap<String, Vec<String>> {
    // With any `bundle.exclude` policy active, exclusion means exactly "absence
    // from the fully staged shadow tree": every under-root target rewrites to a
    // SHADOW-ONLY target with no live-real fallback, so an excluded file can
    // never be resurrected from the real tree. esbuild resolves inside the
    // shadow; a missing target simply fails to resolve, naming the path (the
    // metafile audit, wired separately, is the authoritative backstop). This is
    // deliberately uniform — no per-target 3-state classification, no
    // provably-disjoint carve-out — so the seam has a single, decidable rule.
    // A workspace-sibling target obeys the SAME rule against the work mirror.
    let exclusions_active = bundle_exclude.is_some_and(|matcher| !matcher.is_empty());
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (key, targets) in paths {
        let mut new_targets: Vec<String> = Vec::with_capacity(targets.len() + 1);
        for target in targets {
            // Split the `/*` glob suffix exactly like resolve_tsconfig_path_target.
            let (prefix, suffix) = match target.rsplit_once("/*") {
                Some((p, "")) => (p, "/*"),
                _ => (target.as_str(), ""),
            };
            let prefix_path = Path::new(prefix);
            // Already under the shadow tree (re-entrant call, or a plugin
            // temp file materialised inside `shadow`) — leave untouched.
            let already_shadowed = prefix_path.starts_with(shadow);
            if !already_shadowed && prefix_path.strip_prefix(project_root).is_ok() {
                // Under project_root → shadow-first.
                // `rel` is empty for the whole-root `@/* -> /root/*`
                // (baseUrl ".") case — the most common alias shape.
                // `shadow.join("")` would yield `<shadow>/` and produce a
                // malformed `<shadow>//*` target; use `shadow` directly so
                // the shadow-first entry is a clean `<shadow>/*`.
                // Only under-`project_root` prefixes reach here (the guard
                // above), so the first-party tier is inert: pass the project
                // mirror as its own first-party/work root.
                let shadow_prefix = shadow_path_for_project_path(
                    prefix_path,
                    project_root,
                    project_root,
                    shadow,
                    shadow,
                    node_modules_isolation_root,
                );
                let mut shadow_target = preserve_trailing_path_separator(
                    shadow_prefix.to_string_lossy().into_owned(),
                    prefix,
                );
                shadow_target.push_str(suffix);
                push_unique(&mut new_targets, shadow_target);

                if exclusions_active {
                    // Shadow-only: suppress the live-real fallback entirely.
                    continue;
                }
            } else if !already_shadowed
                && first_party_root != project_root
                && prefix_path.strip_prefix(first_party_root).is_ok()
            {
                // Issue #1668 part 2: a workspace-sibling alias target (outside
                // project_root, inside first_party_root) rebases to its
                // workspace-relative slot in the WORK mirror — work-first with
                // the real-tree fallback under an empty exclude, staged-only
                // under exclusions (the same uniform rule as the project tier).
                // `shadow_path_for_project_path` deliberately leaves the
                // first-party root itself unchanged because its usual callers
                // map concrete files, not a whole workspace. A broad root
                // alias (`@/* -> <workspace>/*`) is the one exception: only
                // its concrete runtime claims were staged into `work_root`, so
                // the wildcard must resolve against that staged view first.
                let work_prefix = if prefix_path == first_party_root {
                    work_root.to_path_buf()
                } else {
                    shadow_path_for_project_path(
                        prefix_path,
                        project_root,
                        first_party_root,
                        shadow,
                        work_root,
                        node_modules_isolation_root,
                    )
                };
                let mut work_target = preserve_trailing_path_separator(
                    work_prefix.to_string_lossy().into_owned(),
                    prefix,
                );
                work_target.push_str(suffix);
                push_unique(&mut new_targets, work_target);

                if exclusions_active {
                    // Shadow-only: suppress the live-real fallback entirely.
                    continue;
                }
            }
            // Empty `bundle.exclude` (or an out-of-root / plugin / shadow
            // target): keep the original real-abs target as the graceful
            // fallback (the dual-target array esbuild tries in order), or as
            // the sole target when it is not under the project root.
            push_unique(&mut new_targets, target.clone());
        }
        out.insert(key.clone(), new_targets);
    }
    out
}

/// Push `value` onto `vec` only if not already present — keeps the
/// dual-target arrays de-duplicated (and guards the case where the shadow
/// and real targets collapse to the same string on a case-insensitive FS).
fn push_unique(vec: &mut Vec<String>, value: String) {
    if !vec.iter().any(|v| v == &value) {
        vec.push(value);
    }
}

/// Write a synthetic `tsconfig.json` esbuild can read for path-alias
/// resolution. `baseUrl` is set to `"."` (the shadow root); the user's
/// `paths` are kept verbatim — they're already expressed relative to
/// the project root, and our shadow tree mirrors that layout, so the
/// targets resolve correctly.
fn write_synthetic_tsconfig(
    shadow: &Path,
    paths: &BTreeMap<String, Vec<String>>,
    jsx_import_source: &str,
) -> Result<()> {
    let json = serde_json::json!({
        "//": "Synthetic tsconfig generated by zfb_build::bundler. \
               Driven by BundlerInput::tsconfig_paths so esbuild --tsconfig \
               can resolve user path aliases like @/components/foo.",
        "compilerOptions": {
            "baseUrl": ".",
            "paths": paths,
            "jsx": "react-jsx",
            "jsxImportSource": jsx_import_source,
            "moduleResolution": "Bundler",
            "module": "ESNext",
            "target": "ES2022",
            "isolatedModules": true,
            "esModuleInterop": true,
            "resolveJsonModule": true,
        }
    });
    let path = shadow.join(SHADOW_TSCONFIG_FILENAME);
    fs::write(&path, serde_json::to_vec_pretty(&json)?)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

// Emission inputs for write_entry_module. Grouping these into a struct lets
// future `globalThis.__zfb.*` slots add a field here instead of widening the
// function signature.
struct EntryModuleInputs<'a> {
    render_to_string_module: &'a str,
    content_snapshot_json: Option<&'a str>,
    content_imports: &'a [ContentImport],
    site: Option<&'a str>,
    prefetch_disabled: bool,
    /// Shadow-relative specifier of the materialised `mdx-components.tsx`
    /// (sub-issue #616). When `Some`, emit a default `import` of the file
    /// plus the `globalThis.__zfb.mdxComponents` installer. `None` => omit.
    mdx_components_import_spec: Option<&'a str>,
    /// Base prefix for the `clientScript()` SSR helper (#978).
    ///
    /// `Some(prefix)` → emit `globalThis.__zfb.base = <json-prefix>` so
    /// `clientScript(name)` can build base-prefixed stable URLs at SSR time.
    /// `None` → omit entirely (builds without client scripts stay byte-identical).
    base_prefix: Option<&'a str>,
}

/// Generate the `entry.mjs` module that re-exports `routes`,
/// `hydrateIsland`, and a Workers-style `default { fetch }` wrapper
/// driven by `createPageRouter` from `@takazudo/zfb-runtime/server`. This is
/// the single load-bearing module the embedded V8 host (T6/T7) and the
/// runtime SSR adapter (T2) consume.
///
/// `render_to_string_module` is the framework's `renderToString`
/// specifier (e.g. `"preact-render-to-string"` for Preact,
/// `"react-dom/server"` for React) — drawn from
/// [`zfb_render::adapters::Adapter::render_to_string_module`]. The
/// wrapper imports `renderToString` by name from this specifier and
/// hands it to `createPageRouter` as the framework adapter, so the
/// bundle pins its own SSR call without leaking the framework choice
/// into the embedded V8 host's boot.
///
/// The default-fetch wrapper is emitted unconditionally, even when
/// `routes` is empty: an empty Hono app simply 404s every request, but
/// the bundle still satisfies workerd's "module must export default
/// with a fetch handler" contract so the embedded V8 host can boot and
/// surface a clean 404 rather than a missing-export error.
/// Generate the `entry.mjs` module.
///
/// `content_snapshot_json` is the JSON-serialized content snapshot to
/// embed. When `None`, a placeholder `{ collections: {} }` is used.
///
/// `content_imports` carries one entry per MDX file the per-collection
/// materialiser handled (#506). Each entry triggers two emitted lines
/// in the body:
///
/// 1. `import * as __zfb_content_<i> from "./<shadow_rel_path>";`
/// 2. A `[<specifier>, __zfb_content_<i>.default]` row inside the
///    `globalThis.__zfb.content` bridge map. Both the hash-bearing
///    `mdx://<collection>/<slug>#<hash>` form (Rust snapshot) and the
///    hash-stripped `mdx://<collection>/<slug>` form (JS stub) are
///    registered so `bridge.get(...)` resolves either flavour, per
///    the contract documented in `crates/zfb-render/src/loader.rs`.
///
/// When `content_imports` is empty the bridge installer is omitted —
/// runtime `bridge?.get(...)` calls fall through to the
/// `<pre data-zfb-content-fallback>` shape, matching the behaviour of
/// builds with no content collections.
fn write_entry_module(
    shadow: &Path,
    routes: &[RouteEntry],
    inputs: &EntryModuleInputs<'_>,
) -> Result<()> {
    let render_to_string_module = inputs.render_to_string_module;
    let content_snapshot_json = inputs.content_snapshot_json;
    let content_imports = inputs.content_imports;
    let site = inputs.site;
    let prefetch_disabled = inputs.prefetch_disabled;
    let mdx_components_import_spec = inputs.mdx_components_import_spec;
    let base_prefix = inputs.base_prefix;
    use std::fmt::Write as _;

    // Static-HTML routes are emitted verbatim by the renderer and must
    // NOT appear in the JS bundle's imports, `routes` export, or
    // `__zfb_pages` array. Filter them out here so the bundle only
    // contains the JS page modules.
    let js_routes: Vec<&RouteEntry> = routes.iter().filter(|r| !r.static_html).collect();

    let mut src = String::new();
    src.push_str("// AUTO-GENERATED by zfb_build::bundler. Do not edit.\n");
    src.push_str(
        "// Single ESM entry shared by the embedded V8 host (T6/T7) and the runtime SSR adapter.\n",
    );
    src.push_str("// Exports: { routes, hydrateIsland, default: { fetch } }.\n\n");
    src.push_str(&format!(
        "import {{ hydrateIsland }} from \"./{SHADOW_HYDRATE_FILENAME}\";\n",
    ));
    // `createPageRouter` lives at the server-only subpath so the client-safe
    // root barrel (`@takazudo/zfb-runtime`) never pulls Hono into an island's
    // `--platform=browser` bundle (issue #1298). This SSR entry runs on the
    // Worker side, where resolving `hono` is expected and correct. The literal
    // is the shared `ZFB_RUNTIME_SERVER_SPECIFIER` so the staged-dependency seed
    // (issue #1645) stays in lockstep with what the entry actually imports.
    writeln!(
        &mut src,
        "import {{ createPageRouter }} from \"{ZFB_RUNTIME_SERVER_SPECIFIER}\";"
    )
    .unwrap();
    writeln!(
        &mut src,
        "import {{ renderToString as __zfb_renderToString }} from {spec};",
        spec = json_str(render_to_string_module),
    )
    .unwrap();

    // Stable per-route import alias so mangled-letter routes still
    // produce a valid identifier.
    for (idx, route) in js_routes.iter().enumerate() {
        // Import the shadow page module by its path **under `pages_dir`**.
        // `rel_under_pages` is carried straight from the materialise walk
        // (#1193), so the import stays correct no matter where `pages_dir`
        // physically is — the real `project_root/pages`, #1518's private empty
        // dev root, OR a per-build overlay temp dir (package-owned routes). The
        // old derivation
        // (`route_path_under_pages(source_path)`, a literal `pages/`-prefix
        // strip) collapsed nested overlay routes to a bare filename; that
        // path is retired. The `source_path` fallback only fires for an
        // empty `rel_under_pages` (a legacy manifest deserialised with the
        // serde default).
        let rel_under_pages = if route.rel_under_pages.as_os_str().is_empty() {
            route_path_under_pages(&route.source_path)
        } else {
            rel_to_forward_slash(&route.rel_under_pages)
        };
        let import_path = format!("./pages/{}", rel_under_pages);
        writeln!(
            &mut src,
            "import * as __zfb_route_{idx} from \"{import_path}\";",
        )
        .unwrap();
    }

    // Per-MDX-entry namespace imports for the content bridge (#506).
    // Each `__zfb_content_<i>` is the namespace of a compiled MDX
    // module; its `.default` is the `MDXContent({components}) → JSX`
    // function the JS-side `<entry.Content>` invokes.
    for (idx, ci) in content_imports.iter().enumerate() {
        writeln!(
            &mut src,
            "import * as __zfb_content_{idx} from \"./{path}\";",
            path = ci.shadow_rel_path,
        )
        .unwrap();
    }

    // Default import of the project-root `mdx-components.tsx` global override
    // map (#616). The file's default export is the canonical `{ h2, … }`
    // contract; the matching installer (`globalThis.__zfb.mdxComponents`) is
    // emitted in the globalThis section below, before `createPageRouter`.
    if let Some(spec) = mdx_components_import_spec {
        writeln!(
            &mut src,
            "import __zfb_mdx_components from {spec};",
            spec = json_str(spec),
        )
        .unwrap();
    }

    src.push_str("\nexport const routes = {\n");
    for (idx, route) in js_routes.iter().enumerate() {
        writeln!(
            &mut src,
            "  {key}: __zfb_route_{idx},",
            key = json_str(&route.route),
        )
        .unwrap();
    }
    src.push_str("};\n\n");
    src.push_str("export { hydrateIsland };\n\n");

    // -----------------------------------------------------------------
    // Workers-style default-fetch wrapper.
    //
    // The embedded V8 host loads this bundle as a Module-syntax worker;
    // it dispatches every request through `default.fetch`. We construct
    // the fetch handler exactly once at module evaluation time using
    // `createPageRouter` from `@takazudo/zfb-runtime`. Inputs:
    //
    //   - `pages`: derived from the static `routes` map. Each entry is
    //     a `{ route, module: () => Promise<PageModule> }` pair where
    //     the module thunk returns the already-loaded namespace (no
    //     code splitting today; the bundler emits everything in one
    //     ESM file).
    //   - `contentSnapshot`: a placeholder empty snapshot. Wave 2 will
    //     embed the real snapshot here so user pages calling
    //     `getCollection(...)` resolve from memory; until then,
    //     content-using pages must still be authored to handle an
    //     empty snapshot. The wrapper is emitted unconditionally so
    //     embedded V8 host boot is decoupled from the snapshot deliverable.
    //   - `framework`: an inline adapter pinning `renderToString` to
    //     the framework's import. This keeps `@takazudo/zfb-runtime`
    //     framework-agnostic and lets the bundle pick its own SSR call.
    // -----------------------------------------------------------------
    // The `__zfb_pages` array feeds Hono's router via `createPageRouter`.
    // Hono uses `:param` / `:param{.+}` syntax for dynamic segments, not
    // the `[param]` / `[...param]` file-system convention that `derive_route`
    // returns. Convert each route key to Hono syntax here so
    // `createPageRouter` can register the routes correctly and so the
    // `/__paths__/` synthetic endpoint resolves page lookups by the same key.
    src.push_str("const __zfb_pages = [\n");
    for (idx, route) in js_routes.iter().enumerate() {
        let hono_key = bracket_to_hono(&route.route);
        writeln!(
            &mut src,
            "  {{ route: {key}, module: () => Promise.resolve(__zfb_route_{idx}) }},",
            key = json_str(&hono_key),
        )
        .unwrap();
    }
    src.push_str("];\n\n");
    // Embed the content snapshot. When the caller supplies a real
    // snapshot (JSON-serialized `ContentSnapshot`), inline it so
    // `getCollection(...)` resolves from memory inside the worker.
    // The fallback empty snapshot is used for builds where content
    // collections are not needed.
    //
    // Defensively validate that the supplied string is well-formed JSON
    // (not just a JSON object) before inlining. The value is produced by
    // `serde_json::to_string` in the build pipeline, so failures here
    // would indicate a bug rather than user input, but the check prevents
    // accidental JS injection if the call site ever changes.
    let snapshot_literal = if let Some(json) = content_snapshot_json {
        // Validate both syntax AND shape. The runtime expects a top-level
        // object whose `collections` field is itself an object — anything
        // else (a JSON array, a scalar, an object missing `collections`)
        // would crash `getCollection` at request time. Fall back to the
        // safe empty snapshot so the build still produces a working
        // worker; the resolver will return empty collections, which is
        // visible in the build summary (zero dynamic pages).
        serde_json::from_str::<serde_json::Value>(json)
            .ok()
            .filter(|v| v.is_object() && v.get("collections").is_some_and(|c| c.is_object()))
            .map(|_| json)
            .unwrap_or(r#"{ "collections": {} }"#)
    } else {
        r#"{ "collections": {} }"#
    };
    writeln!(
        &mut src,
        "const __zfb_content_snapshot = {snapshot_literal};",
    )
    .unwrap();
    src.push('\n');

    // ---------------------------------------------------------------
    // Site setter (sub-issue #254).
    //
    // When `site` is configured, emit it onto `globalThis.__zfb.site`
    // so layouts can build canonical `<link>` tags, OG URLs, sitemap
    // absolute hrefs, and hreflang `<link rel="alternate">` from a
    // single config-level source. When absent, zero bytes are emitted
    // so the build output is byte-for-byte identical to the pre-`site`
    // build.
    // ---------------------------------------------------------------
    if let Some(site_url) = site {
        src.push_str("globalThis.__zfb = globalThis.__zfb ?? {};\n");
        src.push_str(&format!(
            "globalThis.__zfb.site = {};\n\n",
            json_str(site_url)
        ));
    }

    // ---------------------------------------------------------------
    // Prefetch-disabled flag (#277).
    //
    // When `prefetch.disabled === true` in `zfb.config.ts`, emit
    // `globalThis.__zfb.prefetchDisabled = true` so that
    // `<ClientRouter />` renders `<meta name="zfb-prefetch-disabled"
    // content="true">` and the runtime's prefetch-core short-circuits
    // at `init()` time.  The flag is site-wide and static — emitted
    // once at bundle time, never per-page.  When absent, zero bytes
    // are added so the build output is byte-for-byte identical to the
    // pre-`prefetch` build.
    // ---------------------------------------------------------------
    if prefetch_disabled {
        src.push_str("globalThis.__zfb = globalThis.__zfb ?? {};\n");
        src.push_str("globalThis.__zfb.prefetchDisabled = true;\n\n");
    }

    // ---------------------------------------------------------------
    // Client-script base prefix (#978).
    //
    // Emitted IFF at least one `*.client.*` entry was discovered
    // (signalled by `base_prefix` being `Some`). The value is the
    // resolved base prefix — `""` for root-mounted / no-base builds,
    // `"/foo"` for `base="/foo/"` sub-path builds. `clientScript(name)`
    // reads `globalThis.__zfb?.base ?? ""` so it gets the right prefix
    // whether or not this setter ran.
    //
    // Zero-script builds stay byte-for-byte identical to before (#261
    // zero-registration parity, #940 byte-identical dev bundle skip).
    // ---------------------------------------------------------------
    if let Some(prefix) = base_prefix {
        src.push_str("globalThis.__zfb = globalThis.__zfb ?? {};\n");
        src.push_str(&format!(
            "globalThis.__zfb.base = {};\n\n",
            json_str(prefix)
        ));
    }

    // ---------------------------------------------------------------
    // Global MDX component-override map installer (sub-issue #616).
    //
    // Populates the `globalThis.__zfb.mdxComponents` slot read by
    // `mergeMdxComponents` in `@takazudo/zfb`'s `content.ts` (the
    // precedence seam from #614: defaultComponents → this slot →
    // per-`<Content>` `components`). `buildContentComponent` does a lazy
    // per-render lookup, so install-ordering is a non-issue; we still
    // emit before `createPageRouter` alongside the other setters.
    //
    // Emitted independently of `content_imports`: a project may define
    // overrides with zero content-collection entries. The idempotent
    // `__zfb ??= {}` guard makes the install safe regardless of which
    // other setters above already ran. When the file is absent, zero
    // bytes are emitted so the output is byte-for-byte identical to a
    // project without the convention.
    // ---------------------------------------------------------------
    if mdx_components_import_spec.is_some() {
        src.push_str("globalThis.__zfb = globalThis.__zfb ?? {};\n");
        src.push_str("globalThis.__zfb.mdxComponents = __zfb_mdx_components;\n\n");
    }

    // ---------------------------------------------------------------
    // Content bridge installer (#506).
    //
    // The JS-side `<entry.Content>` (in `@takazudo/zfb/content`)
    // calls `globalThis.__zfb.content.get(entry.module_specifier)`
    // and renders the returned `MDXContent({components})` function
    // when present, falling back to a `<pre
    // data-zfb-content-fallback>` block otherwise. We install the
    // bridge before `createPageRouter` so the very first SSR call
    // already sees the populated map.
    //
    // Both forms documented in `crates/zfb-render/src/loader.rs`
    // are registered:
    //
    // - `mdx://<collection>/<slug>#<hash>` — the Rust snapshot's
    //   `module_specifier`, baked by
    //   `compile_mdx_to_jsx_module_cached` and serialized into
    //   `EntrySnapshot.module_specifier`.
    // - `mdx://<collection>/<slug>` — the hash-less form the JS stub
    //   constructs in `buildModuleSpecifier(name, slug)` when reading
    //   collections off disk (no JSX hash to compute).
    //
    // Both keys point at the same `__zfb_content_<i>.default` value
    // so a `bridge.get(spec)` call succeeds regardless of which
    // shape the caller obtained `entry.module_specifier` from.
    // ---------------------------------------------------------------
    if !content_imports.is_empty() {
        src.push_str("const __zfb_content_modules = new Map([\n");
        for (idx, ci) in content_imports.iter().enumerate() {
            // Hash-bearing form (Rust snapshot).
            writeln!(
                &mut src,
                "  [{key}, __zfb_content_{idx}.default],",
                key = json_str(&ci.specifier),
            )
            .unwrap();
            // Hash-stripped form (JS stub fallback). Skip when the
            // specifier already has no `#` segment — should never
            // happen for `compile_mdx_to_jsx_module_cached` output,
            // but the bundler stays defensive in case the upstream
            // contract changes.
            if let Some((no_hash, _)) = ci.specifier.split_once('#') {
                writeln!(
                    &mut src,
                    "  [{key}, __zfb_content_{idx}.default],",
                    key = json_str(no_hash),
                )
                .unwrap();
            }
        }
        src.push_str("]);\n");
        src.push_str("globalThis.__zfb = globalThis.__zfb ?? {};\n");
        src.push_str(
            "globalThis.__zfb.content = { get: (spec) => __zfb_content_modules.get(spec) };\n\n",
        );
    }

    src.push_str("const __zfb_router = createPageRouter({\n");
    src.push_str("  pages: __zfb_pages,\n");
    src.push_str("  contentSnapshot: __zfb_content_snapshot,\n");
    src.push_str("  framework: { renderToString: __zfb_renderToString },\n");
    src.push_str("});\n\n");
    src.push_str("export default {\n");
    src.push_str("  fetch: (request) => __zfb_router(request),\n");
    src.push_str("};\n");

    let path = shadow.join(SHADOW_ENTRY_FILENAME);
    fs::write(&path, src.as_bytes()).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Normalise a `pages_dir`-relative path to forward-slash form for use
/// in a JS import specifier. Windows back-slashes become `/`; an
/// already-POSIX path is unchanged.
fn rel_to_forward_slash(rel: &Path) -> String {
    rel.to_string_lossy().replace('\\', "/")
}

/// Heuristic to recover "path under pages/" from a project-relative
/// page path. We assume `source_path` starts with `pages/` (since the
/// pages-dir walk pushed RouteEntries with project-relative source
/// paths). If for some reason it doesn't, fall back to the file name.
///
/// Retained only as the legacy fallback for [`RouteEntry`]s deserialised
/// from an older manifest (empty `rel_under_pages`). New routes carry
/// `rel_under_pages` from the walk and never reach this — see the import
/// emit in [`write_entry_module`].
fn route_path_under_pages(source_path: &Path) -> String {
    for prefix in ["pages/", "pages\\"] {
        let s = source_path.to_string_lossy();
        if let Some(rest) = s.strip_prefix(prefix) {
            return rest.replace('\\', "/");
        }
    }
    // Last resort: take just the file name. This shouldn't fire in
    // practice but keeps the function total.
    source_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Render the TSX shell module for a `.md` page.
///
/// Returns a complete TSX module that:
/// 1. Imports the compiled MDX body module (a `_`-prefixed sibling file).
/// 2. Exports a default page component that renders a full `<html>` document
///    with `<head>` (charset + title) and `<body>` containing the MDX body.
///
/// Recognised frontmatter keys (both optional):
/// - `title` (string) — used as `<title>`. Falls back to `slug_fallback`
///   (last URL segment, or `"index"` for `/`).
/// - `lang` (string) — used as `<html lang="…">`. Defaults to `"en"`.
///
/// All other frontmatter keys are silently ignored (v1 contract: no layout
/// system, no `layout:` frontmatter).
///
/// The title and lang values are assigned to `const` variables and
/// referenced via JSX expressions so any reserved characters (`&`, `"`,
/// `<`) in frontmatter values cannot break the generated JSX syntax.
pub(crate) fn render_md_page_shell(
    frontmatter: &serde_json::Value,
    slug_fallback: &str,
    body_import: &str,
) -> String {
    // Extract title: string from frontmatter, else slug fallback.
    let title = frontmatter
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or(slug_fallback);
    // Extract lang: string from frontmatter, else "en".
    let lang = frontmatter
        .get("lang")
        .and_then(|v| v.as_str())
        .unwrap_or("en");

    format!(
        "// AUTO-GENERATED by zfb_build::bundler (md page shell). Do not edit.\n\
         import MdBody from {import_spec};\n\
         \n\
         const __title = {title_json};\n\
         const __lang = {lang_json};\n\
         \n\
         export default function MdPage() {{\n\
         \u{0020} return (\n\
         \u{0020}   <html lang={{__lang}}>\n\
         \u{0020}     <head>\n\
         \u{0020}       <meta charSet=\"utf-8\" />\n\
         \u{0020}       <title>{{__title}}</title>\n\
         \u{0020}     </head>\n\
         \u{0020}     <body>\n\
         \u{0020}       <MdBody />\n\
         \u{0020}     </body>\n\
         \u{0020}   </html>\n\
         \u{0020} );\n\
         }}\n",
        import_spec = json_str(body_import),
        title_json = json_str(title),
        lang_json = json_str(lang),
    )
}

/// Convert a route string from the file-system bracket notation used by
/// `derive_route` (e.g. `/blog/[slug]`, `/docs/[...slug]`) into the
/// Hono path-pattern notation (`/blog/:slug`, `/docs/:slug{.+}`) that
/// `createPageRouter` registers with the Hono app.
///
/// Segment rules:
/// - `[[...param]]` → `:param{.+}?` (optional catchall — zero or more
///   path segments; the zero case matches the bare prefix URL)
/// - `[...param]` → `:param{.+}` (catchall — one or more path
///   segments separated by `/`, matched by Hono's regex quantifier)
/// - `[param]`    → `:param` (single-segment dynamic param)
/// - Anything else (static) → unchanged
///
/// Leading `/` and the overall shape of the route are preserved.
/// Non-bracket segments (e.g. `blog`, `page`) are left as-is.
pub(crate) fn bracket_to_hono(route: &str) -> String {
    // Collect non-empty segments from the route and transform each.
    // We split on '/' and skip empty parts (produced by the leading
    // slash and by the `/` root route which splits to ["", ""]).
    let segments: Vec<&str> = route.split('/').filter(|s| !s.is_empty()).collect();

    if segments.is_empty() {
        // The root route `/`.
        return "/".to_string();
    }

    let mut out = String::with_capacity(route.len() + 4);
    for segment in &segments {
        out.push('/');
        if segment.starts_with("[[...") && segment.ends_with("]]") {
            // Optional catchall: `[[...param]]` → `:param{.+}?`. Checked
            // before the single-bracket forms so the doubled brackets do
            // not fall into the plain-dynamic branch. Must stay
            // bit-identical to `Segment::OptionalCatchall::template()` so
            // the worker's `pagesByRoute` lookup keys match.
            let name = &segment[5..segment.len() - 2];
            out.push(':');
            out.push_str(name);
            out.push_str("{.+}?");
        } else if segment.starts_with("[...") && segment.ends_with(']') {
            // Catchall: `[...param]` → `:param{.+}`
            let name = &segment[4..segment.len() - 1];
            out.push(':');
            out.push_str(name);
            out.push_str("{.+}");
        } else if segment.starts_with('[') && segment.ends_with(']') {
            // Dynamic: `[param]` → `:param`
            let name = &segment[1..segment.len() - 1];
            out.push(':');
            out.push_str(name);
        } else {
            out.push_str(segment);
        }
    }
    out
}

/// Returns `true` when [`run_esbuild`] will invoke esbuild with
/// `--preserve-symlinks`. Encodes the three activation paths described
/// in `run_esbuild`'s big comment block as a single shared predicate so
/// the flag decision and the `copy_mode` derivation in [`bundle`] cannot
/// drift apart.
///
/// `true` ⇒ esbuild stays anchored at the shadow tree (symlinked source
/// files are resolved at their shadow path, so in-shadow transforms are
/// already visible — no copy needed).
///
/// `false` ⇒ **branch 4**: `node_modules_dir.is_some()` +
/// `!node_modules_preserve_symlinks` + non-empty `tsconfig_paths`.
/// esbuild canonicalises symlinked source files back to the real tree
/// (deliberately, to keep #443/#450 workspace path-alias resolution
/// working). In that one config, [`bundle`] sets `copy_mode = true` so
/// the in-shadow source copies are real files and the
/// `import.meta.glob` / CSS-module transforms become visible.
fn esbuild_will_preserve_symlinks(input: &BundlerInput) -> bool {
    (input.node_modules_dir.is_some() && input.node_modules_preserve_symlinks)
        || input.node_modules_dir.is_none()
        || input.tsconfig_paths.is_empty()
}

fn effective_ssr_main_fields(input: &BundlerInput) -> Vec<&str> {
    if !input.main_fields.is_empty() {
        input.main_fields.iter().map(String::as_str).collect()
    } else if matches!(input.framework, Framework::React) {
        vec!["main", "module"]
    } else {
        Vec::new()
    }
}

/// Resolve and run the esbuild subprocess.
///
/// When `metafile_path` is `Some`, esbuild also writes its `--metafile` JSON
/// there. The metafile's `inputs` graph is the canonical *transitive* import
/// graph esbuild itself resolved — the dev path parses it to populate per-route
/// `DepKind::Module` edges (#1284/#1287), while every real bundle pass uses its
/// `outputs` map to publish copied Wasm deployment assets.
// The shadow layout roots (`shadow`/`first_party_root`/`work_root`) are passed
// separately rather than folded into a struct so each stays explicit at the
// single call site (issue #1668).
#[allow(clippy::too_many_arguments)]
fn run_esbuild(
    input: &BundlerInput,
    shadow: &Path,
    // Issue #1668: the widened first-party root and the WORK mirror root, so
    // the plugin-alias remap and tsconfig rebase below can point a
    // workspace-sibling alias target at its staged copy in the work mirror.
    // Outside a workspace `first_party_root == normalize(project_root)` and
    // `work_root == shadow`, making the sibling tier inert.
    first_party_root: &Path,
    work_root: &Path,
    bundle_path: &Path,
    metafile_path: Option<&Path>,
    bundle_exclude: &BundleExcludeMatcher,
    node_modules_isolation_root: Option<&Path>,
) -> Result<()> {
    let bin = resolve_esbuild_binary(input.esbuild_binary.as_deref())?;
    let entry = shadow.join(SHADOW_ENTRY_FILENAME);
    let tsconfig = shadow.join(SHADOW_TSCONFIG_FILENAME);

    let mut cmd = Command::new(&bin);
    cmd.current_dir(shadow);
    cmd.arg("--bundle");
    cmd.arg("--format=esm");
    cmd.arg("--platform=neutral");
    cmd.arg("--target=es2022");
    cmd.arg("--splitting=false");
    cmd.arg("--tree-shaking=true");
    cmd.arg("--sourcemap=linked");
    cmd.arg("--log-level=warning");
    // esbuild caps its own error/warning output at 10 messages by default and
    // silently drops the rest. `friendly_esbuild_error`'s note loop below
    // walks every `Could not resolve` block in stderr to report ALL escaping
    // imports in one shot (issue #1839) — without uncapping the limit here,
    // a project with more than 10 offenders would only ever surface the
    // first batch, forcing a fix-one/rebuild/discover-next loop.
    cmd.arg("--log-limit=0");
    // The `.wasm=copy` loader emits these beside the bundle. Pin the basename
    // format so both production passes produce deterministic deployment asset
    // paths that can be safely handed to an adapter.
    cmd.arg("--asset-names=[name]-[hash]");
    for arg in esbuild_loader_args(input) {
        cmd.arg(arg);
    }

    if input.mode.is_prod() && input.minify {
        cmd.arg("--minify");
    }

    cmd.arg(format!("--tsconfig={}", tsconfig.display()));
    cmd.arg(format!("--outfile={}", bundle_path.display()));
    if let Some(meta) = metafile_path {
        cmd.arg(format!("--metafile={}", meta.display()));
    }

    // MDX modules emitted by `compile_mdx_to_jsx_module_cached` carry a
    // hard-coded `import { Fragment as _Fragment } from "react/jsx-runtime";`
    // (the emitter targets the React JSX-runtime convention). Esbuild's
    // own JSX transform handles JSX *syntax* through tsconfig's
    // `jsxImportSource`, but **explicit import statements** are passed
    // through unchanged. So when the project's framework is Preact, we
    // rewrite `react/jsx-runtime` (and the dev-runtime sibling) to the
    // Preact equivalents at the bundler level. This is the same trick
    // the Preact ecosystem uses with bundlers like Vite.
    if matches!(input.framework, Framework::Preact) {
        cmd.arg("--alias:react/jsx-runtime=preact/jsx-runtime");
        cmd.arg("--alias:react/jsx-dev-runtime=preact/jsx-dev-runtime");
    }

    // React-only: route conditional-exports resolution through the
    // `worker` condition so `react-dom/server` resolves to its
    // `server.browser.js` build instead of the `default` →
    // `server.node.js` build. The node build does
    // `require("stream")` / `require("util")`, which esbuild cannot
    // satisfy under `--platform=neutral` (this bundle runs as a
    // workerd-style ES module in the embedded V8 host, where node
    // builtins do not exist) — without this it fails with
    // `Could not resolve "stream"`. react-dom's exports map keys the
    // browser-safe SSR entry under the `worker`/`browser`/`deno`
    // conditions; `worker` is the surgical choice because react-dom
    // honors it for the server-render entry while Preact's packages do
    // not use it, so the Preact bundle's resolution is unaffected.
    // Gated on `Framework::React` so the Preact path adds no new arg and
    // cannot regress. esbuild's exports-map resolution takes precedence
    // over `--main-fields`, so no main-fields change is needed for the
    // exports-based react-dom package.
    if matches!(input.framework, Framework::React) {
        cmd.arg("--conditions=worker");
    }

    // Main-fields for the `--platform=neutral` page/SSR pass. Under `neutral`
    // esbuild's main-fields list is EMPTY by default, so a package resolved
    // purely via `package.json` `main`/`module` (no `exports` map) fails with
    // `Could not resolve "<pkg>" ... The "main" field here was ignored. Main
    // fields must be configured explicitly when using the "neutral" platform.`
    //
    // Resolution order:
    // 1. An explicit `bundle.mainFields` (input.main_fields) wins for EVERY
    //    framework -- the #676 host knob (e.g. a Preact project hitting
    //    `msw` -> `path-to-regexp@6` sets `["main", "module"]`).
    // 2. Otherwise React keeps its historical `main,module` default (the
    //    `@headlessui/react` -> `@floating-ui/react` -> `tabbable` chain the T6
    //    configurator depends on; `tabbable` ships `main`/`module`, no
    //    `exports`). `--conditions=worker` cannot help -- it only steers
    //    `exports`-map resolution.
    // 3. Otherwise (non-React, no knob) NO `--main-fields` is emitted, keeping
    //    the Preact bundle's arg set byte-identical (zero regression).
    //
    // Safe in all cases: `--main-fields` only affects packages WITHOUT an
    // `exports` map (`exports` always takes precedence), so it can only turn a
    // currently *failing* main-only resolution into a success, never alter a
    // working one. `main,module` matches esbuild's node-platform default order.
    let effective_main_fields = effective_ssr_main_fields(input);
    if !effective_main_fields.is_empty() {
        cmd.arg(format!("--main-fields={}", effective_main_fields.join(",")));
    }

    // User code consults the public SDK via the bare `zfb` namespace
    // (`zfb/content`, `zfb/config`, `zfb/paginate`, …) — these are the
    // documented import paths surfaced by `@takazudo/zfb`'s `exports`
    // map. The npm package itself is published as `@takazudo/zfb`, so
    // teach esbuild to treat the bare `zfb` prefix as an alias for
    // `@takazudo/zfb`. esbuild's `--alias:<from>=<to>` works for both
    // exact specifiers and subpath suffixes, so `zfb/content` →
    // `@takazudo/zfb/content` resolves through the package's exports
    // map without any per-subpath wiring.
    cmd.arg("--alias:zfb=@takazudo/zfb");

    // Plugin-registered aliases + virtual modules (#269). Both surface
    // through the synthetic `compilerOptions.paths` map esbuild reads
    // via `--tsconfig=<tsconfig.json>` above. Plugin aliases (`@/foo`)
    // are tsconfig-paths-ONLY.
    //
    // Why aliases avoid `--alias`: esbuild's `--alias:<from>=<to>` is
    // prefix-with-slash — registering `@/foo` would silently also
    // rewrite `@/foo/bar` (which can be a real file under a directory
    // alias), contradicting the documented exact-match contract honored
    // by the embedded V8 host
    // (`zfb-render::BundleModuleLoader::resolve_alias`). A
    // `compilerOptions.paths` entry without the wildcard suffix is a
    // literal exact match in the TypeScript / esbuild path-mapping
    // pipeline.
    //
    // Virtual modules ADDITIONALLY surface as `--alias` flags — see
    // `resolver_inputs.virtual_module_alias_flags()` appended to `cmd`
    // after the tsconfig is rewritten below (#1263). The tsconfig is not
    // applied to source files under `node_modules`, so the alias is what
    // resolves a `virtual:*` import from a node_modules route entrypoint;
    // it is exact-match-safe because each virtual target is a single
    // `.mjs` file.
    //
    // `zfb_plugin_resolver::build_resolver_inputs` materializes each
    // virtual module to a `.zfb-virtual-*.mjs` temp file inside
    // `shadow` (so esbuild's upward `node_modules` walk still finds
    // the right packages) and returns POSIX-normalized
    // `(specifier, absolute-path)` pairs. The `NamedTempFile` handles
    // live inside `resolver_inputs._temp_files` and are dropped after
    // the subprocess returns.
    // `input.tsconfig_paths` carries the user's own `compilerOptions.paths`
    // (keyed by pattern, pre-rebase — rebasing only rewrites targets). Pass
    // it so a `virtual:*` specifier the user already claims is NOT also
    // emitted as a plugin `--alias` (which esbuild applies BEFORE tsconfig
    // `paths`, overriding the user's mapping). User-wins, #1267.
    let project_root = normalize_path_lexical(&input.project_root);
    // Remap each plugin alias whose target lives under the project root to its
    // shadow copy.
    //
    // With exclusions active the shadow copy is ALWAYS the target: exclusion =
    // absence from the shadow, so an excluded alias target is simply not staged
    // and esbuild fails to resolve it, naming the specifier. There is no guard
    // child and no live-real fallback.
    //
    // With an empty `bundle.exclude` the historical behaviour is preserved:
    // point at the shadow copy only when a shadow candidate actually exists,
    // else keep the real target (graceful degradation, byte-identical to a
    // build without the knob).
    let exclusions_active = !bundle_exclude.is_empty();
    let effective_plugin_aliases = input
        .plugin_alias_entries
        .iter()
        .map(|(specifier, target)| {
            let target_path = normalize_path_lexical(Path::new(target));
            let remapped = match target_path.strip_prefix(&project_root) {
                Ok(_) => {
                    // Under-`project_root` targets only (the `match` guard), so
                    // the first-party tier is inert: the project mirror is its
                    // own first-party/work root here.
                    let shadow_target = shadow_path_for_project_path(
                        &target_path,
                        &project_root,
                        &project_root,
                        shadow,
                        shadow,
                        node_modules_isolation_root,
                    );
                    if exclusions_active {
                        shadow_target
                    } else {
                        let has_shadow_candidate =
                            concrete_ssr_target_candidates(target, &effective_main_fields)
                                .iter()
                                .map(|candidate| {
                                    shadow_path_for_project_path(
                                        candidate,
                                        &project_root,
                                        &project_root,
                                        shadow,
                                        shadow,
                                        node_modules_isolation_root,
                                    )
                                })
                                .any(|candidate| candidate.is_file());
                        if shadow_target.exists() || has_shadow_candidate {
                            shadow_target
                        } else {
                            target_path
                        }
                    }
                }
                Err(_) => {
                    // Issue #1668 part 2: a workspace-sibling alias target
                    // (outside project_root but inside first_party_root)
                    // resolves to its staged copy in the WORK mirror. Under
                    // exclusions it is the SOLE target (absence = exclusion);
                    // under an empty exclude, prefer the staged copy only when
                    // it (or a candidate) actually exists, else keep the real
                    // target — mirroring the project tier above. Non-sibling
                    // targets (plugin temp files, out-of-workspace paths) pass
                    // through unchanged.
                    if first_party_root != project_root.as_path()
                        && target_path.strip_prefix(first_party_root).is_ok()
                    {
                        let work_target = shadow_path_for_project_path(
                            &target_path,
                            &project_root,
                            first_party_root,
                            shadow,
                            work_root,
                            node_modules_isolation_root,
                        );
                        if exclusions_active {
                            work_target
                        } else {
                            let has_shadow_candidate =
                                concrete_ssr_target_candidates(target, &effective_main_fields)
                                    .iter()
                                    .map(|candidate| {
                                        shadow_path_for_project_path(
                                            candidate,
                                            &project_root,
                                            first_party_root,
                                            shadow,
                                            work_root,
                                            node_modules_isolation_root,
                                        )
                                    })
                                    .any(|candidate| candidate.is_file());
                            if work_target.exists() || has_shadow_candidate {
                                work_target
                            } else {
                                target_path
                            }
                        }
                    } else {
                        target_path
                    }
                }
            };
            let remapped = remapped.to_string_lossy().into_owned();
            (
                specifier.clone(),
                preserve_trailing_path_separator(remapped, target),
            )
        })
        .collect::<Vec<_>>();
    let effective_plugin_virtual_modules = input
        .plugin_virtual_modules
        .iter()
        .map(|(specifier, source)| {
            (
                specifier.clone(),
                remap_virtual_module_project_imports_to_shadow(
                    source,
                    &input.project_root,
                    first_party_root,
                    work_root,
                ),
            )
        })
        .collect::<Vec<_>>();
    let resolver_inputs = zfb_plugin_resolver::build_resolver_inputs(
        &effective_plugin_aliases,
        &effective_plugin_virtual_modules,
        shadow,
        &input.tsconfig_paths,
    )
    .context("bundler: failed materializing plugin resolver inputs")?;

    // Rewrite the synthetic tsconfig with the merged path map — THIS is the
    // tsconfig esbuild actually reads (step 4's earlier write is overwritten
    // here for every real build).
    //
    // ORDER MATTERS: rebase the user's `tsconfig_paths` to shadow-first
    // dual-targets FIRST, then merge the plugin/virtual entries on top.
    // `merge_into_tsconfig_paths` is `or_insert_with` (user-wins on key
    // collision), and plugin/virtual targets point at `<shadow>/.zfb-virtual-*`
    // temp files OUTSIDE `project_root`, so they never pass through the rebase
    // and stay single exact-match targets — preserving the exact-match
    // contract and the user-wins-on-collision merge semantics
    // (`bundler_exact_match_resolution`).
    //
    // BEHAVIOURAL DEPENDENCY (not recoverable from the code): esbuild treats a
    // `compilerOptions.paths` value that is an ARRAY as a try-in-order list —
    // it resolves each candidate, takes the FIRST one that maps to an existing
    // file, and a candidate that maps to no file is silently skipped (NOT an
    // error). The shadow-first dual-target relies on exactly this fallthrough:
    // the shadow copy (carrying the in-shadow transform) is tried first, and
    // the real-root target is the graceful fallback when the shadow has no
    // such file. (TypeScript/esbuild tsconfig paths-array semantics.)
    let mut merged_paths = rebase_tsconfig_paths_to_shadow_with_exclusions(
        &input.tsconfig_paths,
        &input.project_root,
        shadow,
        first_party_root,
        work_root,
        Some(bundle_exclude),
        node_modules_isolation_root,
    );
    zfb_plugin_resolver::merge_into_tsconfig_paths(
        &mut merged_paths,
        &resolver_inputs.paths_entries,
    );
    // Recreate the adapter to get `jsx_import_source` — cheap (adapters
    // are zero-state) and avoids threading another parameter
    // through `run_esbuild`. Stays in sync with step 4 above so a
    // future framework switch can't make the two writes diverge.
    let jsx_import_source = make_adapter(input.framework)
        .jsx_import_source()
        .to_string();
    write_synthetic_tsconfig(shadow, &merged_paths, &jsx_import_source)
        .context("bundler: failed rewriting synthetic tsconfig with plugin entries")?;

    // Virtual-module `--alias:<spec>=<tmp.mjs>` flags (#1263). esbuild does
    // NOT apply the synthetic tsconfig's `compilerOptions.paths` to a source
    // file resident under `node_modules` (`tsConfigForDir` returns nil for
    // `isInsideNodeModules` before consulting the `--tsconfig` override). A
    // preset route entrypoint installed under `node_modules` and pulled in by
    // `synthesize_static_overlay_module` therefore cannot resolve its
    // `virtual:*` imports through the tsconfig alone. `--alias` is not
    // node_modules-gated; it is exact-match-safe here because each virtual
    // target is a single `.mjs` file, so `virtual:foo/bar` remaps to
    // `<tmp>.mjs/bar` and fails (preserving #269). Empty for the
    // zero-virtual-module path, so the argv is unchanged there.
    for flag in resolver_inputs.virtual_module_alias_flags() {
        cmd.arg(flag);
    }

    // Mode defines are always emitted and deliberately independent of minify.
    // process.env.NODE_ENV is mode-driven and framework-agnostic.
    //
    // React's CJS entry (`react`, `react-dom/server`) reads
    // `process.env.NODE_ENV` at module-init time to pick its
    // production-vs-development code path. In the SSR/main bundle this
    // runs inside V8 with no Node `process` global, so without inlining
    // the value the bundle throws `ReferenceError: process is not defined`
    // before any React component can render. The islands *client* bundle
    // already defines this unconditionally (see
    // `zfb-islands/src/esbuild.rs::bundle_one_entry`); mirror it here so
    // both pipelines agree. Preact does not need it but the define is
    // harmless for Preact (esbuild just folds the unused branch away), so
    // it is not framework-gated — matching the client bundle's behaviour.
    for arg in bundle_mode_define_args(input.mode) {
        cmd.arg(arg);
    }

    // PUBLIC_-prefixed env vars only. Anything else is dropped server-
    // side and never reaches the bundle. Both common spellings are emitted,
    // except where an exact operator-authored `bundle.define` owns the same
    // expression. That explicit channel has higher precedence and is shared
    // by SSR and browser bundlers.
    for arg in public_env_define_args(&input.public_env_vars, &input.define_vars) {
        cmd.arg(arg);
    }

    // Operator-authored `bundle.define` expressions are already validated by
    // the config layer (including mode-key reservations) and remain raw.
    for arg in operator_define_args(&input.define_vars) {
        cmd.arg(arg);
    }

    for ext in &input.external {
        cmd.arg(format!("--external:{}", ext));
    }

    // Mark `node:*` builtins external so esbuild does not attempt to
    // resolve them when bundling for workerd / Cloudflare Workers. The
    // Worker runtime does not have filesystem access; any code path that
    // would call `node:fs` or `node:path` (e.g. the fallback branch in
    // `zfb/content` that runs outside a Worker context) is dead in
    // practice because `createPageRouter` installs the content snapshot
    // before any request is served. Marking them external is correct:
    // - it prevents the bundler from erroring on unresolvable built-ins,
    // - it keeps the dead code in the bundle tree-shaken by workerd at
    //   runtime (no actual fs calls ever execute inside the Worker).
    //
    // Pattern `node:*` is the canonical esbuild glob for all Node.js
    // built-in protocols. The explicit `--external:node:*` is NOT the
    // same as `--platform=node`; the bundle stays platform-neutral.
    cmd.arg("--external:node:*");

    // `--preserve-symlinks` has THREE separate activation paths:
    //
    // 1. **Vendored mode** (`node_modules_dir.is_some() &&
    //    node_modules_preserve_symlinks == true`) — `node_modules_dir`
    //    points at a synthetic tempdir whose contents live outside the
    //    project. Without `--preserve-symlinks` esbuild canonicalises
    //    symlinked source files back to their real paths, walks up
    //    looking for `node_modules`, and finds none (the injected vendor
    //    tree only exists at the shadow location). `--preserve-symlinks`
    //    keeps esbuild anchored at the shadow path.
    //
    // 2. **CSS Modules without a project node_modules** (fix #553) —
    //    `rewrite_css_modules_in_shadow` replaces the shadow's
    //    `.module.css` symlinks with real JS files, but `.tsx`/`.jsx`
    //    importers of those files remain symlinks. Without
    //    `--preserve-symlinks`, esbuild canonicalises each symlinked
    //    importer to its real source path and resolves
    //    `./x.module.css` from *there* — finding the original raw CSS
    //    rather than the rewritten JS in the shadow. With
    //    `--preserve-symlinks`, esbuild stays anchored at the shadow
    //    and resolves the relative import to the rewritten JS file.
    //
    // 3. **CSS Modules with a project node_modules but no tsconfig
    //    `paths`** (fix #553, corp's actual shape — `pnpm install`'d
    //    node_modules + plain relative imports of `*.module.css`,
    //    NO `compilerOptions.paths` in tsconfig). The .tsx
    //    canonicalisation problem above ALSO applies here; the only
    //    reason path 2's `is_none()` gate isn't enough is that corp
    //    has a real project `node_modules`. Adding
    //    `--preserve-symlinks` here is safe because the regression in
    //    #443/#450 that the gate originally protected against
    //    (`tsConfigForDir` returning early for workspace-package
    //    importers inside `node_modules`, so the project's
    //    `compilerOptions.paths` aliases stop applying for them)
    //    requires `paths` to even exist — without `paths` there is no
    //    alias resolution to break. The
    //    `bundler_workspace_pkg_alias` regression test exercises the
    //    `paths`-present branch and continues to pass because this
    //    `is_empty()` clause does NOT fire when paths exist.
    //
    // The three activation paths are encoded in
    // `esbuild_will_preserve_symlinks`; `bundle()` calls the SAME
    // predicate to derive `copy_mode` (the in-shadow source files must
    // be real copies precisely when esbuild will NOT preserve symlinks,
    // i.e. branch 4) so the two can never drift.
    if esbuild_will_preserve_symlinks(input) {
        cmd.arg("--preserve-symlinks");
    }

    cmd.arg(OsString::from(entry));

    let output =
        run_capturing(&mut cmd).with_context(|| format!("failed to spawn {}", bin.display()))?;
    // Drop `resolver_inputs` now — the subprocess has finished and
    // esbuild no longer needs the virtual-module `.mjs` temp files.
    // Explicit drop makes the lifetime intent visible; the
    // `NamedTempFile`s inside `_temp_files` delete themselves via
    // their Drop impl.
    drop(resolver_inputs);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let friendly = friendly_esbuild_error(
            stderr.trim(),
            shadow,
            &input.project_root,
            first_party_root,
            work_root,
        );
        bail!(
            "bundler: esbuild exited with status {}: {}",
            output.status,
            friendly
        );
    }
    Ok(())
}

/// Extracts `<specifier>` from an esbuild `Could not resolve "<specifier>"`
/// diagnostic line (the first line of a resolve-failure block). Returns
/// `None` for any other line.
fn could_not_resolve_specifier(line: &str) -> Option<&str> {
    const MARKER: &str = "Could not resolve \"";
    let start = line.find(MARKER)? + MARKER.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// Extracts the file-path portion of an esbuild source-location line, e.g.
/// `"    pages/foo.tsx:1:19:"` -> `Some("pages/foo.tsx")`.
///
/// Esbuild's location lines are `<indent><path>:<line>:<col>:`; a POSIX
/// path has no other colons, so splitting from the right by `:` twice
/// isolates the path. Windows drive-letter paths (`C:\...`) are not
/// handled — this is a diagnostic-message nicety, not a resolution-
/// affecting code path, and zfb has no CI coverage running the Rust suite
/// on Windows yet (see the T3 cutover manifest).
fn esbuild_location_path(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let without_trailing_colon = trimmed.strip_suffix(':')?;
    let mut parts = without_trailing_colon.rsplitn(3, ':');
    let col = parts.next()?;
    let row = parts.next()?;
    let path = parts.next()?;
    if path.is_empty() || col.parse::<u32>().is_err() || row.parse::<u32>().is_err() {
        return None;
    }
    Some(path)
}

/// Post-processes esbuild's captured stderr from `run_esbuild`'s failure
/// branch so a build failure is actionable instead of naming the ephemeral
/// shadow-copy tempdir the user never created (#1385 pt.2 / issue #1388;
/// extended for the workspace tier by issue #1839).
///
/// This is **diagnose-only** — the owner decision recorded on the #1386
/// epic is that a relative import which escapes the project root under
/// shadow-copy bundling stays unsupported; this function only changes the
/// REPORTED message, never resolution behaviour.
///
/// Two staged roots can appear in esbuild's raw stderr, narrowest first:
///
/// - `shadow` — the project mirror (`work_root` itself outside a
///   workspace, or `work_root/<workspace_rel>` inside one — see
///   `run_esbuild`'s doc comment) — maps back to the real `project_root`.
/// - `work_root` — the wholesale-mirrored first-party workspace root a
///   claimed sibling package's own source is staged under — maps back to
///   the real `first_party_root`.
///
/// `shadow` is always nested *under* `work_root` (or equal to it, outside a
/// workspace), so every prefix check below tries `shadow` FIRST:
/// longest-prefix-first. Checking `work_root` first would also match paths
/// that are really under the narrower `shadow`, and report them relative to
/// the coarser `first_party_root` instead of the caller's own
/// (possibly differently-spelled — e.g. not yet lexically normalized)
/// `project_root`.
///
/// Two independent passes:
///
/// 1. **Defensive catch-all** — any literal absolute `shadow` or
///    `work_root` path embedded in the text (e.g. a malformed-tsconfig
///    diagnostic can echo the `--tsconfig=<shadow>/…` flag verbatim) is
///    rewritten to the matching real root, longest-prefix first, so no
///    ephemeral tempdir path ever reaches the user.
/// 2. **Escape detection** — for every `Could not resolve "<specifier>"`
///    failure, first place the *importer* in exactly one scope: `project`
///    (staged under `shadow`) or `workspace` (staged only under the wider
///    `work_root` — a wholesale-mirrored sibling's own source). That
///    scope's OWN staged root is the only boundary its relative imports
///    (`./` or `../`) are allowed to stay inside — a project importer's
///    import landing inside `work_root` but outside `shadow` is STILL an
///    escape (the pre-existing #1386 decision: a plain relative import
///    crossing `project_root` is unsupported regardless of whether the
///    workspace happens to mirror that location too; the sibling mirror is
///    reached only via tsconfig aliases / bare imports, never a relative
///    walk across the project boundary). When the target — joined
///    lexically against the reported importer's directory — falls outside
///    that boundary, append a note naming the importer's REAL source path
///    (under `project_root` for a project importer, under
///    `first_party_root` for a workspace-sibling one), explaining the
///    matching (project- or workspace-scoped) shadow-copy build boundary,
///    and pointing at the package-specifier + wildcard-`exports`
///    workaround. The join is purely lexical (string arithmetic, no
///    filesystem access) — it does not matter whether the target actually
///    exists. `run_esbuild` passes `--log-limit=0` so esbuild's own default
///    10-message cap never truncates the input here, meaning every
///    offending block in stderr is walked and named, not just the first
///    batch.
///
/// Windows note: `esbuild_location_path` (used to parse each offender's
/// source location) does not handle drive-letter paths (`C:\...`), so the
/// "never leak a tempdir path" guarantee below is only verified for the
/// POSIX-style paths esbuild reports on the platforms zfb's Rust suite
/// actually runs on (ubuntu/macOS — see the T3 cutover manifest); it is not
/// exercised on Windows.
fn friendly_esbuild_error(
    stderr: &str,
    shadow: &Path,
    project_root: &Path,
    first_party_root: &Path,
    work_root: &Path,
) -> String {
    // Longest-prefix-first (see doc comment above): `shadow` before
    // `work_root`. Outside a workspace the two pairs are identical, so the
    // second replace is a harmless no-op on already-rewritten text.
    let mut out = stderr.to_string();
    for (staged, real) in [(shadow, project_root), (work_root, first_party_root)] {
        let staged_display = staged.to_string_lossy();
        if !staged_display.is_empty() {
            out = out.replace(staged_display.as_ref(), &real.to_string_lossy());
        }
    }

    let lines: Vec<&str> = stderr.lines().collect();
    let mut notes: Vec<String> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let Some(specifier) = could_not_resolve_specifier(line) else {
            continue;
        };
        if !(specifier.starts_with("./") || specifier.starts_with("../")) {
            // Bare/package specifiers (`preact`, `@scope/pkg`) are never a
            // shadow-root escape — nothing to annotate.
            continue;
        }
        let Some(location_line) = lines[idx + 1..].iter().find(|l| !l.trim().is_empty()) else {
            continue;
        };
        let Some(importer_rel) = esbuild_location_path(location_line) else {
            continue;
        };
        let importer_abs = normalize_path_lexical(&shadow.join(importer_rel));
        let Some(importer_dir) = importer_abs.parent() else {
            continue;
        };
        let target = normalize_path_lexical(&importer_dir.join(specifier));

        // Place the importer in exactly one scope FIRST, longest-prefix
        // (narrower `shadow`) before the wider `work_root` it is nested
        // under — see the doc comment above. That scope's own staged root
        // (never the other one) is the boundary THIS import must stay
        // inside.
        let (scope, staged_boundary, real_importer, real_root): (
            &'static str,
            &Path,
            PathBuf,
            &Path,
        ) = if let Ok(rel) = importer_abs.strip_prefix(shadow) {
            ("project", shadow, project_root.join(rel), project_root)
        } else if let Ok(rel) = importer_abs.strip_prefix(work_root) {
            // A wholesale-mirrored workspace-sibling importer (staged
            // under `work_root`, outside the narrower `shadow`) — map
            // through `first_party_root` so its real live-tree path is
            // named instead of the ephemeral work-mirror one.
            (
                "workspace",
                work_root,
                first_party_root.join(rel),
                first_party_root,
            )
        } else {
            // Already outside every staged root (can happen when
            // `--preserve-symlinks` is off and the importer is a
            // symlink whose realpath esbuild reports) — that path IS
            // the real one already; treat it as project-scoped, the
            // common case this fallback exists for.
            ("project", shadow, importer_abs.clone(), project_root)
        };

        if target.strip_prefix(staged_boundary).is_ok() {
            // Stays inside the importer's OWN staged boundary (e.g.
            // `../sibling/file.ts` from a project page resolving to a
            // valid project-relative location, or a workspace-sibling's
            // own import resolving elsewhere inside the wholesale work
            // mirror) — not an escape, so the raw esbuild message is left
            // to speak for itself. A project importer's target landing
            // inside `work_root` but outside `shadow` is deliberately NOT
            // matched here (see the doc comment above) — it still falls
            // through to the note below.
            continue;
        }
        notes.push(format!(
            "zfb: \"{specifier}\" (imported from {}) escapes the {scope} \
             root's shadow-copy build boundary. zfb bundles from a \
             temporary copy of {}; a relative import that walks above the \
             {scope} root cannot be followed there. Move the target inside \
             the {scope}, or expose it as a package import instead: add a \
             wildcard `exports` entry to the target package's \
             `package.json` (e.g. `\"./src/*\": \"./src/*\"`) and import it \
             by package specifier (e.g. `@scope/pkg/src/button.ts`) — \
             `node_modules` IS included in the shadow copy.",
            real_importer.display(),
            real_root.display(),
        ));
    }

    if !notes.is_empty() {
        out.push_str("\n\n");
        out.push_str(&notes.join("\n\n"));
    }
    out
}

fn resolve_esbuild_binary(explicit: Option<&Path>) -> Result<PathBuf> {
    let (_handle, path) = resolve_esbuild_binary_with_env(
        explicit,
        |name| std::env::var_os(name),
        None::<fn() -> Option<(tempfile::TempDir, PathBuf)>>,
        None,
    )?;
    Ok(path)
}

/// Shared esbuild binary resolver used by both the bundler and the config
/// loader. Home crate: `zfb-build` (lowest crate that both consumers depend
/// on; `zfb-types` is excluded because it is a zero-dep leaf).
///
/// ## Lookup order (documented superset)
///
/// 1. **Explicit path** (`explicit`) — if `Some`, validated and returned
///    immediately; an absent file is a hard error.
/// 2. **`ZFB_ESBUILD_BIN` env var** — read via `env_getter("ZFB_ESBUILD_BIN")`.
///    Injected as a closure so tests can drive this tier without mutating
///    `std::env` (which is `unsafe` in a multi-threaded test runner under
///    Rust 2024).
/// 3. **Embedded extraction** (`embedded_getter`) — optional callback tried
///    *before* the workspace slot. The config-loader caller (`zfb` crate)
///    passes `Some(|| crate::render_pipeline::embedded_binary("esbuild"))` so
///    a `cargo install`-ed binary (which has no workspace) still resolves.
///    The bundler passes `None` here because it has no access to the
///    `EMBEDDED_VENDOR` snapshot that lives in the `zfb` crate.
/// 4. **Workspace slot** — `slot_override.unwrap_or(DEFAULT_ESBUILD_SLOT)`.
///    `slot_override` is an escape hatch for unit tests that need to point the
///    slot at a tempdir without chdir-ing.
///
/// ## Return value
///
/// `(Option<tempfile::TempDir>, PathBuf)` — the `TempDir` is `Some` only when
/// the embedded extraction tier was taken. The caller **must** hold the handle
/// alive for the lifetime of any subprocess that references the returned
/// `PathBuf`; dropping the handle removes the tempdir and the binary.
pub fn resolve_esbuild_binary_with_env<F, E>(
    explicit: Option<&Path>,
    env_getter: F,
    embedded_getter: Option<E>,
    slot_override: Option<&Path>,
) -> Result<(Option<tempfile::TempDir>, PathBuf)>
where
    F: Fn(&str) -> Option<std::ffi::OsString>,
    E: FnOnce() -> Option<(tempfile::TempDir, PathBuf)>,
{
    // Tier 1: explicit path override.
    if let Some(p) = explicit {
        if !p.exists() {
            bail!(
                "bundler: esbuild binary not found at explicit path {}",
                p.display()
            );
        }
        return Ok((None, p.to_path_buf()));
    }
    // Tier 2: ZFB_ESBUILD_BIN env var. An empty value (set but blank) is
    // treated the same as unset, mirroring the build-time override
    // contract in `crates/zfb/build.rs` / `BUILDING.md` — otherwise a
    // blank env var that build.rs treats as "no override" would still
    // trip this tier and fail to resolve a binary at runtime.
    if let Some(env) = env_getter("ZFB_ESBUILD_BIN").filter(|v| !v.is_empty()) {
        let p = PathBuf::from(env);
        if !p.exists() {
            bail!(
                "bundler: esbuild binary not found at ZFB_ESBUILD_BIN={}",
                p.display()
            );
        }
        return Ok((None, p));
    }
    // Tier 3: embedded extraction (config-loader caller only; None for bundler).
    if let Some(getter) = embedded_getter {
        if let Some((handle, path)) = getter() {
            return Ok((Some(handle), path));
        }
        // getter returned None → fall through to the workspace slot.
    }
    // Tier 4: workspace-relative staging slot.
    let slot = slot_override
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(DEFAULT_ESBUILD_SLOT));
    if !slot.exists() {
        return Err(anyhow!(
            "bundler: esbuild binary not found at default slot {}. \
             Evaluating a `zfb.config.ts` needs an esbuild CLI binary: set \
             ZFB_ESBUILD_BIN to one (or, in a workspace checkout, stage the \
             binary at that slot path). If you are embedding zfb-server as a \
             library, prefer shipping a `zfb.config.json` instead — the JSON \
             config path needs no esbuild at all.",
            slot.display(),
        ));
    }
    Ok((None, slot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zfb_test_utils::locate_esbuild as locate_real_esbuild;

    // --- #1645 staged-dependency-view seed predicates ---

    fn write_package_staging_workspace() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let host = workspace.join("apps/host");
        let sibling = workspace.join("packages/ui");
        fs::create_dir_all(host.join("pages")).unwrap();
        fs::create_dir_all(sibling.join("src")).unwrap();
        fs::write(
            workspace.join("pnpm-workspace.yaml"),
            "packages:\n  - 'apps/*'\n  - 'packages/*'\n",
        )
        .unwrap();
        fs::write(
            sibling.join("package.json"),
            r#"{"name":"@acme/ui","exports":{"./button":"./src/button.ts"}}"#,
        )
        .unwrap();
        let importer = host.join("pages/index.tsx");
        fs::write(&importer, "export default null;\n").unwrap();
        (temp, host, sibling, importer)
    }

    #[test]
    fn workspace_package_staging_accepts_only_runtime_manifest_sections() {
        let (_temp, host, sibling, importer) = write_package_staging_workspace();
        for section in ["dependencies", "optionalDependencies", "peerDependencies"] {
            fs::write(
                host.join("package.json"),
                format!(r#"{{"name":"host","{section}":{{"@acme/ui":"workspace:*"}}}}"#),
            )
            .unwrap();
            assert!(workspace_package_source_is_eligible(
                &importer, "@acme/ui", &sibling, &host,
            ));
        }
        fs::write(
            host.join("package.json"),
            r#"{"name":"host","devDependencies":{"@acme/ui":"workspace:*"}}"#,
        )
        .unwrap();
        assert!(!workspace_package_source_is_eligible(
            &importer, "@acme/ui", &sibling, &host,
        ));
    }

    #[test]
    fn workspace_package_staging_rejects_unclaimed_mismatched_and_outside_targets() {
        let (temp, host, sibling, importer) = write_package_staging_workspace();
        fs::write(
            host.join("package.json"),
            r#"{"name":"host","dependencies":{"@acme/ui":"workspace:*"}}"#,
        )
        .unwrap();

        let unclaimed = temp.path().join("workspace/unclaimed/ui");
        fs::create_dir_all(&unclaimed).unwrap();
        fs::write(unclaimed.join("package.json"), r#"{"name":"@acme/ui"}"#).unwrap();
        assert!(!workspace_package_source_is_eligible(
            &importer, "@acme/ui", &unclaimed, &host,
        ));

        let invalid_subdirectory_target = sibling.join("src");
        assert!(!workspace_package_source_is_eligible(
            &importer,
            "@acme/ui",
            &invalid_subdirectory_target,
            &host,
        ));

        fs::write(sibling.join("package.json"), r#"{"name":"@acme/not-ui"}"#).unwrap();
        assert!(!workspace_package_source_is_eligible(
            &importer, "@acme/ui", &sibling, &host,
        ));

        let outside = temp.path().join("outside/ui");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("package.json"), r#"{"name":"@acme/ui"}"#).unwrap();
        assert!(!workspace_package_source_is_eligible(
            &importer, "@acme/ui", &outside, &host,
        ));

        let outside_node_modules = temp.path().join("outside/node_modules/@acme/ui");
        fs::create_dir_all(&outside_node_modules).unwrap();
        fs::write(
            outside_node_modules.join("package.json"),
            r#"{"name":"@acme/ui"}"#,
        )
        .unwrap();
        assert!(!workspace_package_source_is_eligible(
            &importer,
            "@acme/ui",
            &outside_node_modules,
            &host,
        ));

        fs::write(sibling.join("package.json"), r#"{"name":"@acme/ui"}"#).unwrap();
        fs::write(host.join("package.json"), r#"{"name":"host"}"#).unwrap();
        assert!(!workspace_package_source_is_eligible(
            &importer, "@acme/ui", &sibling, &host,
        ));
    }

    #[test]
    fn bare_package_owner_parsing_handles_scoped_unscoped_and_subpaths() {
        assert_eq!(
            bare_package_name("toolkit/feature").as_deref(),
            Some("toolkit")
        );
        assert_eq!(
            bare_package_name("@acme/ui/button").as_deref(),
            Some("@acme/ui")
        );
        assert_eq!(bare_package_name("./relative"), None);
    }

    #[test]
    fn workspace_package_staging_keeps_ordinary_npm_dependency_behavior() {
        let (temp, host, _sibling, importer) = write_package_staging_workspace();
        let ordinary = temp
            .path()
            .join("workspace/node_modules/.pnpm/left-pad@1/node_modules/left-pad");
        fs::create_dir_all(&ordinary).unwrap();
        assert!(workspace_package_source_is_eligible(
            &importer, "left-pad", &ordinary, &host,
        ));
    }

    #[test]
    fn specifier_matches_alias_key_exact_and_anchored_wildcard() {
        assert!(specifier_matches_alias_key(
            "collision-alias",
            "collision-alias"
        ));
        // A non-wildcard key matches exactly, never as a prefix.
        assert!(!specifier_matches_alias_key(
            "collision-alias/x",
            "collision-alias"
        ));
        // Prefix/suffix-anchored wildcards.
        assert!(specifier_matches_alias_key(
            "collision-wildcard/secret",
            "collision-wildcard/*"
        ));
        assert!(!specifier_matches_alias_key(
            "other/secret",
            "collision-wildcard/*"
        ));
        assert!(specifier_matches_alias_key("@/foo", "@/*"));
        // `@/*` must NOT claim an unrelated scoped package.
        assert!(!specifier_matches_alias_key("@takazudo/zfb-runtime", "@/*"));
    }

    #[test]
    fn relative_node_modules_specifier_resolves_owning_package() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let package_root = root.join("node_modules/@fixture/routes");
        std::fs::create_dir_all(package_root.join("src")).unwrap();
        let importer = root.join("pages/docs/reference.tsx");

        let resolved = node_modules_dependency_from_path_specifier(
            "../../node_modules/@fixture/routes/src/page.tsx",
            &importer,
            root,
        );
        assert_eq!(
            resolved,
            Some(("@fixture/routes".to_string(), package_root))
        );
    }

    #[test]
    fn specifier_matches_alias_key_universal_catch_all_never_claims() {
        // A bare `*` is a fall-back alias, not a specific claim (#1645): it must
        // never suppress node_modules staging.
        assert!(!specifier_matches_alias_key(
            "@takazudo/zfb-adapter-cloudflare",
            "*"
        ));
        assert!(!specifier_matches_alias_key("anything/at/all", "*"));
    }

    #[test]
    fn specifier_is_alias_claimed_over_key_set() {
        let keys: BTreeSet<String> = [
            "@/*".to_string(),
            "collision-alias".to_string(),
            "*".to_string(),
        ]
        .into_iter()
        .collect();
        assert!(specifier_is_alias_claimed("@/foo", &keys));
        assert!(specifier_is_alias_claimed("collision-alias", &keys));
        // The catch-all `*` in the set must not make an ordinary package claimed.
        assert!(!specifier_is_alias_claimed(
            "@takazudo/zfb-adapter-cloudflare",
            &keys
        ));
    }

    #[test]
    fn package_namespace_is_alias_claimed_over_key_shapes() {
        let claimed = |package: &str, key: &str| {
            package_namespace_is_alias_claimed(package, &BTreeSet::from([key.to_string()]))
        };
        assert!(claimed("collision-alias", "collision-alias"));
        assert!(claimed("collision-alias", "collision-alias/subpath"));
        assert!(claimed("collision-alias", "collision-alias/*"));
        assert!(claimed("@scope/package", "@scope/*"));
        assert!(claimed("any-package", "*/generated"));
        assert!(!claimed("unrelated", "collision-alias/*"));
        assert!(!claimed("unrelated", "*"));
    }

    #[test]
    fn package_is_external_matches_whole_package_only() {
        let external = vec![
            "preact".to_string(),
            "@takazudo/zfb-runtime".to_string(),
            "@scope/*".to_string(),
        ];
        assert!(package_is_external("preact", &external));
        assert!(package_is_external("@takazudo/zfb-runtime", &external));
        // A namespace wildcard external matches every package in the namespace.
        assert!(package_is_external("@scope/anything", &external));
        assert!(!package_is_external("not-external", &external));
        // `--external:preact` must NOT externalize the distinct `preact-*` package.
        assert!(!package_is_external("preact-render-to-string", &external));
    }

    fn shadow_env(
        temp_dir: PathBuf,
        xdg_cache_home: Option<PathBuf>,
        home: Option<PathBuf>,
        local_app_data: Option<PathBuf>,
    ) -> ShadowParentEnv {
        ShadowParentEnv {
            temp_dir,
            xdg_cache_home: xdg_cache_home.map(PathBuf::into_os_string),
            home: home.map(PathBuf::into_os_string),
            local_app_data: local_app_data.map(PathBuf::into_os_string),
        }
    }

    #[test]
    fn shadow_parent_uses_system_temp_when_outside_project() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let temp_dir = tmp.path().join("outside-temp");
        fs::create_dir_all(&project).unwrap();

        let parent =
            shadow_parent_dir_with_env(&project, &shadow_env(temp_dir.clone(), None, None, None))
                .unwrap();

        assert_eq!(parent, fs::canonicalize(temp_dir).unwrap());
    }

    #[test]
    fn shadow_parent_uses_xdg_when_temp_is_inside_project() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let temp_dir = project.join(".tmp");
        let xdg = tmp.path().join("cache");
        fs::create_dir_all(&project).unwrap();

        let parent = shadow_parent_dir_with_env(
            &project,
            &shadow_env(temp_dir, Some(xdg.clone()), None, None),
        )
        .unwrap();

        assert_eq!(parent, fs::canonicalize(xdg.join("zfb")).unwrap());
        assert!(parent.is_dir(), "selected cache parent must be created");
    }

    #[cfg(not(windows))]
    #[test]
    fn shadow_parent_uses_home_cache_when_temp_is_inside_and_xdg_unset() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let temp_dir = project.join(".tmp");
        let home = tmp.path().join("home");
        fs::create_dir_all(&project).unwrap();

        let parent = shadow_parent_dir_with_env(
            &project,
            &shadow_env(temp_dir, None, Some(home.clone()), None),
        )
        .unwrap();

        assert_eq!(
            parent,
            fs::canonicalize(home.join(".cache").join("zfb")).unwrap()
        );
        assert!(
            parent.is_dir(),
            "selected HOME cache parent must be created"
        );
    }

    #[cfg(windows)]
    #[test]
    fn shadow_parent_uses_local_app_data_when_temp_is_inside_and_xdg_unset() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let temp_dir = project.join(".tmp");
        let local_app_data = tmp.path().join("LocalAppData");
        fs::create_dir_all(&project).unwrap();

        let parent = shadow_parent_dir_with_env(
            &project,
            &shadow_env(temp_dir, None, None, Some(local_app_data.clone())),
        )
        .unwrap();

        assert_eq!(
            parent,
            fs::canonicalize(local_app_data.join("zfb")).unwrap()
        );
        assert!(
            parent.is_dir(),
            "selected LOCALAPPDATA cache parent must be created"
        );
    }

    #[test]
    fn shadow_parent_errors_when_all_candidates_are_project_local_or_unresolvable() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let temp_dir = project.join(".tmp");
        let xdg = project.join(".xdg-cache");

        #[cfg(not(windows))]
        let env = shadow_env(temp_dir, Some(xdg), Some(project.join("home")), None);
        #[cfg(windows)]
        let env = shadow_env(
            temp_dir,
            Some(xdg),
            None,
            Some(project.join("LocalAppData")),
        );

        let err = shadow_parent_dir_with_env(&project, &env).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("outside project root"), "{msg}");
        assert!(msg.contains("project root"), "{msg}");
    }

    #[cfg(unix)]
    #[test]
    fn shadow_parent_rejects_symlinked_cache_resolving_into_project() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        fs::create_dir_all(project.join("cache-target")).unwrap();
        let temp_dir = project.join(".tmp");
        let xdg_link = tmp.path().join("xdg-link");
        std::os::unix::fs::symlink(project.join("cache-target"), &xdg_link).unwrap();

        let err =
            shadow_parent_dir_with_env(&project, &shadow_env(temp_dir, Some(xdg_link), None, None))
                .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("resolves inside project root"), "{msg}");
    }

    #[test]
    fn shadow_invariant_validation_rejects_project_local_shadow_path() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let shadow = project.join(".zfb-shadow");
        fs::create_dir_all(&shadow).unwrap();

        let err = ensure_shadow_path_outside_project(&project, &shadow, "bundler shadow root")
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("bundler invariant violation"), "{msg}");
        assert!(msg.contains("bundler shadow root"), "{msg}");
    }

    #[test]
    fn rebase_tsconfig_paths_dual_target_under_root_passthrough_external() {
        let root = Path::new("/proj");
        let shadow = Path::new("/tmp/shadowX");
        let mut paths: BTreeMap<String, Vec<String>> = BTreeMap::new();
        // whole-root `@/* -> /proj/*` (baseUrl "." — the most common alias shape;
        // empty `rel` must NOT produce a `<shadow>//*` double-slash target).
        paths.insert("@/*".to_string(), vec!["/proj/*".to_string()]);
        // subdir alias
        paths.insert("@lib/*".to_string(), vec!["/proj/src/lib/*".to_string()]);
        // single-file bare-specifier remap (no `/*` suffix)
        paths.insert(
            "msw".to_string(),
            vec!["/proj/src/mocks/msw.ts".to_string()],
        );
        // external target (not under project_root) — must pass through unchanged
        paths.insert("@ext/*".to_string(), vec!["/other/pkg/*".to_string()]);

        let out = rebase_tsconfig_paths_to_shadow(&paths, root, shadow);

        // whole-root: clean `<shadow>/*` shadow-first, NOT `<shadow>//*`
        assert_eq!(
            out["@/*"],
            vec!["/tmp/shadowX/*".to_string(), "/proj/*".to_string()]
        );
        assert!(
            !out["@/*"][0].contains("//"),
            "bare-root rebase must not double-slash: {:?}",
            out["@/*"]
        );
        // subdir + single-file: shadow-first dual-target
        assert_eq!(
            out["@lib/*"],
            vec![
                "/tmp/shadowX/src/lib/*".to_string(),
                "/proj/src/lib/*".to_string()
            ]
        );
        assert_eq!(
            out["msw"],
            vec![
                "/tmp/shadowX/src/mocks/msw.ts".to_string(),
                "/proj/src/mocks/msw.ts".to_string()
            ]
        );
        // external: single target, unchanged (keeps bundler_exact_match_resolution semantics)
        assert_eq!(out["@ext/*"], vec!["/other/pkg/*".to_string()]);
    }

    #[test]
    fn exact_resolution_candidates_cover_contextual_orders_and_absolute_trailing_child() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let node_target = root.join("node_modules/probe/value");
        fs::create_dir_all(node_target.parent().unwrap()).unwrap();
        fs::write(node_target.with_extension("js"), "export default 'js';\n").unwrap();
        fs::write(node_target.with_extension("ts"), "export default 'ts';\n").unwrap();

        let node_candidates = concrete_ssr_target_candidates(&node_target.to_string_lossy(), &[]);
        assert!(node_candidates.contains(&node_target.with_extension("js")));
        assert!(node_candidates.contains(&node_target.with_extension("ts")));

        let trailing = root.join("trailing");
        fs::create_dir_all(&trailing).unwrap();
        fs::write(
            trailing.join("trailing.css"),
            ".TRAILING_CHILD_CANDIDATE {}\n",
        )
        .unwrap();
        fs::write(
            root.join("trailing.js"),
            "export default 'outside-sibling';\n",
        )
        .unwrap();
        let trailing_candidates =
            concrete_ssr_target_candidates(&format!("{}/", trailing.display()), &["main"]);
        assert!(trailing_candidates.contains(&trailing.join("trailing.css")));
        assert!(!trailing_candidates.contains(&root.join("trailing.js")));
    }

    #[test]
    fn exact_resolution_rooted_main_and_file_alias_package_staging() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let package = root.join(".hidden-package");
        fs::create_dir_all(package.join("dist")).unwrap();
        fs::write(
            package.join("package.json"),
            r##"{"main":"/secret.js","imports":{"#internal":"./dist/internal.js"}}"##,
        )
        .unwrap();
        fs::write(
            package.join("secret.js"),
            "export default 'package-local';\n",
        )
        .unwrap();
        fs::write(
            package.join("dist/internal.js"),
            "export default 'internal';\n",
        )
        .unwrap();
        fs::write(root.join("secret.js"), "export default 'root-decoy';\n").unwrap();

        let candidates = concrete_ssr_target_candidates(&package.to_string_lossy(), &["main"]);
        assert!(candidates.contains(&package.join("secret.js")));
        assert!(!candidates.contains(&root.join("secret.js")));

        let matcher = BundleExcludeMatcher::new(&[]).unwrap();
        let mut files = BTreeSet::new();
        let mut dirs = BTreeSet::new();
        let entry = package.join("secret.js");
        plan_concrete_target_staging(
            &entry.to_string_lossy(),
            root,
            &matcher,
            &["main"],
            true,
            &mut files,
            &mut dirs,
        );
        assert!(files.contains(&entry));
        assert!(dirs.contains(&package));
    }

    #[cfg(not(windows))]
    #[test]
    fn exact_resolution_unix_backslash_is_not_a_path_separator() {
        assert!(!has_trailing_path_separator("literal\\"));
        assert_eq!(
            preserve_trailing_path_separator("shadow-target".to_string(), "literal\\"),
            "shadow-target"
        );
    }

    #[test]
    fn exclusion_active_tsconfig_rebase_is_uniformly_shadow_only() {
        // THE SWITCH (#1557): with any `bundle.exclude` active, every under-root
        // tsconfig target rewrites to a SINGLE shadow-only target — no
        // `[shadow, real]` dual-target array, no per-target 3-state
        // classification, no guard-child routing, no provably-disjoint carve-out.
        // Exclusion = absence from the shadow; esbuild resolves inside the shadow
        // and a missing target simply fails to resolve. External targets pass
        // through unchanged.
        let shadow = Path::new("/tmp/shadowSwitch");
        let root = Path::new("/proj");
        let matcher = BundleExcludeMatcher::new(&["src/secret.ts".to_string()]).unwrap();

        let mut paths: BTreeMap<String, Vec<String>> = BTreeMap::new();
        // whole-root wildcard (baseUrl "." — empty rel must NOT double-slash).
        paths.insert("@/*".to_string(), vec!["/proj/*".to_string()]);
        // the excluded exact file itself.
        paths.insert(
            "secret".to_string(),
            vec!["/proj/src/secret.ts".to_string()],
        );
        // an UNRELATED wildcard that does not overlap the exclusion. Under the
        // old "provably disjoint keeps its real fallback" heuristic this kept a
        // second real-abs element; the switch drops it — this is the conscious
        // flip.
        paths.insert(
            "unrelated/*".to_string(),
            vec!["/proj/unmirrored/*".to_string()],
        );
        // a directory target with a trailing separator.
        paths.insert("dir".to_string(), vec!["/proj/pkg/".to_string()]);
        // an external target (not under project_root) — passes through unchanged.
        paths.insert("@ext/*".to_string(), vec!["/other/pkg/*".to_string()]);

        let out = rebase_tsconfig_paths_to_shadow_with_exclusions(
            &paths,
            root,
            shadow,
            // Sibling tier inert (first_party/work == project/shadow).
            root,
            shadow,
            Some(&matcher),
            None,
        );

        assert_eq!(out["@/*"], vec!["/tmp/shadowSwitch/*".to_string()]);
        assert!(
            !out["@/*"][0].contains("//"),
            "bare-root rebase must not double-slash: {:?}",
            out["@/*"]
        );
        assert_eq!(
            out["secret"],
            vec!["/tmp/shadowSwitch/src/secret.ts".to_string()],
            "the excluded exact target is shadow-only — no live-real fallback"
        );
        assert_eq!(
            out["unrelated/*"],
            vec!["/tmp/shadowSwitch/unmirrored/*".to_string()],
            "an unrelated wildcard also loses its real fallback under exclusions \
             (the provably-disjoint carve-out is deleted)"
        );
        assert_eq!(out["dir"], vec!["/tmp/shadowSwitch/pkg/".to_string()]);
        assert_eq!(
            out["@ext/*"],
            vec!["/other/pkg/*".to_string()],
            "external targets pass through unchanged"
        );
    }

    #[test]
    fn rebase_tsconfig_paths_sibling_tier_maps_to_work_mirror() {
        // Issue #1668 part 2: a workspace-sibling alias target (outside
        // project_root, inside first_party_root) rebases to the WORK mirror,
        // obeying the same shadow-first-vs-shadow-only rule as the project tier.
        let project = Path::new("/ws/sub-packages/host");
        let first_party = Path::new("/ws");
        let shadow = Path::new("/work/sub-packages/host");
        let work = Path::new("/work");
        let mut paths: BTreeMap<String, Vec<String>> = BTreeMap::new();
        // project-local alias → project-mirror-first dual-target
        paths.insert(
            "@/*".to_string(),
            vec!["/ws/sub-packages/host/src/*".to_string()],
        );
        // workspace-sibling alias → WORK-mirror-first dual-target
        paths.insert(
            "@shared/*".to_string(),
            vec!["/ws/lib/shared/*".to_string()],
        );
        // beyond the workspace entirely → unchanged
        paths.insert("@ext/*".to_string(), vec!["/elsewhere/pkg/*".to_string()]);

        // Empty exclude: dual-targets (staged-first, real fallback).
        let empty = BundleExcludeMatcher::new(&[]).unwrap();
        let out = rebase_tsconfig_paths_to_shadow_with_exclusions(
            &paths,
            project,
            shadow,
            first_party,
            work,
            Some(&empty),
            None,
        );
        assert_eq!(
            out["@/*"],
            vec![
                "/work/sub-packages/host/src/*".to_string(),
                "/ws/sub-packages/host/src/*".to_string(),
            ]
        );
        assert_eq!(
            out["@shared/*"],
            vec![
                "/work/lib/shared/*".to_string(),
                "/ws/lib/shared/*".to_string(),
            ],
            "a workspace-sibling alias rebases work-mirror-first with a real fallback"
        );
        assert_eq!(out["@ext/*"], vec!["/elsewhere/pkg/*".to_string()]);

        // Active exclusions: staged-only (no real fallback) for BOTH tiers.
        let excluded = BundleExcludeMatcher::new(&["**/*.secret".to_string()]).unwrap();
        let out = rebase_tsconfig_paths_to_shadow_with_exclusions(
            &paths,
            project,
            shadow,
            first_party,
            work,
            Some(&excluded),
            None,
        );
        assert_eq!(
            out["@/*"],
            vec!["/work/sub-packages/host/src/*".to_string()]
        );
        assert_eq!(
            out["@shared/*"],
            vec!["/work/lib/shared/*".to_string()],
            "under exclusions a workspace-sibling alias is work-mirror-only (no live-real fallback)"
        );
        assert_eq!(out["@ext/*"], vec!["/elsewhere/pkg/*".to_string()]);
    }

    #[test]
    fn rebase_tsconfig_paths_workspace_root_target_maps_to_work_mirror() {
        let project = Path::new("/ws/sub-packages/host");
        let first_party = Path::new("/ws");
        let shadow = Path::new("/work/sub-packages/host");
        let work = Path::new("/work");
        let paths = BTreeMap::from([("@/*".to_string(), vec!["/ws/*".to_string()])]);
        let empty = BundleExcludeMatcher::new(&[]).unwrap();
        let out = rebase_tsconfig_paths_to_shadow_with_exclusions(
            &paths,
            project,
            shadow,
            first_party,
            work,
            Some(&empty),
            None,
        );
        assert_eq!(out["@/*"], vec!["/work/*", "/ws/*"]);

        let excluded = BundleExcludeMatcher::new(&["**/*.secret".to_string()]).unwrap();
        let out = rebase_tsconfig_paths_to_shadow_with_exclusions(
            &paths,
            project,
            shadow,
            first_party,
            work,
            Some(&excluded),
            None,
        );
        assert_eq!(out["@/*"], vec!["/work/*"]);
    }

    // ── friendly_esbuild_error + its parsing helpers (#1388) ────────────────

    #[test]
    fn could_not_resolve_specifier_extracts_quoted_specifier() {
        assert_eq!(
            could_not_resolve_specifier(
                "\u{2718} [ERROR] Could not resolve \"../../outside/shared.ts\""
            ),
            Some("../../outside/shared.ts")
        );
        assert_eq!(could_not_resolve_specifier("some unrelated line"), None);
    }

    #[test]
    fn esbuild_location_path_extracts_path_before_line_col() {
        assert_eq!(
            esbuild_location_path("    pages/foo.tsx:1:19:"),
            Some("pages/foo.tsx")
        );
        // Not a location line: no trailing colon / non-numeric row-col.
        assert_eq!(esbuild_location_path("1 error"), None);
        assert_eq!(esbuild_location_path(""), None);
    }

    #[test]
    fn friendly_esbuild_error_annotates_escaping_relative_import() {
        let shadow = Path::new("/tmp/zfb-bundler-abc123");
        let project_root = Path::new("/Users/dev/my-project");
        // Exact shape captured from a real esbuild 0.27.3 run against a
        // shadow-copied fixture whose page imports
        // `../../outside/shared.ts` (two levels up from `pages/foo.tsx`
        // escapes the shadow root, matching the #1385 pt.2 repro shape).
        let stderr = "\u{2718} [ERROR] Could not resolve \"../../outside/shared.ts\"\n\n    pages/foo.tsx:1:19:\n      1 \u{2502} import Button from \"../../outside/shared.ts\";\n        \u{2575}                    ~~~~~~~~~~~~~~~~~~~~~~~~~\n\n1 error";

        // Non-workspace: `first_party_root == project_root`, `work_root ==
        // shadow` (see `run_esbuild`'s doc comment) — the sibling tier is
        // inert.
        let friendly = friendly_esbuild_error(stderr, shadow, project_root, project_root, shadow);

        // Names the REAL source path, not a tempdir.
        assert!(
            friendly.contains("/Users/dev/my-project/pages/foo.tsx"),
            "should name the real project path: {friendly}"
        );
        // States the boundary rule.
        assert!(
            friendly.to_lowercase().contains("shadow-copy"),
            "should explain the shadow-copy boundary: {friendly}"
        );
        // Mentions the exports-map workaround.
        assert!(
            friendly.contains("exports"),
            "should mention the package.json exports workaround: {friendly}"
        );
        // Never leaks the shadow tempdir's own directory name.
        assert!(
            !friendly.contains("zfb-bundler-abc123"),
            "should not leak the shadow tempdir name: {friendly}"
        );
    }

    #[test]
    fn friendly_esbuild_error_leaves_non_escaping_resolve_failure_unannotated() {
        let shadow = Path::new("/tmp/zfb-bundler-abc123");
        let project_root = Path::new("/Users/dev/my-project");
        // `../sibling/file.ts` from `pages/foo.tsx` resolves to
        // `<root>/sibling/file.ts` — still inside the project root, so this
        // is an ordinary (non-escaping) unresolved import and must get no
        // boundary annotation.
        let stderr = "\u{2718} [ERROR] Could not resolve \"../sibling/file.ts\"\n\n    pages/foo.tsx:1:19:\n      1 \u{2502} import Button from \"../sibling/file.ts\";\n\n1 error";

        let friendly = friendly_esbuild_error(stderr, shadow, project_root, project_root, shadow);
        assert_eq!(
            friendly, stderr,
            "non-escaping resolve failures should pass through unchanged"
        );
    }

    #[test]
    fn friendly_esbuild_error_replaces_leaked_shadow_root_prefix() {
        let shadow = Path::new("/tmp/zfb-bundler-abc123");
        let project_root = Path::new("/Users/dev/my-project");
        let stderr = "some diagnostic mentioning /tmp/zfb-bundler-abc123/tsconfig.json directly";
        let friendly = friendly_esbuild_error(stderr, shadow, project_root, project_root, shadow);
        assert_eq!(
            friendly,
            "some diagnostic mentioning /Users/dev/my-project/tsconfig.json directly"
        );
    }

    #[test]
    fn friendly_esbuild_error_maps_work_root_prefixed_sibling_importer_to_first_party_root() {
        // Workspace shape: `shadow` (the narrower project mirror) is nested
        // under `work_root` (the wholesale-mirrored workspace root). A
        // claimed sibling package's own source is staged only under
        // `work_root`, outside `shadow` — e.g. `work_root/lib/shared/helper.ts`
        // when `shadow == work_root/sub-packages/host`. esbuild's cwd is
        // `shadow`, so it reports the importer as a relative path that
        // climbs OUT of shadow and back into the sibling.
        let work_root = Path::new("/tmp/zfb-bundler-abc123");
        let shadow = Path::new("/tmp/zfb-bundler-abc123/sub-packages/host");
        let project_root = Path::new("/workspace/sub-packages/host");
        let first_party_root = Path::new("/workspace");
        let stderr = "\u{2718} [ERROR] Could not resolve \"../../../outside/escaped.ts\"\n\n    ../../lib/shared/helper.ts:1:19:\n      1 \u{2502} import x from \"../../../outside/escaped.ts\";\n\n1 error";

        let friendly =
            friendly_esbuild_error(stderr, shadow, project_root, first_party_root, work_root);

        // Names the sibling's REAL live-tree path (mapped through
        // `first_party_root`), not the ephemeral `work_root` tempdir.
        assert!(
            friendly.contains("/workspace/lib/shared/helper.ts"),
            "should name the real workspace-sibling path: {friendly}"
        );
        assert!(
            !friendly.contains("zfb-bundler-abc123"),
            "should not leak the work_root tempdir name: {friendly}"
        );
        assert!(
            friendly.to_lowercase().contains("shadow-copy"),
            "should explain the shadow-copy boundary: {friendly}"
        );
    }

    #[test]
    fn friendly_esbuild_error_prefers_shadow_prefix_over_work_root_prefix() {
        // `shadow` is nested under `work_root`, so an importer that lives
        // INSIDE shadow also lexically satisfies a `work_root` prefix
        // check. `project_root` is deliberately NOT the lexical
        // `first_party_root.join(<workspace-rel>)` composition (it carries
        // a `.` component `first_party_root` doesn't normalize away in
        // this fabricated case) so the two mappings are texually
        // distinguishable — proving the longest-prefix-first order picks
        // the caller-supplied `project_root` (via `shadow`) rather than
        // deriving a path through `first_party_root` (via `work_root`).
        let work_root = Path::new("/tmp/zfb-bundler-abc123");
        let shadow = Path::new("/tmp/zfb-bundler-abc123/sub-packages/host");
        let project_root = Path::new("/workspace/./sub-packages/host");
        let first_party_root = Path::new("/workspace");
        // 2 levels up from `pages/foo.tsx` escapes `shadow` — enough to
        // trigger the note, since a PROJECT importer's boundary is
        // `shadow` alone (see the escape-scoping test below); this test is
        // only about which of the two prefix mappings names the importer.
        let stderr = "\u{2718} [ERROR] Could not resolve \"../../outside/shared.ts\"\n\n    pages/foo.tsx:1:19:\n      1 \u{2502} import x from \"../../outside/shared.ts\";\n\n1 error";

        let friendly =
            friendly_esbuild_error(stderr, shadow, project_root, first_party_root, work_root);

        assert!(
            friendly.contains("/workspace/./sub-packages/host/pages/foo.tsx"),
            "should map through the more specific `shadow` prefix (using \
             the caller's own `project_root` spelling) rather than the \
             wider `work_root` prefix (which would derive \
             /workspace/sub-packages/host/pages/foo.tsx instead): {friendly}"
        );
    }

    #[test]
    fn friendly_esbuild_error_scopes_project_importer_escape_to_shadow_even_inside_work_root() {
        // Codex review finding on issue #1839: a PROJECT importer (staged
        // under `shadow`) must be judged against the `shadow` boundary
        // ALONE, never `work_root` — even though the target here happens
        // to land inside the wider `work_root` mirror, the pre-existing
        // #1386 decision is that a relative import crossing `project_root`
        // stays unsupported regardless (the sibling mirror is reached only
        // via tsconfig aliases / bare imports, never a relative walk across
        // the project boundary), so this must still be flagged, not
        // silently swallowed just because a workspace sibling happens to
        // live at that lexical location too.
        let work_root = Path::new("/tmp/zfb-bundler-abc123");
        let shadow = Path::new("/tmp/zfb-bundler-abc123/sub-packages/host");
        let project_root = Path::new("/workspace/sub-packages/host");
        let first_party_root = Path::new("/workspace");
        // 3 levels up from `pages/foo.tsx` reaches `work_root`, then back
        // down into `lib/shared/other.ts` — a location the WORKSPACE
        // wholesale mirror does stage, but the PROJECT's own `shadow`
        // boundary never claims.
        let stderr = "\u{2718} [ERROR] Could not resolve \"../../../lib/shared/other.ts\"\n\n    pages/foo.tsx:1:19:\n      1 \u{2502} import x from \"../../../lib/shared/other.ts\";\n\n1 error";

        let friendly =
            friendly_esbuild_error(stderr, shadow, project_root, first_party_root, work_root);

        assert!(
            friendly.contains("/workspace/sub-packages/host/pages/foo.tsx"),
            "should still name the real project importer path: {friendly}"
        );
        assert!(
            friendly.to_lowercase().contains("shadow-copy"),
            "must NOT be silently suppressed just because the target \
             happens to land inside work_root — a project importer's \
             boundary is `shadow` alone: {friendly}"
        );
    }

    #[test]
    fn friendly_esbuild_error_reports_every_offender_in_one_run() {
        // Two independent escaping imports in the SAME stderr blob — proves
        // the note loop (paired with `run_esbuild`'s `--log-limit=0`) names
        // every offender in a single run instead of stopping at the first.
        let shadow = Path::new("/tmp/zfb-bundler-abc123");
        let project_root = Path::new("/Users/dev/my-project");
        let stderr = "\u{2718} [ERROR] Could not resolve \"../../outside/one.ts\"\n\n    pages/one.tsx:1:19:\n      1 \u{2502} import a from \"../../outside/one.ts\";\n\n\u{2718} [ERROR] Could not resolve \"../../outside/two.ts\"\n\n    pages/two.tsx:1:19:\n      1 \u{2502} import b from \"../../outside/two.ts\";\n\n2 errors";

        let friendly = friendly_esbuild_error(stderr, shadow, project_root, project_root, shadow);

        assert!(
            friendly.contains("../../outside/one.ts")
                && friendly.contains("/Users/dev/my-project/pages/one.tsx"),
            "first offender must be named: {friendly}"
        );
        assert!(
            friendly.contains("../../outside/two.ts")
                && friendly.contains("/Users/dev/my-project/pages/two.tsx"),
            "second offender must ALSO be named, not truncated: {friendly}"
        );
    }

    #[test]
    fn render_css_module_js_emits_sorted_default_export() {
        let mut map = HashMap::new();
        map.insert("btn".to_string(), "h1_btn".to_string());
        map.insert("btn-primary".to_string(), "h1_btn-primary".to_string());
        let js = render_css_module_js(Some(&map));
        // Default export, keys sorted, values JSON-quoted.
        assert!(js.starts_with("export default {"));
        let btn_at = js.find("\"btn\"").expect("btn key present");
        let primary_at = js.find("\"btn-primary\"").expect("btn-primary key present");
        assert!(btn_at < primary_at, "keys must be emitted sorted: {js}");
        assert!(js.contains("\"h1_btn\""));
        assert!(js.contains("\"h1_btn-primary\""));
    }

    #[test]
    fn render_css_module_js_empty_map_and_none_yield_empty_object() {
        assert_eq!(render_css_module_js(None), "export default {};\n");
        assert_eq!(
            render_css_module_js(Some(&HashMap::new())),
            "export default {};\n"
        );
    }

    #[test]
    fn rewrite_css_modules_in_shadow_rewrites_mapped_and_unmapped() {
        let proj = tempfile::tempdir().unwrap();
        let shadow = tempfile::tempdir().unwrap();
        let project_root = proj.path();
        let shadow_root = shadow.path();

        // Shadow mirrors project layout: components/card.module.css and
        // styles/orphan.module.css.
        fs::create_dir_all(shadow_root.join("components")).unwrap();
        fs::create_dir_all(shadow_root.join("styles")).unwrap();
        fs::write(
            shadow_root.join("components/card.module.css"),
            ".card { color: red; }",
        )
        .unwrap();
        fs::write(
            shadow_root.join("styles/orphan.module.css"),
            ".x { color: blue; }",
        )
        .unwrap();
        // A plain .css file must be left untouched.
        fs::write(shadow_root.join("styles/global.css"), ".g{}").unwrap();

        // Map keyed by the ORIGINAL project path of the mapped module.
        let mut names = HashMap::new();
        names.insert("card".to_string(), "sc0_card".to_string());
        let mut maps = HashMap::new();
        maps.insert(project_root.join("components/card.module.css"), names);

        rewrite_css_modules_in_shadow(
            shadow_root,
            project_root,
            &maps,
            leaked_passthrough_writer(),
        )
        .unwrap();

        let mapped = fs::read_to_string(shadow_root.join("components/card.module.css")).unwrap();
        assert!(mapped.contains("sc0_card"), "mapped module: {mapped}");
        assert!(!mapped.contains("color: red"), "raw CSS must be gone");

        let orphan = fs::read_to_string(shadow_root.join("styles/orphan.module.css")).unwrap();
        assert_eq!(
            orphan, "export default {};\n",
            "unmapped module degrades to {{}}"
        );

        let global = fs::read_to_string(shadow_root.join("styles/global.css")).unwrap();
        assert_eq!(global, ".g{}", "plain .css must be untouched");
    }

    #[test]
    fn rewrite_css_modules_in_work_mirror_rewrites_mapped_and_unmapped_sibling() {
        let work = tempfile::tempdir().unwrap();
        let work_root = work.path();
        let first_party_root = Path::new("/workspace");

        // `shadow` is nested under `work` (a workspace_rel of "apps/site"),
        // carrying its own .module.css — the project-mirror walk's job, not
        // this one's. This proves the pruned subtree is left byte-for-byte
        // untouched by the work-mirror walk.
        let shadow_root = work_root.join("apps/site");
        fs::create_dir_all(shadow_root.join("components")).unwrap();
        fs::write(
            shadow_root.join("components/card.module.css"),
            ".card { color: red; }",
        )
        .unwrap();

        // Claimed sibling region staged wholesale under `work`, outside
        // `shadow` — the #1692 wholesale mirror's shape.
        fs::create_dir_all(work_root.join("lib/shared")).unwrap();
        fs::write(
            work_root.join("lib/shared/widget.module.css"),
            ".widget { color: green; }",
        )
        .unwrap();
        fs::write(
            work_root.join("lib/shared/orphan.module.css"),
            ".x { color: blue; }",
        )
        .unwrap();
        // A plain .css file must be left untouched.
        fs::write(work_root.join("lib/shared/global.css"), ".g{}").unwrap();

        // Map keyed by the sibling's absolute physical path under
        // `first_party_root` — #1696's `compute_css_module_class_maps`
        // contract, which this walk must consume directly (no separate map).
        let mut names = HashMap::new();
        names.insert("widget".to_string(), "sc0_widget".to_string());
        let mut maps = HashMap::new();
        maps.insert(first_party_root.join("lib/shared/widget.module.css"), names);

        rewrite_css_modules_in_work_mirror(
            work_root,
            &shadow_root,
            first_party_root,
            &maps,
            leaked_passthrough_writer(),
        )
        .unwrap();

        let mapped = fs::read_to_string(work_root.join("lib/shared/widget.module.css")).unwrap();
        assert!(
            mapped.contains("sc0_widget"),
            "mapped sibling module: {mapped}"
        );
        assert!(!mapped.contains("color: green"), "raw CSS must be gone");

        let orphan = fs::read_to_string(work_root.join("lib/shared/orphan.module.css")).unwrap();
        assert_eq!(
            orphan, "export default {};\n",
            "unmapped sibling module degrades to {{}}, same as an unmapped project file"
        );

        let global = fs::read_to_string(work_root.join("lib/shared/global.css")).unwrap();
        assert_eq!(global, ".g{}", "plain .css must be untouched");

        // The shadow subtree is pruned entirely — this walk never visits it,
        // so its raw CSS is untouched (the project-mirror walk owns it).
        let shadow_untouched =
            fs::read_to_string(shadow_root.join("components/card.module.css")).unwrap();
        assert_eq!(
            shadow_untouched, ".card { color: red; }",
            "shadow subtree must be pruned from the work-mirror walk"
        );
    }

    fn make_minimal_input(tmp: &tempfile::TempDir) -> BundlerInput {
        let root = tmp.path().to_path_buf();
        // pages/index.tsx
        fs::create_dir_all(root.join("pages")).unwrap();
        fs::write(
            root.join("pages/index.tsx"),
            "export default function Home() { return null; }\n",
        )
        .unwrap();
        // empty support dirs
        fs::create_dir_all(root.join("content")).unwrap();
        fs::create_dir_all(root.join("components")).unwrap();
        fs::create_dir_all(root.join("layouts")).unwrap();
        BundlerInput {
            project_root: root.clone(),
            pages_dir: PathBuf::from("pages"),
            injected_pages_root: None,
            content_dir: PathBuf::from("content"),
            content_collections: Vec::new(),
            components_dir: PathBuf::from("components"),
            layouts_dir: PathBuf::from("layouts"),
            framework: Framework::Preact,
            define_vars: BTreeMap::new(),
            public_env_vars: HashMap::new(),
            tsconfig_paths: BTreeMap::new(),
            external: vec![],
            main_fields: Vec::new(),
            extra_loader_args: Vec::new(),
            outdir: root.join("dist"),
            mode: BundleMode::Production,
            minify: false,
            esbuild_binary: None,
            mock_subprocess_output: Some(
                "// mock bundle\nexport const routes = {};\nexport const hydrateIsland = () => {};\n"
                    .to_string(),
            ),
            content_snapshot_json: None,
            node_modules_dir: None,
            node_modules_preserve_symlinks: false,
            pipeline_spec: zfb_content::PipelineSpec::default(),
            resolve_markdown_links: None,
            site: None,
            prefetch_disabled: false,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            worker_only_routes: None,
            bundle_basename: None,
            css_module_class_maps: HashMap::new(),
            mdx_components_file: None,
            bundle_exclude: Vec::new(),
            base_prefix: None,
        }
    }

    #[test]
    fn route_derivation_collapses_index_and_keeps_dynamic_segments() {
        assert_eq!(derive_route(Path::new("index.tsx")).as_deref(), Some("/"));
        assert_eq!(
            derive_route(Path::new("about.tsx")).as_deref(),
            Some("/about")
        );
        assert_eq!(
            derive_route(Path::new("blog/index.tsx")).as_deref(),
            Some("/blog")
        );
        assert_eq!(
            derive_route(Path::new("blog/[slug].tsx")).as_deref(),
            Some("/blog/[slug]")
        );
        assert_eq!(
            derive_route(Path::new("post.mdx")).as_deref(),
            Some("/post")
        );
        // _private files are skipped.
        assert!(derive_route(Path::new("_dev.tsx")).is_none());
        // Unknown extensions are skipped.
        assert!(derive_route(Path::new("README.txt")).is_none());
        // .md and .html are now accepted page extensions.
        assert_eq!(
            derive_route(Path::new("about.md")).as_deref(),
            Some("/about")
        );
        assert_eq!(derive_route(Path::new("index.html")).as_deref(), Some("/"));
    }

    /// The bundler is the SECOND page-extension consumer (issue #1742's
    /// original drift). Skipping the conventional non-page sidecars in
    /// `zfb-router`'s `scan_pages` but not here would bundle them as
    /// production routes the router never emitted — the same drift from
    /// the other side.
    #[test]
    fn route_derivation_skips_conventional_non_page_sidecars() {
        for sidecar in [
            "index.test.ts",
            "about.spec.tsx",
            "widget.test.jsx",
            "helpers.spec.js",
            "env.d.ts",
            "blog/[slug].test.ts",
        ] {
            assert!(
                derive_route(Path::new(sidecar)).is_none(),
                "{sidecar} must not derive a route"
            );
        }
        // Not over-excluded: genuine pages with similar names still route.
        assert_eq!(
            derive_route(Path::new("plain.ts")).as_deref(),
            Some("/plain")
        );
        assert_eq!(derive_route(Path::new("test.md")).as_deref(), Some("/test"));
        assert_eq!(derive_route(Path::new("d.ts")).as_deref(), Some("/d"));
    }

    #[test]
    fn entry_module_emits_default_fetch_wrapper_with_routes() {
        // T7-sibling contract: the bundler's synthetic entry.mjs MUST
        // expose a Workers-style `export default { fetch }` so workerd
        // (embedded V8 host) can dispatch requests. Without it, host boot
        // fails with "missing default export" before any user code runs.
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path();
        let routes = vec![
            RouteEntry {
                route: "/".to_string(),
                source_path: PathBuf::from("pages/index.tsx"),
                entry_key: "/".to_string(),
                static_html: false,
                rel_under_pages: PathBuf::from("index.tsx"),
            },
            RouteEntry {
                route: "/about".to_string(),
                source_path: PathBuf::from("pages/about.tsx"),
                entry_key: "/about".to_string(),
                static_html: false,
                rel_under_pages: PathBuf::from("about.tsx"),
            },
        ];
        write_entry_module(
            shadow,
            &routes,
            &EntryModuleInputs {
                render_to_string_module: "preact-render-to-string",
                content_snapshot_json: None,
                content_imports: &[],
                site: None,
                prefetch_disabled: false,
                mdx_components_import_spec: None,
                base_prefix: None,
            },
        )
        .unwrap();

        let body = fs::read_to_string(shadow.join(SHADOW_ENTRY_FILENAME)).unwrap();

        // Imports the runtime factory and the framework's renderToString.
        assert!(
            body.contains("from \"@takazudo/zfb-runtime/server\""),
            "entry.mjs must import createPageRouter from the server-only subpath \
             @takazudo/zfb-runtime/server (issue #1298); got:\n{body}"
        );
        assert!(
            body.contains("\"preact-render-to-string\""),
            "entry.mjs must import renderToString from the framework module; got:\n{body}"
        );

        // Constructs the router with the routes-derived pages array.
        assert!(
            body.contains("createPageRouter({"),
            "entry.mjs must call createPageRouter; got:\n{body}"
        );
        assert!(
            body.contains("pages: __zfb_pages"),
            "createPageRouter call must hand it the pages array; got:\n{body}"
        );
        assert!(
            body.contains("renderToString: __zfb_renderToString"),
            "createPageRouter call must hand it the framework adapter; got:\n{body}"
        );
        assert!(
            body.contains("route: \"/\", module: () => Promise.resolve(__zfb_route_0)"),
            "pages array must contain a thunk per route; got:\n{body}"
        );
        assert!(
            body.contains("route: \"/about\", module: () => Promise.resolve(__zfb_route_1)"),
            "pages array must contain every route; got:\n{body}"
        );

        // Workers-style default export.
        assert!(
            body.contains("export default {"),
            "entry.mjs must emit a default object export; got:\n{body}"
        );
        assert!(
            body.contains("fetch: (request) => __zfb_router(request)"),
            "default export must carry a fetch field delegating to the router; got:\n{body}"
        );

        // Existing exports must still be present so the runtime
        // adapter (T2) and the islands hydration path keep working.
        assert!(body.contains("export const routes = {"));
        assert!(body.contains("export { hydrateIsland };"));
    }

    #[test]
    fn entry_module_emits_content_bridge_for_provided_imports() {
        // #506 acceptance: when the bundler hands `write_entry_module`
        // a non-empty `content_imports`, the synthetic `entry.mjs`
        // must (a) emit one `import * as __zfb_content_<i>` line per
        // entry, (b) populate a `__zfb_content_modules` Map keyed on
        // BOTH the hash-bearing and hash-stripped specifier forms,
        // and (c) install `globalThis.__zfb.content.get(...)` BEFORE
        // `createPageRouter` so the very first SSR call sees the map.
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path();
        let imports = vec![
            ContentImport {
                specifier: "mdx://docs/intro#abc12345".to_string(),
                shadow_rel_path: "content/docs/intro.mdx".to_string(),
            },
            ContentImport {
                specifier: "mdx://docs-ja/intro#def67890".to_string(),
                shadow_rel_path: "content/docs-ja/intro.mdx".to_string(),
            },
        ];
        write_entry_module(
            shadow,
            &[],
            &EntryModuleInputs {
                render_to_string_module: "preact-render-to-string",
                content_snapshot_json: None,
                content_imports: &imports,
                site: None,
                prefetch_disabled: false,
                mdx_components_import_spec: None,
                base_prefix: None,
            },
        )
        .unwrap();

        let body = fs::read_to_string(shadow.join(SHADOW_ENTRY_FILENAME)).unwrap();

        // Per-import namespace import lines.
        assert!(
            body.contains("import * as __zfb_content_0 from \"./content/docs/intro.mdx\";"),
            "missing __zfb_content_0 import; got:\n{body}"
        );
        assert!(
            body.contains("import * as __zfb_content_1 from \"./content/docs-ja/intro.mdx\";"),
            "missing __zfb_content_1 import; got:\n{body}"
        );

        // Bridge map carries both hash-bearing and no-hash keys.
        assert!(
            body.contains("[\"mdx://docs/intro#abc12345\", __zfb_content_0.default]"),
            "hash-bearing key for entry 0 missing; got:\n{body}"
        );
        assert!(
            body.contains("[\"mdx://docs/intro\", __zfb_content_0.default]"),
            "no-hash key for entry 0 missing; got:\n{body}"
        );
        assert!(
            body.contains("[\"mdx://docs-ja/intro#def67890\", __zfb_content_1.default]"),
            "hash-bearing key for entry 1 missing; got:\n{body}"
        );
        assert!(
            body.contains("[\"mdx://docs-ja/intro\", __zfb_content_1.default]"),
            "no-hash key for entry 1 missing; got:\n{body}"
        );

        // Bridge installer assigns onto `globalThis.__zfb.content`.
        assert!(
            body.contains("globalThis.__zfb = globalThis.__zfb ?? {};"),
            "missing globalThis.__zfb namespacing; got:\n{body}"
        );
        assert!(
            body.contains(
                "globalThis.__zfb.content = { get: (spec) => __zfb_content_modules.get(spec) };"
            ),
            "missing globalThis.__zfb.content installer; got:\n{body}"
        );

        // The installer must run BEFORE createPageRouter is constructed.
        let bridge_idx = body
            .find("globalThis.__zfb.content = ")
            .expect("bridge install line present");
        let router_idx = body
            .find("createPageRouter({")
            .expect("createPageRouter call present");
        assert!(
            bridge_idx < router_idx,
            "bridge installer must precede createPageRouter; bridge at {bridge_idx}, router at {router_idx}"
        );
    }

    #[test]
    fn entry_module_omits_content_bridge_when_no_imports() {
        // No collections → no bridge installer (back-compat: legacy
        // builds with an empty content_dir keep producing a clean
        // `entry.mjs` without the bridge symbols).
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path();
        write_entry_module(
            shadow,
            &[],
            &EntryModuleInputs {
                render_to_string_module: "preact-render-to-string",
                content_snapshot_json: None,
                content_imports: &[],
                site: None,
                prefetch_disabled: false,
                mdx_components_import_spec: None,
                base_prefix: None,
            },
        )
        .unwrap();

        let body = fs::read_to_string(shadow.join(SHADOW_ENTRY_FILENAME)).unwrap();
        assert!(
            !body.contains("__zfb_content_modules"),
            "no imports → no bridge map; got:\n{body}"
        );
        assert!(
            !body.contains("globalThis.__zfb.content"),
            "no imports → no bridge installer; got:\n{body}"
        );
    }

    // --- site setter tests (#254) ------------------------------------------

    #[test]
    fn entry_module_emits_site_setter_when_some() {
        // When `site` is `Some`, the entry module must emit
        // `globalThis.__zfb.site = <json-encoded-url>` BEFORE the
        // content bridge and `createPageRouter` so any SSR call to
        // `globalThis.__zfb.site` sees the value from the first request.
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path();
        write_entry_module(
            shadow,
            &[],
            &EntryModuleInputs {
                render_to_string_module: "preact-render-to-string",
                content_snapshot_json: None,
                content_imports: &[],
                site: Some("https://example.com"),
                prefetch_disabled: false,
                mdx_components_import_spec: None,
                base_prefix: None,
            },
        )
        .unwrap();

        let body = fs::read_to_string(shadow.join(SHADOW_ENTRY_FILENAME)).unwrap();

        assert!(
            body.contains("globalThis.__zfb = globalThis.__zfb ?? {};"),
            "site branch must emit the namespacing guard; got:\n{body}"
        );
        assert!(
            body.contains("globalThis.__zfb.site = \"https://example.com\";"),
            "site setter must contain the JSON-encoded URL; got:\n{body}"
        );

        // The site setter must precede createPageRouter so SSR code
        // that reads `globalThis.__zfb.site` during the first request
        // already sees the value.
        let site_idx = body
            .find("globalThis.__zfb.site = ")
            .expect("site setter present");
        let router_idx = body
            .find("createPageRouter({")
            .expect("createPageRouter present");
        assert!(
            site_idx < router_idx,
            "site setter must precede createPageRouter; site at {site_idx}, router at {router_idx}"
        );
    }

    #[test]
    fn entry_module_omits_site_setter_when_none() {
        // When `site` is `None`, zero bytes related to the site setter
        // are emitted — preserving byte-for-byte parity with the
        // pre-`site` build (sub #254 acceptance criterion).
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path();
        write_entry_module(
            shadow,
            &[],
            &EntryModuleInputs {
                render_to_string_module: "preact-render-to-string",
                content_snapshot_json: None,
                content_imports: &[],
                site: None,
                prefetch_disabled: false,
                mdx_components_import_spec: None,
                base_prefix: None,
            },
        )
        .unwrap();

        let body = fs::read_to_string(shadow.join(SHADOW_ENTRY_FILENAME)).unwrap();
        assert!(
            !body.contains("globalThis.__zfb.site"),
            "site=None → no site setter; got:\n{body}"
        );
    }

    // --- prefetch_disabled flag tests (#277) ---------------------------------

    #[test]
    fn entry_module_emits_prefetch_disabled_when_true() {
        // When `prefetch_disabled` is `true`, the entry module must emit
        // `globalThis.__zfb.prefetchDisabled = true` before `createPageRouter`
        // so `<ClientRouter />` can read it during the first SSR call.
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path();
        write_entry_module(
            shadow,
            &[],
            &EntryModuleInputs {
                render_to_string_module: "preact-render-to-string",
                content_snapshot_json: None,
                content_imports: &[],
                site: None,
                prefetch_disabled: true,
                mdx_components_import_spec: None,
                base_prefix: None,
            },
        )
        .unwrap();

        let body = fs::read_to_string(shadow.join(SHADOW_ENTRY_FILENAME)).unwrap();

        assert!(
            body.contains("globalThis.__zfb = globalThis.__zfb ?? {};"),
            "prefetch_disabled branch must emit the namespacing guard; got:\n{body}"
        );
        assert!(
            body.contains("globalThis.__zfb.prefetchDisabled = true;"),
            "prefetch_disabled setter must be present; got:\n{body}"
        );

        // The flag must precede createPageRouter so SSR code that reads
        // `globalThis.__zfb.prefetchDisabled` on the first request already
        // sees the value.
        let flag_idx = body
            .find("globalThis.__zfb.prefetchDisabled")
            .expect("prefetchDisabled setter present");
        let router_idx = body
            .find("createPageRouter({")
            .expect("createPageRouter present");
        assert!(
            flag_idx < router_idx,
            "prefetch_disabled setter must precede createPageRouter; flag at {flag_idx}, router at {router_idx}"
        );
    }

    #[test]
    fn entry_module_omits_prefetch_disabled_when_false() {
        // When `prefetch_disabled` is `false`, zero bytes related to the
        // prefetch flag are emitted — preserving byte-for-byte parity with
        // the pre-`prefetch` build (sub #277 acceptance criterion).
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path();
        write_entry_module(
            shadow,
            &[],
            &EntryModuleInputs {
                render_to_string_module: "preact-render-to-string",
                content_snapshot_json: None,
                content_imports: &[],
                site: None,
                prefetch_disabled: false,
                mdx_components_import_spec: None,
                base_prefix: None,
            },
        )
        .unwrap();

        let body = fs::read_to_string(shadow.join(SHADOW_ENTRY_FILENAME)).unwrap();
        assert!(
            !body.contains("globalThis.__zfb.prefetchDisabled"),
            "prefetch_disabled=false → no prefetch setter; got:\n{body}"
        );
    }

    // --- base_prefix / globalThis.__zfb.base (#978) ---------------------------

    #[test]
    fn entry_module_emits_base_prefix_when_some() {
        // When `base_prefix` is `Some`, the entry module must emit
        // `globalThis.__zfb.base = <json-value>` before `createPageRouter` so
        // `clientScript(name)` can build the correct base-prefixed stable URL.
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path();
        write_entry_module(
            shadow,
            &[],
            &EntryModuleInputs {
                render_to_string_module: "preact-render-to-string",
                content_snapshot_json: None,
                content_imports: &[],
                site: None,
                prefetch_disabled: false,
                mdx_components_import_spec: None,
                base_prefix: Some("/foo"),
            },
        )
        .unwrap();

        let body = fs::read_to_string(shadow.join(SHADOW_ENTRY_FILENAME)).unwrap();

        assert!(
            body.contains("globalThis.__zfb = globalThis.__zfb ?? {};"),
            "base_prefix branch must emit the namespacing guard; got:\n{body}"
        );
        assert!(
            body.contains("globalThis.__zfb.base = \"/foo\";"),
            "base setter must contain the JSON-encoded prefix; got:\n{body}"
        );

        // The setter must precede createPageRouter so clientScript() calls
        // inside any SSR route already see the value from the first request.
        let base_idx = body
            .find("globalThis.__zfb.base = ")
            .expect("base setter present");
        let router_idx = body
            .find("createPageRouter({")
            .expect("createPageRouter present");
        assert!(
            base_idx < router_idx,
            "base setter must precede createPageRouter; base at {base_idx}, router at {router_idx}"
        );
    }

    #[test]
    fn entry_module_emits_empty_base_prefix_when_some_empty_string() {
        // Root-mounted / no-base build: `base_prefix = Some("")`.
        // The setter must still be emitted (so clientScript() knows to use
        // the empty prefix), but the JSON value is `""`.
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path();
        write_entry_module(
            shadow,
            &[],
            &EntryModuleInputs {
                render_to_string_module: "preact-render-to-string",
                content_snapshot_json: None,
                content_imports: &[],
                site: None,
                prefetch_disabled: false,
                mdx_components_import_spec: None,
                base_prefix: Some(""),
            },
        )
        .unwrap();

        let body = fs::read_to_string(shadow.join(SHADOW_ENTRY_FILENAME)).unwrap();

        assert!(
            body.contains("globalThis.__zfb.base = \"\";"),
            "empty-string base prefix must emit base = \"\"; got:\n{body}"
        );
    }

    #[test]
    fn entry_module_omits_base_prefix_when_none() {
        // Builds without client scripts (`base_prefix = None`) must not emit
        // the base setter — keeping byte-for-byte parity with pre-#978 builds
        // (#261 zero-registration parity, #940 byte-identical dev bundle skip).
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path();
        write_entry_module(
            shadow,
            &[],
            &EntryModuleInputs {
                render_to_string_module: "preact-render-to-string",
                content_snapshot_json: None,
                content_imports: &[],
                site: None,
                prefetch_disabled: false,
                mdx_components_import_spec: None,
                base_prefix: None,
            },
        )
        .unwrap();

        let body = fs::read_to_string(shadow.join(SHADOW_ENTRY_FILENAME)).unwrap();
        assert!(
            !body.contains("globalThis.__zfb.base"),
            "base_prefix=None → no base setter; got:\n{body}"
        );
    }

    // --- mdx-components.tsx global override map (#616) ------------------------

    #[test]
    fn materialise_mdx_components_file_copies_into_shadow_root_and_returns_spec() {
        // The "easily-missed" step: a root-level FILE is copied (not
        // symlinked) into the shadow root so esbuild sees an in-shadow
        // importer whose relative imports + tsconfig `paths` resolve. The
        // returned spec is the shadow-relative import specifier.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("mdx-components.tsx");
        let contents = "export default { h2: function MyH2() {} };\n";
        fs::write(&src, contents).unwrap();

        let shadow = tempfile::tempdir().unwrap();
        let shadow_root = shadow.path();

        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(tmp.path(), &exclude);
        let spec = materialise_mdx_components_file(&src, shadow_root, &ctx).unwrap();
        assert_eq!(spec.as_deref(), Some("./mdx-components.tsx"));

        // A real copy lands in the shadow root (so esbuild resolves its
        // relative imports against the shadow tree, not the project root).
        let dst = shadow_root.join("mdx-components.tsx");
        assert!(dst.is_file(), "copied file must exist at shadow root");
        assert_eq!(fs::read_to_string(&dst).unwrap(), contents);
    }

    #[test]
    fn materialise_mdx_components_file_omits_excluded_source_and_import_spec() {
        let project = tempfile::tempdir().unwrap();
        let src = project.path().join("mdx-components.tsx");
        fs::write(
            &src,
            "export default { h2: function ExcludedHeading() {} };\n",
        )
        .unwrap();
        let shadow = tempfile::tempdir().unwrap();
        let exclude = BundleExcludeMatcher::new(&["mdx-components.tsx".to_string()]).unwrap();
        let ctx = default_mat_ctx(project.path(), &exclude);

        let spec = materialise_mdx_components_file(&src, shadow.path(), &ctx).unwrap();

        assert_eq!(spec, None, "excluded override must not be imported");
        assert!(
            !shadow.path().join("mdx-components.tsx").exists(),
            "excluded override must not be materialised"
        );
    }

    #[test]
    fn materialise_mdx_components_file_expands_raw_imports() {
        let project = tempfile::tempdir().unwrap();
        let src = project.path().join("mdx-components.tsx");
        fs::write(
            &src,
            "import theme from './theme.css?raw';\nexport default { theme };\n",
        )
        .unwrap();
        fs::write(project.path().join("theme.css"), "--raw-theme-marker").unwrap();
        let shadow = tempfile::tempdir().unwrap();
        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(project.path(), &exclude);

        materialise_mdx_components_file(&src, shadow.path(), &ctx)
            .unwrap()
            .expect("override import spec");
        let staged = fs::read_to_string(shadow.path().join("mdx-components.tsx")).unwrap();
        assert!(!staged.contains("?raw"), "{staged}");
        let generated = fs::read_dir(shadow.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".zfb-raw-") && name.ends_with(".mjs"))
            })
            .expect("generated root raw wrapper");
        assert!(fs::read_to_string(generated)
            .unwrap()
            .contains("--raw-theme-marker"));
    }

    #[test]
    fn entry_module_emits_mdx_components_installer_when_present() {
        // When the global override map is discovered, the entry module must
        // (a) default-import the materialised file, (b) install it onto
        // `globalThis.__zfb.mdxComponents`, and (c) place the setter before
        // `createPageRouter` (alongside the other __zfb setters).
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path();
        write_entry_module(
            shadow,
            &[],
            &EntryModuleInputs {
                render_to_string_module: "preact-render-to-string",
                content_snapshot_json: None,
                content_imports: &[],
                site: None,
                prefetch_disabled: false,
                mdx_components_import_spec: Some("./mdx-components.tsx"),
                base_prefix: None,
            },
        )
        .unwrap();

        let body = fs::read_to_string(shadow.join(SHADOW_ENTRY_FILENAME)).unwrap();

        // (a) Default import of the materialised override map.
        assert!(
            body.contains("import __zfb_mdx_components from \"./mdx-components.tsx\";"),
            "must default-import the override map; got:\n{body}"
        );
        // (b) Idempotent namespacing guard + setter.
        assert!(
            body.contains("globalThis.__zfb = globalThis.__zfb ?? {};"),
            "mdxComponents branch must emit the namespacing guard; got:\n{body}"
        );
        assert!(
            body.contains("globalThis.__zfb.mdxComponents = __zfb_mdx_components;"),
            "mdxComponents setter must be present; got:\n{body}"
        );

        // (c) The setter must precede createPageRouter so the very first
        // SSR call already sees the populated slot.
        let setter_idx = body
            .find("globalThis.__zfb.mdxComponents = ")
            .expect("mdxComponents setter present");
        let router_idx = body
            .find("createPageRouter({")
            .expect("createPageRouter present");
        assert!(
            setter_idx < router_idx,
            "mdxComponents setter must precede createPageRouter; setter at {setter_idx}, router at {router_idx}"
        );
    }

    #[test]
    fn entry_module_emits_mdx_components_installer_with_zero_content_imports() {
        // Acceptance criterion: the override map installs even with ZERO
        // content-collection entries. The install is gated only on the file
        // being present, NOT on `content_imports` being non-empty — so an
        // empty `content_imports` slice must still produce the installer and
        // must NOT produce the content-bridge installer.
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path();
        write_entry_module(
            shadow,
            &[],
            &EntryModuleInputs {
                render_to_string_module: "preact-render-to-string",
                content_snapshot_json: None,
                content_imports: &[], // zero content imports
                site: None,
                prefetch_disabled: false,
                mdx_components_import_spec: Some("./mdx-components.tsx"),
                base_prefix: None,
            },
        )
        .unwrap();

        let body = fs::read_to_string(shadow.join(SHADOW_ENTRY_FILENAME)).unwrap();
        assert!(
            body.contains("globalThis.__zfb.mdxComponents = __zfb_mdx_components;"),
            "mdxComponents installer must be emitted with zero content imports; got:\n{body}"
        );
        // The content bridge installer is gated on content_imports and must
        // stay absent — proving the two installers are independent.
        assert!(
            !body.contains("globalThis.__zfb.content = "),
            "content bridge installer must NOT appear with zero content imports; got:\n{body}"
        );
    }

    #[test]
    fn entry_module_omits_mdx_components_installer_when_none() {
        // When no override file is discovered, zero bytes related to the
        // installer are emitted — preserving byte-for-byte parity with a
        // project that does not use the convention (#616 acceptance).
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path();
        write_entry_module(
            shadow,
            &[],
            &EntryModuleInputs {
                render_to_string_module: "preact-render-to-string",
                content_snapshot_json: None,
                content_imports: &[],
                site: None,
                prefetch_disabled: false,
                mdx_components_import_spec: None,
                base_prefix: None,
            },
        )
        .unwrap();

        let body = fs::read_to_string(shadow.join(SHADOW_ENTRY_FILENAME)).unwrap();
        assert!(
            !body.contains("__zfb_mdx_components"),
            "no file → no override import; got:\n{body}"
        );
        assert!(
            !body.contains("globalThis.__zfb.mdxComponents"),
            "no file → no override setter; got:\n{body}"
        );
    }

    // C1 — file-map layer: proves the entry module installs the mdx-components
    // default export using the canonical #616 shape.
    //
    // The canonical export shape pinned by #616 is a **default export object**:
    //   export default { h2: MyH2, … }
    //
    // The bundler must (a) emit a default import of the materialised file and
    // (b) install it as `globalThis.__zfb.mdxComponents = __zfb_mdx_components`.
    // The default import form is the only contract — named exports or namespace
    // imports are not supported. This test pins that contract so a future
    // refactor that accidentally switches to `import { h2 } from …` or
    // `import * as …` will be caught here.
    #[test]
    fn entry_module_uses_default_import_for_mdx_components_file_map_layer() {
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path();
        write_entry_module(
            shadow,
            &[],
            &EntryModuleInputs {
                render_to_string_module: "preact-render-to-string",
                content_snapshot_json: None,
                content_imports: &[],
                site: None,
                prefetch_disabled: false,
                mdx_components_import_spec: Some("./mdx-components.tsx"),
                base_prefix: None,
            },
        )
        .unwrap();

        let body = fs::read_to_string(shadow.join(SHADOW_ENTRY_FILENAME)).unwrap();

        // (a) Must be a DEFAULT import, not a namespace or named import.
        // The canonical #616 shape is `export default { h2: MyH2, … }`, so
        // entry.mjs must read it as the default binding.
        assert!(
            body.contains("import __zfb_mdx_components from \"./mdx-components.tsx\";"),
            "file-map layer must use a default import (canonical #616 shape); got:\n{body}"
        );
        // Absence of namespace and named import forms specifically for the
        // mdx-components file — these would fail to read a plain default-export
        // object. (Other named imports in the entry module, like renderToString,
        // are unrelated and must not be checked here.)
        assert!(
            !body.contains("import * as __zfb_mdx_components"),
            "namespace import must not be used for the file-map layer; got:\n{body}"
        );
        assert!(
            !body.contains("import { __zfb_mdx_components"),
            "named import of __zfb_mdx_components must not be used for the file-map layer; got:\n{body}"
        );

        // (b) The installed binding is assigned to the mdxComponents slot.
        // This is the precedence seam read by mergeMdxComponents in content.ts
        // (defaultComponents → this slot → per-call components).
        assert!(
            body.contains("globalThis.__zfb.mdxComponents = __zfb_mdx_components;"),
            "file-map layer must install to the mdxComponents precedence slot; got:\n{body}"
        );
    }

    #[test]
    fn materialise_collection_records_imports_and_compiles_mdx() {
        // The per-collection materialiser must:
        //   1. Copy non-MDX files verbatim into the per-collection
        //      shadow subtree.
        //   2. Compile MDX files via compile_mdx_to_jsx_module_cached
        //      and write the resulting JSX text to disk under the
        //      `.mdx` extension.
        //   3. Record an `(specifier, shadow_rel_path)` pair for each
        //      MDX entry, where `shadow_rel_path` is prefixed with
        //      `content/<collection>/` so esbuild's later import
        //      resolution lands on the shadow tree.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("docs");
        fs::create_dir_all(src.join("getting-started")).unwrap();
        fs::write(
            src.join("intro.mdx"),
            "---\ntitle: Intro\n---\n\n# Hello\n\nplain paragraph\n",
        )
        .unwrap();
        fs::write(
            src.join("getting-started/installation.mdx"),
            "## Install\n\nstep one\n",
        )
        .unwrap();
        fs::write(src.join("README.txt"), "not mdx\n").unwrap();

        let dest = tmp.path().join("shadow_content").join("docs");
        let mut imports: Vec<ContentImport> = Vec::new();
        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(tmp.path(), &exclude);
        materialise_collection(
            &src,
            &dest,
            "docs",
            &mut imports,
            &ctx,
            None,
            None,
            None,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();

        // Two MDX files → two ContentImport records, with stable
        // forward-slash shadow_rel_paths under `content/docs/...`.
        assert_eq!(imports.len(), 2, "expected 2 MDX imports, got {imports:?}");
        let rels: Vec<&str> = imports.iter().map(|i| i.shadow_rel_path.as_str()).collect();
        assert!(
            rels.contains(&"content/docs/intro.mdx"),
            "missing top-level intro entry; got: {rels:?}"
        );
        assert!(
            rels.contains(&"content/docs/getting-started/installation.mdx"),
            "missing nested installation entry; got: {rels:?}"
        );

        // Specifier shape comes from compile_mdx_to_jsx_module_cached
        // (`mdx://<parent_dir>/<file_stem>#<hash>`). We don't pin the
        // hash (compile output may vary as the emitter evolves); we
        // just check the prefix.
        for ci in &imports {
            assert!(
                ci.specifier.starts_with("mdx://"),
                "specifier should be mdx://*, got {}",
                ci.specifier
            );
            assert!(
                ci.specifier.contains('#'),
                "specifier should include hash, got {}",
                ci.specifier
            );
        }

        // Files were materialised on disk.
        assert!(dest.join("intro.mdx").is_file());
        assert!(dest.join("getting-started/installation.mdx").is_file());
        assert!(dest.join("README.txt").is_file());

        // MDX bodies were rewritten to JSX (the compiled output ships
        // a `function _createMdxContent` wrapper).
        let intro_jsx = fs::read_to_string(dest.join("intro.mdx")).unwrap();
        assert!(
            intro_jsx.contains("_createMdxContent"),
            "compiled MDX must contain _createMdxContent; got:\n{intro_jsx}"
        );

        // Non-MDX files copied verbatim.
        let txt = fs::read_to_string(dest.join("README.txt")).unwrap();
        assert_eq!(txt, "not mdx\n");
    }

    #[test]
    fn materialise_collection_rejects_bundle_excluded_markdown_for_snapshot_parity() {
        for extension in ["md", "mdx"] {
            let project = tempfile::tempdir().unwrap();
            let src = project.path().join("posts");
            fs::create_dir_all(&src).unwrap();
            let filename = format!("excluded.{extension}");
            fs::write(src.join(&filename), "# Must not enter the SSR bridge\n").unwrap();
            let dest = project.path().join("shadow_content/posts");
            let mut imports = Vec::new();
            let matcher = BundleExcludeMatcher::new(&[format!("posts/{filename}")]).unwrap();
            let ctx = default_mat_ctx(project.path(), &matcher);

            let error = materialise_collection(
                &src,
                &dest,
                "posts",
                &mut imports,
                &ctx,
                None,
                None,
                None,
                &mut Vec::new(),
                &mut Vec::new(),
                &mut Vec::new(),
                &mut Vec::new(),
            )
            .expect_err("bundle.exclude on collection Markdown must fail loudly");
            let message = format!("{error:#}");
            assert!(message.contains("snapshot"), "{message}");
            assert!(message.contains(&filename), "{message}");
            assert!(imports.is_empty(), "excluded source must not be imported");
            assert!(
                !dest.join(&filename).exists(),
                "excluded source must not be materialised"
            );
        }
    }

    #[test]
    fn materialise_collection_compiles_md_files_into_bridge() {
        // Regression test for zfb#405 / zfb#398: `.md` files were previously
        // `fs::copy`'d verbatim and never added to `imports`, so the bridge
        // map had no entry and `bridge.get(spec)` returned `undefined`,
        // causing the page to fall back to `<pre data-zfb-content-fallback>`.
        //
        // After the fix, `.md` files are compiled through
        // `compile_mdx_to_jsx_module_cached` (CommonMark is a strict MDX
        // subset) and produce a `ContentImport` with an `mdx://...` specifier,
        // exactly like `.mdx` files. The shadow file retains the `.md`
        // extension; `--loader:.md=jsx` in `ESBUILD_LOADER_ARGS` tells esbuild
        // to parse the compiled JSX.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("posts");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            src.join("hello.md"),
            "---\ntitle: Hello\n---\n\n**node-free**\n",
        )
        .unwrap();

        let dest = tmp.path().join("shadow_content").join("posts");
        let mut imports: Vec<ContentImport> = Vec::new();
        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(tmp.path(), &exclude);
        materialise_collection(
            &src,
            &dest,
            "posts",
            &mut imports,
            &ctx,
            None,
            None,
            None,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();

        // The `.md` file must produce a ContentImport (was missing before the fix).
        assert_eq!(
            imports.len(),
            1,
            "expected 1 import for the .md file, got {imports:?}"
        );

        let ci = &imports[0];

        // Shadow path retains `.md` extension (not renamed to `.mdx`).
        assert_eq!(
            ci.shadow_rel_path, "content/posts/hello.md",
            "shadow_rel_path must retain .md extension; got {}",
            ci.shadow_rel_path
        );

        // Specifier must be an `mdx://` URI with a hash segment.
        assert!(
            ci.specifier.starts_with("mdx://"),
            "specifier must start with mdx://; got {}",
            ci.specifier
        );
        assert!(
            ci.specifier.contains('#'),
            "specifier must contain hash segment; got {}",
            ci.specifier
        );

        // Shadow file exists at the `.md` path and contains compiled JSX.
        let shadow_file = dest.join("hello.md");
        assert!(shadow_file.is_file(), "shadow file must exist at hello.md");
        let jsx = fs::read_to_string(&shadow_file).unwrap();
        assert!(
            jsx.contains("_createMdxContent"),
            "shadow .md file must contain compiled JSX (_createMdxContent); got:\n{jsx}"
        );

        // The specifier is present in an entry.mjs bridge map — confirming
        // that `write_entry_module` correctly wires the `.md` import.
        let shadow_root = tmp.path().join("shadow_for_entry");
        fs::create_dir_all(&shadow_root).unwrap();
        write_entry_module(
            &shadow_root,
            &[],
            &EntryModuleInputs {
                render_to_string_module: "preact-render-to-string",
                content_snapshot_json: None,
                content_imports: &imports,
                site: None,
                prefetch_disabled: false,
                mdx_components_import_spec: None,
                base_prefix: None,
            },
        )
        .unwrap();
        let entry = fs::read_to_string(shadow_root.join(SHADOW_ENTRY_FILENAME)).unwrap();

        // The bridge import line must reference the `.md` shadow path.
        assert!(
            entry.contains("from \"./content/posts/hello.md\";"),
            "entry.mjs bridge import must reference hello.md; got:\n{entry}"
        );

        // The bridge map must contain the specifier key.
        let spec_no_hash = ci.specifier.split('#').next().unwrap();
        assert!(
            entry.contains(&format!("[\"{}\",", ci.specifier))
                || entry.contains(&format!("[\"{}\", ", ci.specifier)),
            "entry.mjs bridge map must contain hash-bearing specifier {}; got:\n{entry}",
            ci.specifier
        );
        assert!(
            entry.contains(&format!("[\"{}\",", spec_no_hash))
                || entry.contains(&format!("[\"{}\", ", spec_no_hash)),
            "entry.mjs bridge map must contain no-hash specifier {}; got:\n{entry}",
            spec_no_hash
        );

        // Loader constant must include `.md=jsx` so esbuild can parse the
        // compiled shadow file (ESBUILD_LOADER_ARGS regression check).
        assert!(
            ESBUILD_LOADER_ARGS.contains(&"--loader:.md=jsx"),
            "ESBUILD_LOADER_ARGS must include --loader:.md=jsx; got: {ESBUILD_LOADER_ARGS:?}"
        );
    }

    // -----------------------------------------------------------------
    // ShadowSession content-file skip cache (incremental materialise,
    // zfb#1148)
    // -----------------------------------------------------------------

    /// Result of one simulated `materialise_collection` tick in session
    /// mode (see [`run_collection_tick`]).
    struct CollectionTickOut {
        imports: Vec<ContentImport>,
        headings: Vec<FileHeadings>,
        cross_links: Vec<CrossFileLinkCandidate>,
    }

    /// Run ONE tick of `materialise_collection` against `session` in
    /// session mode, faithfully mirroring `bundle_with_session`'s
    /// per-call lifecycle: build the `ShadowWriter` (which performs the
    /// dirty/copy_mode-flip wipe + arms dirty), materialise, prune stale
    /// files, then `mark_clean`. The shadow root is the session's
    /// persistent tempdir; `dest` lives under `content/<name>/` within it,
    /// exactly as the real bundler arranges.
    fn run_collection_tick(
        session: &mut ShadowSession,
        spec: zfb_content::PipelineSpec,
        src: &Path,
        collection_name: &str,
        id_strip_suffix: Option<&str>,
        include: Option<&[String]>,
    ) -> CollectionTickOut {
        // Legacy callers: no snapshot supplied → collection skip falls back
        // to the `(mtime, size)` key (the pre-#1151 behaviour).
        run_collection_tick_with_snapshot(
            session,
            spec,
            src,
            collection_name,
            id_strip_suffix,
            include,
            None,
        )
    }

    /// Like [`run_collection_tick`] but threads an explicit
    /// [`MaterialiseCtx::snapshot_specifiers`] set so tests can exercise the
    /// #1151 SHA-accurate collection-skip gate deterministically — injecting
    /// a mismatching set simulates a coarse-mtime content edit the per-tick
    /// snapshot re-hashed, without fighting filesystem mtime granularity.
    fn run_collection_tick_with_snapshot(
        session: &mut ShadowSession,
        spec: zfb_content::PipelineSpec,
        src: &Path,
        collection_name: &str,
        id_strip_suffix: Option<&str>,
        include: Option<&[String]>,
        snapshot_specifiers: Option<std::collections::HashSet<String>>,
    ) -> CollectionTickOut {
        let shadow_root = session.shadow_root().to_path_buf();
        let dest = shadow_root.join("content").join(collection_name);
        let exclude = no_bundle_exclude();
        // project_root = src's parent so transclude/context roots (when
        // armed) have a concrete anchor above the collection.
        let project_root = src.parent().unwrap_or(src).to_path_buf();

        let fingerprint = spec_fingerprint(&spec);
        let writer = ShadowWriter::new(shadow_root, Some(session), false, fingerprint)
            .expect("session writer construction");
        let ctx = MaterialiseCtx {
            pipeline_spec: spec,
            copy_mode: false,
            bundle_exclude: &exclude,
            project_root: &project_root,
            writer: &writer,
            glob_matched_files: RefCell::new(BTreeSet::new()),
            raw_import_edges: RefCell::new(BTreeSet::new()),
            raw_import_aliases: RawImportAliasContext::empty(),
            module_worker_dependencies: RefCell::new(BTreeSet::new()),
            worker_build_context: ModuleWorkerBuildContext::default(),
            raw_preflight_complete: Cell::new(false),
            snapshot_specifiers,
        };
        let mut imports = Vec::new();
        let mut broken = Vec::new();
        let mut md = Vec::new();
        let mut cross_links = Vec::new();
        let mut headings = Vec::new();
        materialise_collection(
            src,
            &dest,
            collection_name,
            &mut imports,
            &ctx,
            include,
            None,
            id_strip_suffix,
            &mut broken,
            &mut md,
            &mut cross_links,
            &mut headings,
        )
        .expect("materialise_collection tick");
        writer.prune_stale().expect("prune");
        writer.mark_clean();
        CollectionTickOut {
            imports,
            headings,
            cross_links,
        }
    }

    /// Run ONE tick that materialises `src` through BOTH passes under a
    /// SINGLE `ShadowWriter` — the collection pass (→
    /// `content/<collection_name>/...`, with bridge imports) AND the
    /// `materialise_shadow` extra-top-level-dir pass (→
    /// `<shadow_dir_name>/...`, no bridge imports) — exactly as
    /// `bundle_with_session` does for `src/mdx/**`. Both passes must share
    /// one writer so the single prune pass at the end sees the union of
    /// their visited paths (a per-pass writer would prune the other pass's
    /// output). Returns the collection pass's imports.
    fn run_collection_and_shadow_tick(
        session: &mut ShadowSession,
        src: &Path,
        collection_name: &str,
        shadow_dir_name: &str,
    ) -> Vec<ContentImport> {
        let shadow_root = session.shadow_root().to_path_buf();
        let collection_dest = shadow_root.join("content").join(collection_name);
        let shadow_dest = shadow_root.join(shadow_dir_name);
        let exclude = no_bundle_exclude();
        let project_root = src.parent().unwrap_or(src).to_path_buf();

        let spec = zfb_content::PipelineSpec::default();
        let fingerprint = spec_fingerprint(&spec);
        let writer = ShadowWriter::new(shadow_root, Some(session), false, fingerprint)
            .expect("session writer construction");
        let ctx = MaterialiseCtx {
            pipeline_spec: spec,
            copy_mode: false,
            bundle_exclude: &exclude,
            project_root: &project_root,
            writer: &writer,
            glob_matched_files: RefCell::new(BTreeSet::new()),
            raw_import_edges: RefCell::new(BTreeSet::new()),
            raw_import_aliases: RawImportAliasContext::empty(),
            module_worker_dependencies: RefCell::new(BTreeSet::new()),
            worker_build_context: ModuleWorkerBuildContext::default(),
            raw_preflight_complete: Cell::new(false),
            // This dual-pass helper exercises the source/`materialise_shadow`
            // path (`import: None`), which is not snapshot-gated; `None` here.
            snapshot_specifiers: None,
        };
        let mut imports = Vec::new();
        materialise_collection(
            src,
            &collection_dest,
            collection_name,
            &mut imports,
            &ctx,
            None,
            None,
            None,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .expect("collection tick");
        materialise_shadow(
            src,
            &shadow_dest,
            &mut Vec::new(),
            &ctx,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .expect("shadow tick");
        writer.prune_stale().expect("prune");
        writer.mark_clean();
        imports
    }

    /// A `PipelineSpec` with transclude (and therefore a `ReadRecorder`)
    /// armed, anchored at `project_root` / `public_dir` so the
    /// context-aware plugins actually fire and record their reads.
    fn transclude_spec(project_root: &Path) -> zfb_content::PipelineSpec {
        let features = zfb_content::MarkdownFeaturesConfig {
            transclude: Some(zfb_content::TranscludeConfig::default()),
            ..Default::default()
        };
        zfb_content::PipelineSpec {
            features: Some(features),
            build_context_roots: Some((project_root.to_path_buf(), project_root.to_path_buf())),
            ..Default::default()
        }
    }

    /// The `content_skip` cache KEY (a DEST shadow-relative `PathBuf`) for
    /// a file at `rel` inside collection `name` — i.e.
    /// `content/<name>/<rel>` (zfb#1148, dest-keyed cache).
    fn collection_skip_key(name: &str, rel: &str) -> PathBuf {
        PathBuf::from(format!("content/{name}/{rel}"))
    }

    /// The pipeline `config_fingerprint` a `ShadowWriter` needs for the
    /// config-change wipe — the same value the production bundler computes
    /// from the effective spec (zfb#1148, Defect A).
    fn spec_fingerprint(spec: &zfb_content::PipelineSpec) -> Option<String> {
        spec.build_pipeline()
            .ok()
            .and_then(|p| p.config_fingerprint())
    }

    /// The specifier of the import whose `shadow_rel_path` equals
    /// `rel_path`, for tests that materialise more than one content file
    /// (e.g. a page plus its transcluded sibling).
    fn page_import_specifier(imports: &[ContentImport], rel_path: &str) -> String {
        imports
            .iter()
            .find(|i| i.shadow_rel_path == rel_path)
            .unwrap_or_else(|| panic!("no import for {rel_path}; got {imports:?}"))
            .specifier
            .clone()
    }

    #[test]
    fn content_skip_unchanged_file_is_skipped_and_still_contributes_import() {
        // Tick 1 materialises; tick 2 (source untouched) must SKIP the
        // file — reusing its cached bridge import without re-reading /
        // re-compiling / re-writing — yet still contribute the import.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("docs");
        fs::create_dir_all(&src).unwrap();
        // A heading so the file contributes a `FileHeadings` record we can
        // assert is replayed on skip (rule 2: the build-wide cross-file
        // anchor check runs every tick and must still see this file).
        fs::write(src.join("intro.mdx"), "# Intro\n\nbody one\n").unwrap();

        let mut session = ShadowSession::new(tmp.path()).unwrap();
        let out1 = run_collection_tick(
            &mut session,
            zfb_content::PipelineSpec::default(),
            &src,
            "docs",
            None,
            None,
        );
        assert_eq!(out1.imports.len(), 1, "tick 1 must produce one import");
        // A skip entry must have been recorded for the (dep-free) file,
        // keyed by its DEST shadow-relative path (zfb#1148).
        let key = collection_skip_key("docs", "intro.mdx");
        assert!(
            session.content_skip.contains_key(&key),
            "a dep-free content file must be entered into the skip cache after tick 1"
        );

        // White-box discriminator: corrupt the dest shadow file AND drop
        // its `written` hash. A full recompile would land in
        // write_if_changed's `None` branch and rewrite fresh JSX; a SKIP
        // leaves the corrupted bytes untouched. This isolates the skip
        // path from #993's identical-bytes write-elision.
        let dest_file = session
            .shadow_root()
            .join("content")
            .join("docs")
            .join("intro.mdx");
        assert!(
            dest_file.is_file(),
            "dest shadow file must exist after tick 1"
        );
        fs::write(&dest_file, b"__CORRUPTED_NOT_JSX__").unwrap();
        let rel = PathBuf::from("content/docs/intro.mdx");
        session.written.remove(&rel);

        let out2 = run_collection_tick(
            &mut session,
            zfb_content::PipelineSpec::default(),
            &src,
            "docs",
            None,
            None,
        );

        // The skipped file STILL contributes its bridge import, byte-equal
        // to tick 1 (parity), and the dest file was NOT rewritten.
        assert_eq!(
            out2.imports.len(),
            1,
            "tick 2 must still contribute the import"
        );
        assert_eq!(
            out2.imports[0].specifier, out1.imports[0].specifier,
            "skipped-file specifier must be byte-identical to tick 1"
        );
        assert_eq!(
            out2.imports[0].shadow_rel_path, out1.imports[0].shadow_rel_path,
            "skipped-file shadow_rel_path must be byte-identical to tick 1"
        );
        let after = fs::read(&dest_file).unwrap();
        assert_eq!(
            after, b"__CORRUPTED_NOT_JSX__",
            "an unchanged file must be SKIPPED — its shadow JSX must not be re-written"
        );

        // Rule 2: the skipped file must STILL contribute its cross-file
        // anchor records (FileHeadings) to the out-params, so a link from
        // a changed file to a heading here cannot falsely report broken.
        // The default pipeline does not run linkValidation, so the
        // headings channel may be empty; assert the replay is at least
        // byte-equal to tick 1 (same records, whatever they are).
        assert_eq!(
            out2.headings, out1.headings,
            "skipped-file FileHeadings replay must equal tick 1's contribution"
        );
        assert_eq!(
            out2.cross_links, out1.cross_links,
            "skipped-file cross-link replay must equal tick 1's contribution"
        );
    }

    /// A `PipelineSpec` with linkValidation armed (which populates the
    /// per-file `FileHeadings` cross-file channel), anchored at
    /// `project_root`.
    fn link_validation_spec(project_root: &Path) -> zfb_content::PipelineSpec {
        let features = zfb_content::MarkdownFeaturesConfig {
            link_validation: Some(zfb_content::LinkValidationConfig::default()),
            ..Default::default()
        };
        zfb_content::PipelineSpec {
            features: Some(features),
            build_context_roots: Some((project_root.to_path_buf(), project_root.to_path_buf())),
            ..Default::default()
        }
    }

    #[test]
    fn content_skip_replays_nonempty_headings_for_skipped_file() {
        // With linkValidation armed, a heading-only (link-free) file is
        // still dep-free, so it is skip-cached — and its NON-EMPTY
        // FileHeadings record must be replayed on skip, proving the
        // cross-file anchor check still sees the skipped file (rule 2).
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("docs");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            src.join("anchored.mdx"),
            "# Top Heading\n\n## Second Heading\n\nbody\n",
        )
        .unwrap();

        let mut session = ShadowSession::new(tmp.path()).unwrap();
        let out1 = run_collection_tick(
            &mut session,
            link_validation_spec(tmp.path()),
            &src,
            "docs",
            None,
            None,
        );
        // The file is dep-free (no link reads) ⇒ skip-cached, and it
        // surfaced a non-empty headings record.
        let key = collection_skip_key("docs", "anchored.mdx");
        assert!(
            session.content_skip.contains_key(&key),
            "a heading-only, link-free file must be skip-cached even with linkValidation on"
        );
        assert_eq!(
            out1.headings.len(),
            1,
            "exactly one FileHeadings record for the file"
        );
        assert!(
            !out1.headings[0].headings.is_empty(),
            "the file has anchor-addressable headings — the record must be non-empty"
        );

        let out2 = run_collection_tick(
            &mut session,
            link_validation_spec(tmp.path()),
            &src,
            "docs",
            None,
            None,
        );
        // Tick 2 skips, yet replays the identical non-empty headings.
        assert_eq!(
            out2.headings, out1.headings,
            "skip must replay the file's non-empty FileHeadings byte-for-byte"
        );
    }

    #[test]
    fn content_skip_changed_file_is_rematerialised() {
        // A file whose bytes (and so size/mtime) change must take the
        // full path on tick 2: the dest is rewritten with fresh JSX.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("docs");
        fs::create_dir_all(&src).unwrap();
        let file = src.join("intro.mdx");
        fs::write(&file, "# Intro\n\nbody one\n").unwrap();

        let mut session = ShadowSession::new(tmp.path()).unwrap();
        let out1 = run_collection_tick(
            &mut session,
            zfb_content::PipelineSpec::default(),
            &src,
            "docs",
            None,
            None,
        );

        // Change the body (different length ⇒ size differs ⇒ skip key
        // misses even if mtime granularity is coarse).
        fs::write(&file, "# Intro\n\nbody two is a bit longer than one\n").unwrap();

        let out2 = run_collection_tick(
            &mut session,
            zfb_content::PipelineSpec::default(),
            &src,
            "docs",
            None,
            None,
        );

        // Different body ⇒ different content hash ⇒ different specifier.
        assert_eq!(out2.imports.len(), 1);
        assert_ne!(
            out2.imports[0].specifier, out1.imports[0].specifier,
            "a changed file must recompile to a fresh specifier (no false skip)"
        );
        // The shadow JSX reflects the new body.
        let dest_file = session
            .shadow_root()
            .join("content")
            .join("docs")
            .join("intro.mdx");
        let jsx = fs::read_to_string(&dest_file).unwrap();
        assert!(
            jsx.contains("_createMdxContent"),
            "changed file must be recompiled to JSX; got:\n{jsx}"
        );
    }

    #[test]
    fn content_skip_collection_snapshot_hash_mismatch_forces_rematerialise() {
        // zfb#1151 regression (the bug): a collection .mdx whose CONTENT
        // changed while `(mtime, size)` was preserved must NOT be falsely
        // skipped. We reproduce the mechanism deterministically — without
        // fighting filesystem mtime granularity — by leaving the file
        // byte-identical on disk across both ticks (so `(mtime, size)` is
        // provably unchanged) while supplying a tick-2 snapshot specifier set
        // that does NOT contain the stored bridge specifier. That models a
        // coarse-mtime content edit the per-tick snapshot re-hashed: the snapshot
        // now bakes a different `mdx://…#hash`, so the cached specifier drops out
        // of the set and the skip MUST be invalidated. Pre-fix (no snapshot
        // gate) this would replay the stale specifier and serve a broken
        // raw-markdown `<pre>` page.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("docs");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("intro.mdx"), "# Intro\n\nbody one\n").unwrap();

        let mut session = ShadowSession::new(tmp.path()).unwrap();
        // Tick 1: full materialise, stores the skip entry (snapshot irrelevant
        // on the full path — entries are always stored).
        let out1 = run_collection_tick(
            &mut session,
            zfb_content::PipelineSpec::default(),
            &src,
            "docs",
            None,
            None,
        );
        assert_eq!(out1.imports.len(), 1);
        let key = collection_skip_key("docs", "intro.mdx");
        assert!(
            session.content_skip.contains_key(&key),
            "collection file must be skip-cached after tick 1"
        );

        // White-box discriminator: corrupt the dest + drop its `written` hash.
        // A SKIP leaves the corruption; a full recompile rewrites fresh JSX.
        let dest_file = session
            .shadow_root()
            .join("content")
            .join("docs")
            .join("intro.mdx");
        fs::write(&dest_file, b"__CORRUPTED_NOT_JSX__").unwrap();
        session
            .written
            .remove(&PathBuf::from("content/docs/intro.mdx"));

        // Tick 2: file untouched on disk (so `(mtime, size)` matches and deps
        // are unchanged — the ONLY thing that can force a recompile is the
        // snapshot gate), but the snapshot set lacks the stored specifier.
        let empty: std::collections::HashSet<String> = std::collections::HashSet::new();
        let out2 = run_collection_tick_with_snapshot(
            &mut session,
            zfb_content::PipelineSpec::default(),
            &src,
            "docs",
            None,
            None,
            Some(empty),
        );

        // Must have RE-materialised despite unchanged `(mtime, size)`.
        assert_eq!(out2.imports.len(), 1);
        let jsx = fs::read_to_string(&dest_file).unwrap();
        assert!(
            jsx.contains("_createMdxContent"),
            "a snapshot-hash mismatch must force a full recompile even with \
             unchanged (mtime,size); got:\n{jsx}"
        );
    }

    #[test]
    fn content_skip_collection_snapshot_hash_match_still_skips() {
        // zfb#1151 positive parity: when the snapshot set STILL contains the
        // stored specifier (content genuinely unchanged), the SHA gate is a
        // no-op and the file is skipped exactly as before — dest left
        // untouched and the import replayed byte-for-byte. Proves the gate does
        // not over-invalidate the happy path.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("docs");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("intro.mdx"), "# Intro\n\nbody one\n").unwrap();

        let mut session = ShadowSession::new(tmp.path()).unwrap();
        let out1 = run_collection_tick(
            &mut session,
            zfb_content::PipelineSpec::default(),
            &src,
            "docs",
            None,
            None,
        );
        assert_eq!(out1.imports.len(), 1);
        let specifier = out1.imports[0].specifier.clone();

        let dest_file = session
            .shadow_root()
            .join("content")
            .join("docs")
            .join("intro.mdx");
        fs::write(&dest_file, b"__CORRUPTED_NOT_JSX__").unwrap();
        session
            .written
            .remove(&PathBuf::from("content/docs/intro.mdx"));

        // Tick 2: snapshot set contains the stored specifier → SKIP honoured.
        let set: std::collections::HashSet<String> =
            std::collections::HashSet::from([specifier.clone()]);
        let out2 = run_collection_tick_with_snapshot(
            &mut session,
            zfb_content::PipelineSpec::default(),
            &src,
            "docs",
            None,
            None,
            Some(set),
        );

        assert_eq!(out2.imports.len(), 1);
        assert_eq!(
            out2.imports[0].specifier, specifier,
            "skipped-file specifier must be replayed byte-identical to tick 1"
        );
        let after = fs::read(&dest_file).unwrap();
        assert_eq!(
            after, b"__CORRUPTED_NOT_JSX__",
            "a matching snapshot hash must still SKIP — dest not re-written"
        );
    }

    #[test]
    fn content_skip_no_snapshot_falls_back_to_mtime_size() {
        // zfb#1151 fallback: with NO snapshot supplied (snapshot_specifiers ==
        // None) the collection skip degrades to the legacy `(mtime, size)`
        // key — an unchanged file still skips. Guards the
        // passthrough/snapshot-absent path so the gate never breaks existing
        // behaviour when the snapshot is unavailable.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("docs");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("intro.mdx"), "# Intro\n\nbody one\n").unwrap();

        let mut session = ShadowSession::new(tmp.path()).unwrap();
        let _out1 = run_collection_tick(
            &mut session,
            zfb_content::PipelineSpec::default(),
            &src,
            "docs",
            None,
            None,
        );

        let dest_file = session
            .shadow_root()
            .join("content")
            .join("docs")
            .join("intro.mdx");
        fs::write(&dest_file, b"__CORRUPTED_NOT_JSX__").unwrap();
        session
            .written
            .remove(&PathBuf::from("content/docs/intro.mdx"));

        // Tick 2: no snapshot, file unchanged → legacy `(mtime, size)` skip.
        let out2 = run_collection_tick_with_snapshot(
            &mut session,
            zfb_content::PipelineSpec::default(),
            &src,
            "docs",
            None,
            None,
            None,
        );

        assert_eq!(out2.imports.len(), 1);
        let after = fs::read(&dest_file).unwrap();
        assert_eq!(
            after, b"__CORRUPTED_NOT_JSX__",
            "with no snapshot, an unchanged file must still SKIP via (mtime,size)"
        );
    }

    #[test]
    fn snapshot_specifier_set_parses_and_degrades() {
        // zfb#1151: unit-cover the helper itself — None input, malformed JSON
        // (→ None, degrade to the legacy key, never fail the build), and a
        // multi-collection snapshot (every entry's module_specifier collected
        // into the flat cross-collection set).
        assert!(snapshot_specifier_set(None).is_none());
        assert!(
            snapshot_specifier_set(Some("{ not valid json")).is_none(),
            "malformed JSON must degrade to None, not panic"
        );

        let json = r#"{
            "collections": {
                "docs": [
                    {"slug":"intro","frontmatter":null,"body":"","module_specifier":"mdx://docs/intro#aaaa1111","rel_path":"intro.mdx"}
                ],
                "blog": [
                    {"slug":"post","frontmatter":null,"body":"","module_specifier":"mdx://blog/post#bbbb2222","rel_path":"post.mdx"}
                ]
            }
        }"#;
        let set = snapshot_specifier_set(Some(json)).expect("valid snapshot parses");
        assert_eq!(set.len(), 2);
        assert!(set.contains("mdx://docs/intro#aaaa1111"));
        assert!(set.contains("mdx://blog/post#bbbb2222"));
    }

    #[test]
    fn content_skip_file_with_unchanged_deps_is_skipped() {
        // Thorough dep-mtime variant (zfb#1148): a file that transcludes
        // another (a recorded read) IS skip-cached, and on a later tick —
        // where neither the file NOR its transcluded dep changed — it is
        // SKIPPED, reusing its import without re-reading/re-compiling/
        // re-writing.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("docs");
        fs::create_dir_all(&src).unwrap();
        let page = src.join("page.mdx");
        fs::write(&page, "# Page\n\n:::include{file=\"./snippet.md\"}\n").unwrap();
        fs::write(src.join("snippet.md"), "included body\n").unwrap();

        let mut session = ShadowSession::new(tmp.path()).unwrap();
        let out1 = run_collection_tick(
            &mut session,
            transclude_spec(tmp.path()),
            &src,
            "docs",
            None,
            None,
        );
        // Two imports: the transcluding page.mdx AND snippet.md (which is
        // also a content file in the collection). We track the page's.
        let page_spec_1 = page_import_specifier(&out1.imports, "content/docs/page.mdx");
        // The transcluding file IS cached, and its entry records a
        // non-empty dep set (it read snippet.md). Keyed by DEST path.
        let page_key = collection_skip_key("docs", "page.mdx");
        let entry = session
            .content_skip
            .get(&page_key)
            .expect("a transcluding file must now be skip-cached (dep-mtime variant)");
        assert!(
            !entry.deps.is_empty(),
            "the transcluding file's entry must record its transcluded dep"
        );

        // White-box discriminator (same as the dep-free skip test):
        // corrupt the dest + drop its `written` hash. A SKIP leaves the
        // corruption; a full recompile rewrites fresh JSX.
        let dest_file = session
            .shadow_root()
            .join("content")
            .join("docs")
            .join("page.mdx");
        fs::write(&dest_file, b"__CORRUPTED__").unwrap();
        let rel = PathBuf::from("content/docs/page.mdx");
        session.written.remove(&rel);

        // Tick 2: nothing changed → SKIP.
        let out2 = run_collection_tick(
            &mut session,
            transclude_spec(tmp.path()),
            &src,
            "docs",
            None,
            None,
        );
        let page_spec_2 = page_import_specifier(&out2.imports, "content/docs/page.mdx");
        assert_eq!(
            page_spec_2, page_spec_1,
            "skipped transcluding file must replay its import byte-for-byte"
        );
        let after = fs::read(&dest_file).unwrap();
        assert_eq!(
            after, b"__CORRUPTED__",
            "a file with UNCHANGED deps must be SKIPPED — dest not re-written"
        );
    }

    #[test]
    fn content_skip_dep_change_invalidates_dependent() {
        // When a transcluded dep changes, the dependent file must take the
        // full path (re-validate / re-rewrite) even though its OWN
        // bytes/mtime are untouched. We change the dep's SIZE (different
        // content length) so the invalidation is granularity-independent.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("docs");
        fs::create_dir_all(&src).unwrap();
        let page = src.join("page.mdx");
        let snippet = src.join("snippet.md");
        fs::write(&page, "# Page\n\n:::include{file=\"./snippet.md\"}\n").unwrap();
        fs::write(&snippet, "v1\n").unwrap();

        let mut session = ShadowSession::new(tmp.path()).unwrap();
        let out1 = run_collection_tick(
            &mut session,
            transclude_spec(tmp.path()),
            &src,
            "docs",
            None,
            None,
        );
        assert!(session
            .content_skip
            .contains_key(&collection_skip_key("docs", "page.mdx")));
        let page_spec_1 = page_import_specifier(&out1.imports, "content/docs/page.mdx");

        // Corrupt the dest + drop its written hash: now a SKIP would leave
        // the corruption, a full recompile rewrites fresh JSX.
        let dest_file = session
            .shadow_root()
            .join("content")
            .join("docs")
            .join("page.mdx");
        fs::write(&dest_file, b"__CORRUPTED__").unwrap();
        let rel = PathBuf::from("content/docs/page.mdx");
        session.written.remove(&rel);

        // Change ONLY the transcluded dep (different length ⇒ size differs).
        fs::write(
            &snippet,
            "a much longer second version of the snippet body\n",
        )
        .unwrap();

        // Tick 2: page bytes unchanged, but its dep changed → full path.
        let out2 = run_collection_tick(
            &mut session,
            transclude_spec(tmp.path()),
            &src,
            "docs",
            None,
            None,
        );
        let page_spec_2 = page_import_specifier(&out2.imports, "content/docs/page.mdx");
        let after = fs::read_to_string(&dest_file).unwrap();
        assert!(
            after.contains("_createMdxContent"),
            "a dep change must FORCE re-materialise of the dependent file (no false skip); got:\n{after}"
        );
        // The transcluded content changed, so the compiled JSX (and hence
        // the content hash / specifier) differs from tick 1.
        assert_ne!(
            page_spec_2, page_spec_1,
            "the re-materialised page must reflect the new transcluded content"
        );
    }

    #[test]
    fn content_skip_add_then_delete_prunes_with_no_stale_shadow() {
        // Tick 1: only a.mdx exists. Tick 2: add b.mdx (materialises).
        // Tick 3: delete b.mdx — it is no longer walked, so the prune
        // pass must remove its shadow file and no stale entry survives.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("docs");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.mdx"), "# A\n\naye\n").unwrap();

        let mut session = ShadowSession::new(tmp.path()).unwrap();
        run_collection_tick(
            &mut session,
            zfb_content::PipelineSpec::default(),
            &src,
            "docs",
            None,
            None,
        );

        // Add b.mdx.
        let b = src.join("b.mdx");
        fs::write(&b, "# B\n\nbee\n").unwrap();
        let out2 = run_collection_tick(
            &mut session,
            zfb_content::PipelineSpec::default(),
            &src,
            "docs",
            None,
            None,
        );
        assert_eq!(out2.imports.len(), 2, "tick 2 must materialise both files");
        let b_dest = session
            .shadow_root()
            .join("content")
            .join("docs")
            .join("b.mdx");
        assert!(b_dest.is_file(), "b.mdx shadow file must exist after add");

        // Delete b.mdx and re-run: it is not walked, so the prune pass
        // must delete its shadow file and drop the skip entry.
        fs::remove_file(&b).unwrap();
        let out3 = run_collection_tick(
            &mut session,
            zfb_content::PipelineSpec::default(),
            &src,
            "docs",
            None,
            None,
        );
        assert_eq!(
            out3.imports.len(),
            1,
            "tick 3 must drop the deleted file's import"
        );
        assert!(
            !b_dest.exists(),
            "the deleted file's shadow copy must be pruned (no stale module)"
        );
        // a.mdx must still be present and skipped-but-contributing.
        let a_dest = session
            .shadow_root()
            .join("content")
            .join("docs")
            .join("a.mdx");
        assert!(
            a_dest.is_file(),
            "the surviving file's shadow copy must remain"
        );
    }

    #[test]
    fn content_skip_strip_suffix_parity_skipped_equals_full_compile() {
        // The skipped-file bridge import for an `idStripSuffix` collection
        // must be byte-identical to the full-compile import (the
        // snapshot↔bridge specifier-parity invariant). We obtain the
        // ground-truth full-compile import from a SEPARATE fresh session
        // (so it never skips), and compare it against the SKIP-produced
        // import from a reused session.
        let strip = Some(".en");
        let body = "# Guide\n\nstrip-suffix body\n";

        // Ground truth: a fresh session, single tick → full compile.
        let tmp_truth = tempfile::tempdir().unwrap();
        let src_truth = tmp_truth.path().join("docs");
        fs::create_dir_all(&src_truth).unwrap();
        fs::write(src_truth.join("guide.en.mdx"), body).unwrap();
        let mut truth_session = ShadowSession::new(tmp_truth.path()).unwrap();
        let truth = run_collection_tick(
            &mut truth_session,
            zfb_content::PipelineSpec::default(),
            &src_truth,
            "docs",
            strip,
            None,
        );
        assert_eq!(truth.imports.len(), 1);
        let full_specifier = truth.imports[0].specifier.clone();
        // Sanity: the `.en` suffix is stripped from the slug segment.
        assert!(
            !full_specifier.contains(".en#") && !full_specifier.contains(".en/"),
            "idStripSuffix must strip `.en` from the specifier slug; got {full_specifier}"
        );

        // Skip path: same source in a reused session; tick 2 skips.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("docs");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("guide.en.mdx"), body).unwrap();
        let mut session = ShadowSession::new(tmp.path()).unwrap();
        run_collection_tick(
            &mut session,
            zfb_content::PipelineSpec::default(),
            &src,
            "docs",
            strip,
            None,
        );
        assert!(
            session
                .content_skip
                .contains_key(&collection_skip_key("docs", "guide.en.mdx")),
            "strip-suffix file must be skip-cached"
        );
        let out2 = run_collection_tick(
            &mut session,
            zfb_content::PipelineSpec::default(),
            &src,
            "docs",
            strip,
            None,
        );
        assert_eq!(out2.imports.len(), 1);
        assert_eq!(
            out2.imports[0].specifier, full_specifier,
            "the skipped-file specifier MUST byte-equal the full-compile specifier (parity)"
        );
    }

    #[test]
    fn content_skip_sessionless_never_skips() {
        // The sessionless (`bundle()` / prod) path must never engage the
        // skip cache: passthrough writers report `in_session() == false`,
        // so every tick takes the full materialise path. Two runs through
        // the passthrough ctx both write the dest fresh.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("docs");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("intro.mdx"), "# Intro\n\nbody\n").unwrap();
        let dest = tmp.path().join("shadow").join("content").join("docs");

        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(tmp.path(), &exclude);
        assert!(
            !ctx.writer.in_session(),
            "default (passthrough) test writer must report not-in-session"
        );

        let mut imports = Vec::new();
        materialise_collection(
            &src,
            &dest,
            "docs",
            &mut imports,
            &ctx,
            None,
            None,
            None,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();
        // Corrupt the dest, then re-run: a passthrough write always
        // remove-firsts and rewrites (no write-if-changed elision, no
        // skip), so the dest is fresh JSX again.
        let dest_file = dest.join("intro.mdx");
        fs::write(&dest_file, b"__CORRUPTED__").unwrap();
        let mut imports2 = Vec::new();
        materialise_collection(
            &src,
            &dest,
            "docs",
            &mut imports2,
            &ctx,
            None,
            None,
            None,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();
        let jsx = fs::read_to_string(&dest_file).unwrap();
        assert!(
            jsx.contains("_createMdxContent"),
            "sessionless re-run must re-materialise fresh JSX (no skip); got:\n{jsx}"
        );
        assert_eq!(imports2.len(), 1);
    }

    #[test]
    fn content_skip_same_source_two_dests_two_independent_entries_both_skip() {
        // The SAME source `.mdx` is materialised into TWO distinct shadow
        // dests each tick — `content/docs/foo.mdx` (collection pass, with a
        // bridge import) and `src/foo.mdx` (the `materialise_shadow`
        // extra-top-level-dir pass, no bridge import). Dest-keying gives
        // each pass its own independent skip entry, and on the 2nd tick
        // (source untouched) BOTH must skip — no re-write to either dest.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("docs");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("foo.mdx"), "# Foo\n\nbody\n").unwrap();

        let mut session = ShadowSession::new(tmp.path()).unwrap();
        let imports1 = run_collection_and_shadow_tick(&mut session, &src, "docs", "src");
        // The collection pass produced the bridge import for foo.
        assert_eq!(imports1.len(), 1, "collection pass produces one import");
        let collection_spec = imports1[0].specifier.clone();

        // Two independent entries: one per dest.
        let collection_key = collection_skip_key("docs", "foo.mdx");
        let shadow_key = PathBuf::from("src/foo.mdx");
        let collection_entry = session
            .content_skip
            .get(&collection_key)
            .expect("collection-pass dest must be skip-cached");
        let shadow_entry = session
            .content_skip
            .get(&shadow_key)
            .expect("materialise_shadow-pass dest must be skip-cached");
        // The collection entry carries a bridge import; the shadow one does not.
        assert!(
            collection_entry.import.is_some(),
            "collection-pass entry must carry the bridge import"
        );
        assert!(
            shadow_entry.import.is_none(),
            "materialise_shadow-pass entry must carry NO bridge import"
        );
        // Both describe the same source.
        let foo = src.join("foo.mdx");
        assert_eq!(collection_entry.source, foo);
        assert_eq!(shadow_entry.source, foo);

        // White-box: corrupt BOTH dests + drop their `written` hashes. A
        // full recompile of either would rewrite fresh JSX; a SKIP leaves
        // the corruption.
        let coll_dest = session.shadow_root().join("content/docs/foo.mdx");
        let shadow_dest = session.shadow_root().join("src/foo.mdx");
        assert!(coll_dest.is_file() && shadow_dest.is_file());
        fs::write(&coll_dest, b"__CORRUPT_COLL__").unwrap();
        fs::write(&shadow_dest, b"__CORRUPT_SHADOW__").unwrap();
        session.written.remove(&collection_key);
        session.written.remove(&shadow_key);

        // Tick 2: source untouched → BOTH passes skip.
        let imports2 = run_collection_and_shadow_tick(&mut session, &src, "docs", "src");
        assert_eq!(imports2.len(), 1);
        assert_eq!(
            imports2[0].specifier, collection_spec,
            "collection bridge import replayed byte-for-byte on skip"
        );
        assert_eq!(
            fs::read(&coll_dest).unwrap(),
            b"__CORRUPT_COLL__",
            "collection dest must be SKIPPED (not re-written) on tick 2"
        );
        assert_eq!(
            fs::read(&shadow_dest).unwrap(),
            b"__CORRUPT_SHADOW__",
            "materialise_shadow dest must be SKIPPED (not re-written) on tick 2"
        );
    }

    /// A `PipelineSpec` with `resolve_source_map` armed (the
    /// `resolveMarkdownLinks` feature), mapping `target_key` → `target_url`.
    /// `ResolveLinksPlugin` rewrites `./target.mdx` links to the URL but
    /// records NOTHING through the ReadRecorder — so the page's deps are
    /// empty and a map change is invisible to the per-dep stat check
    /// (the Defect-A hazard).
    fn resolve_links_spec(target_key: &Path, target_url: &str) -> zfb_content::PipelineSpec {
        let mut map = HashMap::new();
        map.insert(target_key.to_path_buf(), target_url.to_string());
        zfb_content::PipelineSpec {
            resolve_source_map: Some(map),
            ..Default::default()
        }
    }

    #[test]
    fn content_skip_resolve_source_map_change_forces_rematerialise() {
        // Defect A (zfb#1148): a resolve-links page links to a target via
        // the IN-MEMORY route→URL map. `ResolveLinksPlugin` records no dep,
        // so the page's `deps` are empty. When the map's URL for the target
        // changes between ticks (e.g. the target was renamed / re-slugged),
        // the page's rewritten link — and thus its compiled JSX / content
        // hash / bridge specifier — MUST change, even though the page's own
        // source `(mtime, size)` is untouched. The config-fingerprint wipe
        // is what catches this: the map digest is folded into
        // `config_fingerprint`, so a map change wipes the skip caches and
        // forces a full re-materialise. Without the fix the page would be
        // wrongly skipped → stale URL.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("docs");
        fs::create_dir_all(&src).unwrap();
        // The page links to ./target.mdx; the resolver keys the lookup on
        // the absolute path of the link target (source_dir = page's dir).
        fs::write(
            src.join("page.mdx"),
            "# Page\n\n[see target](./target.mdx)\n",
        )
        .unwrap();
        let target_key = src.join("target.mdx");

        let mut session = ShadowSession::new(tmp.path()).unwrap();
        // Tick 1: target maps to /docs/target/.
        let spec1 = resolve_links_spec(&target_key, "/docs/target/");
        let out1 = run_collection_tick(&mut session, spec1, &src, "docs", None, None);
        let spec_1 = page_import_specifier(&out1.imports, "content/docs/page.mdx");
        // The page IS skip-cached (resolve-links pages have empty deps, so
        // they would otherwise be eagerly skipped — exactly the hazard).
        assert!(session
            .content_skip
            .contains_key(&collection_skip_key("docs", "page.mdx")));
        // Sanity: the rewritten URL is in the compiled shadow JSX.
        let dest = session.shadow_root().join("content/docs/page.mdx");
        assert!(
            fs::read_to_string(&dest).unwrap().contains("/docs/target/"),
            "tick 1 must rewrite the link to /docs/target/"
        );

        // Tick 2: the SAME page source (mtime/size unchanged), but the map
        // now points the target at a NEW url ⇒ the config fingerprint
        // changes ⇒ skip caches wiped ⇒ page re-materialised with the new
        // rewritten URL and a new specifier.
        let spec2 = resolve_links_spec(&target_key, "/docs/renamed-target/");
        let out2 = run_collection_tick(&mut session, spec2, &src, "docs", None, None);
        let spec_2 = page_import_specifier(&out2.imports, "content/docs/page.mdx");

        assert_ne!(
            spec_2, spec_1,
            "a resolve_source_map change MUST re-materialise the page (new specifier), not skip it"
        );
        assert!(
            fs::read_to_string(&dest)
                .unwrap()
                .contains("/docs/renamed-target/"),
            "the re-materialised page must reflect the NEW map URL"
        );
    }

    /// Run ONE tick that materialises each `(src_file, dest_rel)` pair
    /// through `materialise_source_file` against `session`, mirroring the
    /// writer lifecycle (build writer + wipe, materialise, prune,
    /// mark_clean). `dest_rel` is shadow-root-relative (forward slashes).
    /// Returns nothing; tests inspect the session + shadow tree.
    fn run_source_tick(session: &mut ShadowSession, files: &[(&Path, &str)]) {
        let shadow_root = session.shadow_root().to_path_buf();
        let fingerprint = spec_fingerprint(&zfb_content::PipelineSpec::default());
        let writer = ShadowWriter::new(shadow_root.clone(), Some(session), false, fingerprint)
            .expect("session writer construction");
        let glob_matched_files = RefCell::new(BTreeSet::new());
        let raw_import_edges = RefCell::new(BTreeSet::new());
        let module_worker_dependencies = RefCell::new(BTreeSet::new());
        for (from, dest_rel) in files {
            let to = shadow_root.join(dest_rel);
            if let Some(parent) = to.parent() {
                writer.ensure_dir(parent).expect("ensure dest parent dir");
            }
            materialise_source_file(
                from,
                from,
                &to,
                &|_| false,
                false,
                &writer,
                &glob_matched_files,
                &raw_import_edges,
                &RawImportAliasContext::empty(),
                &module_worker_dependencies,
                from.parent().unwrap_or(from),
                &ModuleWorkerBuildContext::default(),
            )
            .expect("materialise_source_file tick");
        }
        writer.prune_stale().expect("prune");
        writer.mark_clean();
    }

    #[test]
    fn source_skip_plain_file_is_skipped_on_second_tick() {
        // A plain (non-glob) `.ts` source is symlinked on tick 1 and
        // SKIPPED on tick 2 (source untouched). White-box: replace the
        // dest with a corrupt REGULAR file + drop its `written` hash — a
        // full re-materialise would re-symlink/overwrite; a skip leaves
        // the corruption untouched.
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("util.ts");
        fs::write(&from, "export const x = 1;\n").unwrap();
        let dest_rel = "src/util.ts";

        let mut session = ShadowSession::new(tmp.path()).unwrap();
        run_source_tick(&mut session, &[(&from, dest_rel)]);
        // Entry recorded, non-glob.
        let key = PathBuf::from(dest_rel);
        let entry = session
            .source_skip
            .get(&key)
            .expect("plain source file must be skip-cached");
        assert!(
            !entry.has_glob,
            "a non-glob file's entry must record has_glob=false"
        );
        assert_eq!(entry.source, from);

        // White-box discriminator: a real file with distinct bytes + no
        // `written` hash. A full path would re-symlink (removing this file
        // and creating the link); a skip leaves it.
        let dest = session.shadow_root().join(dest_rel);
        fs::remove_file(&dest).ok();
        fs::write(&dest, b"__CORRUPT_NOT_THE_SOURCE__").unwrap();
        session.written.remove(&key);

        run_source_tick(&mut session, &[(&from, dest_rel)]);
        assert_eq!(
            fs::read(&dest).unwrap(),
            b"__CORRUPT_NOT_THE_SOURCE__",
            "an unchanged plain file must be SKIPPED — dest not re-materialised"
        );
    }

    #[test]
    fn module_worker_importer_rehashes_when_worker_only_bytes_change() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let importer = root.join("src/Island.tsx");
        let worker = root.join("src/search.worker.ts");
        fs::create_dir_all(importer.parent().unwrap()).unwrap();
        fs::write(
            &importer,
            "'use client'; export function Island() { new Worker(new URL('./search.worker.ts', import.meta.url), { type: 'module' }); return null; }\n",
        )
        .unwrap();
        fs::write(&worker, "self.postMessage('a');\n").unwrap();

        let mut session = ShadowSession::new(root).unwrap();
        let staged_rel = Path::new("src/Island.tsx");
        let run_tick = |session: &mut ShadowSession| {
            let shadow_root = session.shadow_root().to_path_buf();
            let fingerprint = spec_fingerprint(&zfb_content::PipelineSpec::default());
            let writer =
                ShadowWriter::new(shadow_root.clone(), Some(session), false, fingerprint).unwrap();
            let glob_matched_files = RefCell::new(BTreeSet::new());
            let raw_edges = RefCell::new(BTreeSet::new());
            let worker_dependencies = RefCell::new(BTreeSet::new());
            let staged = shadow_root.join(staged_rel);
            writer.ensure_dir(staged.parent().unwrap()).unwrap();
            materialise_source_file(
                &importer,
                &importer,
                &staged,
                &|_| false,
                false,
                &writer,
                &glob_matched_files,
                &raw_edges,
                &RawImportAliasContext::empty(),
                &worker_dependencies,
                root,
                &ModuleWorkerBuildContext::default(),
            )
            .unwrap();
            let body = fs::read_to_string(&staged).unwrap();
            let deps = worker_dependencies.into_inner();
            writer.prune_stale().unwrap();
            writer.mark_clean();
            (body, deps)
        };

        let (first, first_deps) = run_tick(&mut session);
        assert!(first.contains(".js?v="), "{first}");
        assert!(first_deps.iter().any(|edge| edge.dependency == worker));
        assert!(
            session
                .source_skip
                .get(staged_rel)
                .is_some_and(|entry| entry.has_worker),
            "worker-bearing source must refuse the stat-only importer skip"
        );

        // Same byte length and untouched importer: only the browser worker
        // changes. `has_worker` must force the importer rewrite anyway.
        fs::write(&worker, "self.postMessage('b');\n").unwrap();
        let (second, _) = run_tick(&mut session);
        assert_ne!(first, second, "worker-only edit must change the v= query");
    }

    #[test]
    fn ssr_worker_island_bundles_without_browser_entry_in_server_graph() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let components = root.join("components");
        fs::create_dir_all(&components).unwrap();
        fs::write(
            components.join("Island.tsx"),
            "'use client'; export function Island() { new Worker(new URL('./search.worker.ts', import.meta.url), { type: 'module' }); return null; }\n",
        )
        .unwrap();
        fs::write(
            components.join("search.worker.ts"),
            "const BROWSER_ONLY_SENTINEL = 'worker-must-not-enter-ssr'; self.postMessage(BROWSER_ONLY_SENTINEL);\n",
        )
        .unwrap();

        let shadow = tempfile::tempdir().unwrap();
        let shadow_components = shadow.path().join("components");
        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(root, &exclude);
        materialise_shadow(
            &components,
            &shadow_components,
            &mut Vec::new(),
            &ctx,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();

        let staged_island = shadow_components.join("Island.tsx");
        let staged = fs::read_to_string(&staged_island).unwrap();
        assert!(staged
            .contains("new Worker(new URL(\"./worker-components-s-search-d-worker-d-ts.js?v="));
        assert!(staged.contains(".js?v="), "{staged}");
        assert!(!staged.contains("new URL('./search.worker.ts'"), "{staged}");

        // Level 1 + real compiler seam when the pinned binary is staged: the
        // rewritten island itself bundles, but the browser-only worker entry
        // cannot appear because the transform injected no import edge.
        let Some(esbuild) = locate_real_esbuild() else {
            eprintln!(
                "[ssr_worker_island_bundles_without_browser_entry_in_server_graph] no esbuild binary; structural assertions completed"
            );
            return;
        };
        let output = shadow.path().join("ssr-island.mjs");
        let result = std::process::Command::new(esbuild)
            .arg(&staged_island)
            .arg("--bundle")
            .arg("--platform=neutral")
            .arg("--format=esm")
            .arg(format!("--outfile={}", output.display()))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "SSR-like esbuild pass failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let bundled = fs::read_to_string(output).unwrap();
        assert!(bundled.contains("worker-components-s-search-d-worker-d-ts.js?v="));
        assert!(!bundled.contains("BROWSER_ONLY_SENTINEL"), "{bundled}");
        assert!(!bundled.contains("worker-must-not-enter-ssr"), "{bundled}");
    }

    #[test]
    fn source_skip_glob_file_is_never_skipped() {
        // A `.ts` file using `import.meta.glob` must be re-expanded every
        // tick (its expansion depends on the live tree). It gets an entry
        // (has_glob=true) but the skip gate refuses it. White-box: corrupt
        // the dest + drop its written hash; tick 2 must re-expand and
        // overwrite with the expanded barrel.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("styleguide");
        fs::create_dir_all(dir.join("widgets")).unwrap();
        fs::write(dir.join("widgets/a.tsx"), "export const a = 1;\n").unwrap();
        let from = dir.join("barrel.ts");
        fs::write(
            &from,
            "const m = import.meta.glob('./widgets/*.tsx', { eager: true });\nexport default m;\n",
        )
        .unwrap();
        let dest_rel = "src/styleguide/barrel.ts";

        let mut session = ShadowSession::new(tmp.path()).unwrap();
        run_source_tick(&mut session, &[(&from, dest_rel)]);
        let key = PathBuf::from(dest_rel);
        let entry = session
            .source_skip
            .get(&key)
            .expect("glob file is still entered into the cache");
        assert!(
            entry.has_glob,
            "a glob file's entry must record has_glob=true"
        );
        // The dest holds the EXPANDED barrel (macro removed).
        let dest = session.shadow_root().join(dest_rel);
        let first = fs::read_to_string(&dest).unwrap();
        assert!(
            !first.contains("import.meta.glob(") && first.contains("__glob_0"),
            "tick 1 must expand the glob; got:\n{first}"
        );

        // Corrupt + drop written hash: a skip would leave the corruption, a
        // re-expand overwrites with the barrel again.
        fs::write(&dest, b"__CORRUPT__").unwrap();
        session.written.remove(&key);
        run_source_tick(&mut session, &[(&from, dest_rel)]);
        let second = fs::read_to_string(&dest).unwrap();
        assert!(
            !second.contains("import.meta.glob(") && second.contains("__glob_0"),
            "a glob file must be RE-EXPANDED every tick (never skipped); got:\n{second}"
        );
    }

    #[test]
    fn source_skip_raw_target_change_rewrites_generated_module_on_tick_two() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("noise.frag");
        let importer = tmp.path().join("shader.ts");
        fs::write(&target, "AAAA\n").unwrap();
        fs::write(
            &importer,
            "import shader from './noise.frag?raw';\nexport default shader;\n",
        )
        .unwrap();

        let dest_rel = "src/shader.ts";
        let mut session = ShadowSession::new(tmp.path()).unwrap();
        run_source_tick(&mut session, &[(&importer, dest_rel)]);
        let key = PathBuf::from(dest_rel);
        let entry = session
            .source_skip
            .get(&key)
            .expect("raw importer must be skip-cached");
        assert!(entry.has_raw, "raw importer must refuse stat-only skips");

        let generated = fs::read_dir(session.shadow_root().join("src"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".zfb-raw-") && name.ends_with(".mjs"))
            })
            .expect("generated raw module");
        assert!(fs::read_to_string(&generated).unwrap().contains("AAAA"));

        // Same byte length; importer is untouched. The has_raw gate must
        // still re-read the terminal target and update the generated module.
        fs::write(&target, "BBBB\n").unwrap();
        run_source_tick(&mut session, &[(&importer, dest_rel)]);
        let tick_two = fs::read_to_string(&generated).unwrap();
        assert!(tick_two.contains("BBBB"), "tick-two module: {tick_two}");
        assert!(!tick_two.contains("AAAA"), "tick-two module: {tick_two}");
    }

    #[test]
    fn persistent_shadow_prunes_stale_generated_raw_module() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("message.txt");
        let importer = tmp.path().join("entry.ts");
        fs::write(&target, "hello").unwrap();
        fs::write(
            &importer,
            "import text from './message.txt?raw';\nexport default text;\n",
        )
        .unwrap();
        let mut session = ShadowSession::new(tmp.path()).unwrap();
        run_source_tick(&mut session, &[(&importer, "src/entry.ts")]);
        let generated = fs::read_dir(session.shadow_root().join("src"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".zfb-raw-"))
            })
            .expect("generated raw module");

        fs::write(&importer, "export default 'plain';\n").unwrap();
        run_source_tick(&mut session, &[(&importer, "src/entry.ts")]);
        assert!(
            !generated.exists(),
            "generated module must be pruned when its import disappears"
        );
    }

    #[test]
    fn production_pages_overlay_uses_logical_project_importer_for_raw() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        fs::create_dir_all(root.join("pages")).unwrap();
        fs::create_dir_all(root.join("data")).unwrap();
        let logical_page = root.join("pages/index.ts");
        fs::write(
            &logical_page,
            "import text from '../data/message.txt?raw';\nexport default text;\n",
        )
        .unwrap();
        fs::write(root.join("data/message.txt"), "overlay-logical-target").unwrap();

        let overlay = tempfile::tempdir().unwrap();
        let overlay_pages = overlay.path().join("pages");
        fs::create_dir_all(&overlay_pages).unwrap();
        fs::copy(&logical_page, overlay_pages.join("index.ts")).unwrap();
        let shadow = tempfile::tempdir().unwrap();
        let shadow_pages = shadow.path().join("pages");
        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(root, &exclude);
        let mut routes = Vec::new();
        materialise_shadow(
            &overlay_pages,
            &shadow_pages,
            &mut routes,
            &ctx,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();

        let staged = fs::read_to_string(shadow_pages.join("index.ts")).unwrap();
        assert!(!staged.contains("?raw"), "{staged}");
        assert_eq!(routes[0].source_path, PathBuf::from("pages/index.ts"));
        let edges = ctx.raw_import_edges.borrow();
        assert!(edges.iter().any(|edge| {
            edge.importer == logical_page && edge.target == root.join("data/message.txt")
        }));
    }

    #[test]
    fn materialise_source_file_expands_alias_raw_import_with_logical_edge() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        fs::create_dir_all(root.join("pages")).unwrap();
        fs::create_dir_all(root.join("data")).unwrap();
        let importer = root.join("pages/index.ts");
        let target = root.join("data/message.txt");
        fs::write(
            &importer,
            "import text from '@/data/message.txt?raw';\nexport default text;\n",
        )
        .unwrap();
        fs::write(&target, "aliased materialise raw").unwrap();
        let shadow = tempfile::tempdir().unwrap();
        let writer = ShadowWriter::new(shadow.path().to_path_buf(), None, false, None).unwrap();
        let glob_matched_files = RefCell::new(BTreeSet::new());
        let raw_edges = RefCell::new(BTreeSet::new());
        let worker_dependencies = RefCell::new(BTreeSet::new());
        let aliases = RawImportAliasContext::from_paths(&BTreeMap::from([(
            "@/*".to_string(),
            vec![root.join("*").to_string_lossy().into_owned()],
        )]));
        let staged = shadow.path().join("pages/index.ts");
        writer.ensure_dir(staged.parent().unwrap()).unwrap();

        materialise_source_file(
            &importer,
            &importer,
            &staged,
            &|_| false,
            false,
            &writer,
            &glob_matched_files,
            &raw_edges,
            &aliases,
            &worker_dependencies,
            root,
            &ModuleWorkerBuildContext::default(),
        )
        .unwrap();

        let staged_source = fs::read_to_string(&staged).unwrap();
        assert!(!staged_source.contains("?raw"), "{staged_source}");
        assert!(raw_edges
            .borrow()
            .contains(&RawImportEdge { importer, target }));
    }

    // --- issue #2080 (epic #2078): rejected mirror-derived materialisation
    // leaves shared-state residue ---
    //
    // `materialise_source_file` mutates FOUR pieces of shared state before it
    // can fail: it writes generated `.zfb-raw-*.mjs` modules to disk
    // (:7871-7882), and it extends `glob_matched_files` (:7851-7853),
    // `raw_import_edges` (:7883-7885), and `module_worker_dependencies`
    // (:7902-7904) — all BEFORE the final `writer.write_if_changed(to, ...)`
    // commit. A late failure at that final step (the only point reachable
    // after every one of those mutations has already run) leaves every
    // mutation in place with no rollback — issue #2049's non-monotonicity.
    // Fixed by issue #2085 (provenance threading), per epic #2078's pinned
    // "transactional" scope: PRE-COMMIT transformation/parse rejection only,
    // never the sequential `write_if_changed` commit's own atomicity.

    /// Shared fixture for the residue tests below: an importer that exercises
    /// all three cross-file transforms (`import.meta.glob`, terminal `?raw`,
    /// and a module-worker URL macro) in one file, plus an unrelated sibling
    /// (`victim.ts`) that ALSO uses `import.meta.glob` on its own — the file
    /// whose outcome the sharpest residue pin (the "flip" test below) checks.
    /// Returns `(project, root, importer, victim)`.
    fn write_residue_fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().to_path_buf();
        let pages = root.join("pages");
        fs::create_dir_all(pages.join("widgets-a")).unwrap();
        fs::create_dir_all(pages.join("widgets-b")).unwrap();
        fs::write(pages.join("widgets-a/a.tsx"), "export const a = 1;\n").unwrap();
        fs::write(pages.join("widgets-b/b.tsx"), "export const b = 2;\n").unwrap();
        fs::write(
            pages.join("search.worker.ts"),
            "self.postMessage('ready');\n",
        )
        .unwrap();
        let importer = pages.join("importer.ts");
        fs::write(
            &importer,
            "import text from './victim.ts?raw';\n\
             const widgets = import.meta.glob('./widgets-a/*.tsx', { eager: true });\n\
             new Worker(new URL('./search.worker.ts', import.meta.url), { type: 'module' });\n\
             export default { widgets, text };\n",
        )
        .unwrap();
        let victim = pages.join("victim.ts");
        fs::write(
            &victim,
            "export const widgets = import.meta.glob('./widgets-b/*.tsx', { eager: true });\n",
        )
        .unwrap();
        (project, root, importer, victim)
    }

    /// Orphan-detection helper: whether a `.zfb-raw-*.mjs` generated module
    /// exists directly inside `dir`. Content-hash-independent so callers
    /// don't need to reproduce the filename derivation to look for it.
    fn generated_raw_module_exists(dir: &Path) -> bool {
        let Ok(entries) = fs::read_dir(dir) else {
            return false;
        };
        entries.filter_map(|entry| entry.ok()).any(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with(".zfb-raw-") && name.ends_with(".mjs")
        })
    }

    #[test]
    #[ignore = "pending-feature: https://github.com/Takazudo/zudo-front-builder/issues/2085"]
    fn materialise_source_file_late_failure_leaves_orphan_module_and_extended_accumulators() {
        let (_project, root, importer, _victim) = write_residue_fixture();
        let shadow = tempfile::tempdir().unwrap();
        let shadow_pages = shadow.path().join("pages");
        let writer = ShadowWriter::new(shadow.path().to_path_buf(), None, true, None).unwrap();
        writer.ensure_dir(&shadow_pages).unwrap();

        let glob_matched_files = RefCell::new(BTreeSet::new());
        let raw_edges = RefCell::new(BTreeSet::new());
        let worker_dependencies = RefCell::new(BTreeSet::new());

        // Force the ONLY failure point reachable after every mutation below
        // has already run: the final `write_if_changed(to, ...)` commit. A
        // pre-existing DIRECTORY at `to` makes that specific write fail
        // while every earlier mutation stays in place.
        let staged = shadow_pages.join("importer.ts");
        fs::create_dir_all(&staged).unwrap();

        let result = materialise_source_file(
            &importer,
            &importer,
            &staged,
            &|_| false,
            true,
            &writer,
            &glob_matched_files,
            &raw_edges,
            &RawImportAliasContext::empty(),
            &worker_dependencies,
            &root,
            &ModuleWorkerBuildContext::default(),
        );
        let err = result.expect_err("a directory at the destination must reject the final commit");
        let err = format!("{err:#}");
        assert!(
            err.contains("write expanded source to"),
            "must fail at the FINAL write — after glob/raw/worker have already mutated shared \
             state, not earlier: {err}"
        );

        // Desired post-fix state: a rejected call must not leave any of
        // these three accumulators carrying its partial work.
        assert!(
            glob_matched_files.borrow().is_empty(),
            "a rejected materialisation must not leave queued glob matches behind"
        );
        assert!(
            raw_edges.borrow().is_empty(),
            "a rejected materialisation must not leave a raw_import_edges residue behind"
        );
        assert!(
            worker_dependencies.borrow().is_empty(),
            "a rejected materialisation must not leave module_worker_dependencies residue behind"
        );
        assert!(
            !generated_raw_module_exists(&shadow_pages),
            "a rejected materialisation must not leave an orphan generated .zfb-raw-*.mjs module on disk"
        );
    }

    #[test]
    #[ignore = "pending-feature: https://github.com/Takazudo/zudo-front-builder/issues/2085"]
    fn materialise_source_file_late_failure_residue_does_not_corrupt_unrelated_file_outcome() {
        let (_project, root, importer, victim) = write_residue_fixture();
        let shadow = tempfile::tempdir().unwrap();
        let shadow_pages = shadow.path().join("pages");
        let writer = ShadowWriter::new(shadow.path().to_path_buf(), None, true, None).unwrap();
        writer.ensure_dir(&shadow_pages).unwrap();

        let glob_matched_files = RefCell::new(BTreeSet::new());
        let raw_edges = RefCell::new(BTreeSet::new());
        let worker_dependencies = RefCell::new(BTreeSet::new());

        let staged_importer = shadow_pages.join("importer.ts");
        fs::create_dir_all(&staged_importer).unwrap();
        materialise_source_file(
            &importer,
            &importer,
            &staged_importer,
            &|_| false,
            true,
            &writer,
            &glob_matched_files,
            &raw_edges,
            &RawImportAliasContext::empty(),
            &worker_dependencies,
            &root,
            &ModuleWorkerBuildContext::default(),
        )
        .expect_err("importer's final commit must fail (destination is a directory)");

        // The sharpest pin: the failed call above left a `raw_import_edges`
        // entry targeting `victim.ts` (discovered via ITS OWN, unrelated
        // `?raw` import in `importer.ts`). Materialising `victim.ts` next —
        // sharing the SAME `raw_import_edges` set, exactly as one shared
        // `MaterialiseCtx` does across a real walk — must not let that
        // leftover entry flip `raw_target_matches` for it: victim.ts is
        // never itself a surviving `?raw` target of anything.
        let staged_victim = shadow_pages.join("victim.ts");
        materialise_source_file(
            &victim,
            &victim,
            &staged_victim,
            &|_| false,
            true,
            &writer,
            &glob_matched_files,
            &raw_edges,
            &RawImportAliasContext::empty(),
            &worker_dependencies,
            &root,
            &ModuleWorkerBuildContext::default(),
        )
        .unwrap();

        let victim_staged = fs::read_to_string(&staged_victim).unwrap();
        assert!(
            !victim_staged.contains("import.meta.glob("),
            "victim.ts's own import.meta.glob must still be expanded normally — an unrelated \
             rejected sibling call must not corrupt it into a terminal raw-copy; got:\n{victim_staged}"
        );
    }

    /// Control (NOT ignored): a SUCCESSFUL materialisation of the same
    /// importer fixture must still commit all four effects the residue tests
    /// above pin as wrongly-surviving-a-rejection. This is the positive case
    /// the eventual #2085 fix must not break.
    #[test]
    fn materialise_source_file_successful_call_commits_all_four_effects() {
        let (_project, root, importer, _victim) = write_residue_fixture();
        let shadow = tempfile::tempdir().unwrap();
        let shadow_pages = shadow.path().join("pages");
        let writer = ShadowWriter::new(shadow.path().to_path_buf(), None, true, None).unwrap();
        writer.ensure_dir(&shadow_pages).unwrap();

        let glob_matched_files = RefCell::new(BTreeSet::new());
        let raw_edges = RefCell::new(BTreeSet::new());
        let worker_dependencies = RefCell::new(BTreeSet::new());
        let staged = shadow_pages.join("importer.ts");

        materialise_source_file(
            &importer,
            &importer,
            &staged,
            &|_| false,
            true,
            &writer,
            &glob_matched_files,
            &raw_edges,
            &RawImportAliasContext::empty(),
            &worker_dependencies,
            &root,
            &ModuleWorkerBuildContext::default(),
        )
        .unwrap();

        assert!(
            glob_matched_files
                .borrow()
                .contains(&root.join("pages/widgets-a/a.tsx")),
            "a successful call must commit the glob match"
        );
        assert!(
            generated_raw_module_exists(&shadow_pages),
            "a successful call must commit the generated raw module to disk"
        );
        assert!(
            raw_edges
                .borrow()
                .iter()
                .any(|edge| edge.importer == importer),
            "a successful call must commit the raw_import_edges entry"
        );
        assert!(
            worker_dependencies
                .borrow()
                .iter()
                .any(|dep| dep.importer == importer),
            "a successful call must commit the module_worker_dependencies entry"
        );
        let staged_source = fs::read_to_string(&staged).unwrap();
        assert!(!staged_source.contains("?raw"), "{staged_source}");
        assert!(
            !staged_source.contains("import.meta.glob("),
            "{staged_source}"
        );
        assert!(staged_source.contains(".js?v="), "{staged_source}");
    }

    #[test]
    fn ssr_terminal_js_raw_target_is_never_reparsed_by_broad_mirror() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let components = root.join("components");
        fs::create_dir_all(&components).unwrap();
        let target = components.join("a-terminal.js");
        let raw_bytes = b"not javascript ? {{{ import.meta.glob('./also-text')\n";
        fs::write(&target, raw_bytes).unwrap();
        fs::write(
            components.join("z-importer.ts"),
            "import text from './a-terminal.js?raw';\nexport default text;\n",
        )
        .unwrap();

        let shadow = tempfile::tempdir().unwrap();
        let shadow_components = shadow.path().join("components");
        let exclude = no_bundle_exclude();
        let writer = ShadowWriter::new(shadow.path().to_path_buf(), None, true, None).unwrap();
        let ctx = MaterialiseCtx {
            pipeline_spec: zfb_content::PipelineSpec::default(),
            copy_mode: true,
            bundle_exclude: &exclude,
            project_root: root,
            writer: &writer,
            glob_matched_files: RefCell::new(BTreeSet::new()),
            raw_import_edges: RefCell::new(BTreeSet::new()),
            raw_import_aliases: RawImportAliasContext::empty(),
            module_worker_dependencies: RefCell::new(BTreeSet::new()),
            worker_build_context: ModuleWorkerBuildContext::default(),
            raw_preflight_complete: Cell::new(false),
            snapshot_specifiers: None,
        };
        materialise_shadow(
            &components,
            &shadow_components,
            &mut Vec::new(),
            &ctx,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();

        assert_eq!(
            fs::read(shadow_components.join("a-terminal.js")).unwrap(),
            raw_bytes
        );
        let importer = fs::read_to_string(shadow_components.join("z-importer.ts")).unwrap();
        assert!(!importer.contains("?raw"), "{importer}");
    }

    #[test]
    fn materialise_ts_generic_arrow_with_query_text_is_not_a_raw_error() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let components = root.join("components");
        fs::create_dir_all(&components).unwrap();
        let valid_source =
            "const o = { m: async <T>() => {} };\nconst q = '?';\nexport { o, q };\n";
        let invalid_source = "const broken = ;\n// import text from './message.txt?raw'\n";
        fs::write(components.join("generic.ts"), valid_source).unwrap();
        fs::write(components.join("unparseable.ts"), invalid_source).unwrap();

        let shadow = tempfile::tempdir().unwrap();
        let shadow_components = shadow.path().join("components");
        let exclude = no_bundle_exclude();
        let writer = ShadowWriter::new(shadow.path().to_path_buf(), None, true, None).unwrap();
        let ctx = MaterialiseCtx {
            pipeline_spec: zfb_content::PipelineSpec::default(),
            copy_mode: true,
            bundle_exclude: &exclude,
            project_root: root,
            writer: &writer,
            glob_matched_files: RefCell::new(BTreeSet::new()),
            raw_import_edges: RefCell::new(BTreeSet::new()),
            raw_import_aliases: RawImportAliasContext::empty(),
            module_worker_dependencies: RefCell::new(BTreeSet::new()),
            worker_build_context: ModuleWorkerBuildContext::default(),
            raw_preflight_complete: Cell::new(false),
            snapshot_specifiers: None,
        };

        materialise_shadow(
            &components,
            &shadow_components,
            &mut Vec::new(),
            &ctx,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(shadow_components.join("generic.ts")).unwrap(),
            valid_source
        );
        assert_eq!(
            fs::read_to_string(shadow_components.join("unparseable.ts")).unwrap(),
            invalid_source
        );
    }

    #[test]
    fn ssr_terminal_preflight_spans_all_source_roots_before_materialising() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let pages = root.join("pages");
        let components = root.join("components");
        fs::create_dir_all(&pages).unwrap();
        fs::create_dir_all(&components).unwrap();
        let raw_bytes = b"invalid ? {{{ import.meta.glob('./text-only')\n";
        fs::write(pages.join("a-terminal.js"), raw_bytes).unwrap();
        fs::write(
            components.join("importer.ts"),
            "import text from '../pages/a-terminal.js?raw';\nexport default text;\n",
        )
        .unwrap();
        let shadow = tempfile::tempdir().unwrap();
        let shadow_pages = shadow.path().join("pages");
        let shadow_components = shadow.path().join("components");
        let exclude = no_bundle_exclude();
        let writer = ShadowWriter::new(shadow.path().to_path_buf(), None, true, None).unwrap();
        let ctx = MaterialiseCtx {
            pipeline_spec: zfb_content::PipelineSpec::default(),
            copy_mode: true,
            bundle_exclude: &exclude,
            project_root: root,
            writer: &writer,
            glob_matched_files: RefCell::new(BTreeSet::new()),
            raw_import_edges: RefCell::new(BTreeSet::new()),
            raw_import_aliases: RawImportAliasContext::empty(),
            module_worker_dependencies: RefCell::new(BTreeSet::new()),
            worker_build_context: ModuleWorkerBuildContext::default(),
            raw_preflight_complete: Cell::new(false),
            snapshot_specifiers: None,
        };

        preflight_raw_tree(&pages, &shadow_pages, &ctx).unwrap();
        preflight_raw_tree(&components, &shadow_components, &ctx).unwrap();
        ctx.raw_preflight_complete.set(true);
        materialise_shadow(
            &pages,
            &shadow_pages,
            &mut Vec::new(),
            &ctx,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(
            fs::read(shadow_pages.join("a-terminal.js")).unwrap(),
            raw_bytes
        );
    }

    #[cfg(unix)]
    #[test]
    fn ssr_copy_mode_preprocesses_raw_importer_beneath_symlinked_dir() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let components = root.join("components");
        let real = components.join("real");
        fs::create_dir_all(&real).unwrap();
        fs::write(
            real.join("entry.ts"),
            "import text from './message.txt?raw';\nexport default text;\n",
        )
        .unwrap();
        fs::write(real.join("message.txt"), "symlink-directory-raw").unwrap();
        std::os::unix::fs::symlink(&real, components.join("alias")).unwrap();

        let shadow = tempfile::tempdir().unwrap();
        let shadow_components = shadow.path().join("components");
        let exclude = no_bundle_exclude();
        let writer = ShadowWriter::new(shadow.path().to_path_buf(), None, true, None).unwrap();
        let ctx = MaterialiseCtx {
            pipeline_spec: zfb_content::PipelineSpec::default(),
            copy_mode: true,
            bundle_exclude: &exclude,
            project_root: root,
            writer: &writer,
            glob_matched_files: RefCell::new(BTreeSet::new()),
            raw_import_edges: RefCell::new(BTreeSet::new()),
            raw_import_aliases: RawImportAliasContext::empty(),
            module_worker_dependencies: RefCell::new(BTreeSet::new()),
            worker_build_context: ModuleWorkerBuildContext::default(),
            raw_preflight_complete: Cell::new(false),
            snapshot_specifiers: None,
        };
        materialise_shadow(
            &components,
            &shadow_components,
            &mut Vec::new(),
            &ctx,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();

        let staged = fs::read_to_string(shadow_components.join("alias/entry.ts")).unwrap();
        assert!(!staged.contains("?raw"), "{staged}");
        assert!(fs::read_dir(shadow_components.join("alias"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .any(|path| path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".zfb-raw-") && name.ends_with(".mjs"))));
    }

    // ---- dangling symlink preflight (issue #1838) --------------------------

    /// The classifier's exact `NotFound` guard: rejects any non-`NotFound`
    /// error immediately, before it ever inspects the path. Deterministic —
    /// a missing root is always reported as `NotFound` on every platform
    /// this crate targets, no race required.
    #[test]
    fn dangling_symlink_classifier_rejects_notfound_on_nonexistent_non_symlink_path() {
        let project = tempfile::tempdir().unwrap();
        let missing = project.path().join("never-existed");
        let mut walk = WalkDir::new(&missing).into_iter();
        let err = walk
            .next()
            .expect("a walk over a missing root yields one item")
            .expect_err("a missing root must be a walk error, not a successful entry");
        assert_eq!(
            err.io_error().map(|e| e.kind()),
            Some(std::io::ErrorKind::NotFound)
        );

        // The path was never a symlink (it never existed at all), so even
        // though the io error kind matches, this must NOT be classified as
        // a dangling symlink — it must stay fatal.
        assert!(
            dangling_symlink_from_walk_error(&err).is_none(),
            "a NotFound error on a path that is not (and never was) a symlink \
             must not be classified as dangling"
        );
    }

    /// The classifier's positive existence re-check (review-mandated,
    /// issue #1838): a `NotFound` walk error is only trustworthy at the
    /// instant it happened. This deterministically (no real race needed)
    /// simulates the TOCTOU window the re-check guards: capture a genuine
    /// `NotFound` error from a dangling symlink, then make the target exist
    /// BEFORE handing that stale error to the classifier. The classifier
    /// must re-verify against the current filesystem state and refuse to
    /// call it dangling.
    #[cfg(unix)]
    #[test]
    fn dangling_symlink_classifier_rechecks_before_trusting_a_stale_notfound() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let target = root.join("target.ts");
        let link = root.join("link.ts");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let mut walk = WalkDir::new(root).follow_links(true).into_iter();
        walk.next().unwrap().unwrap(); // the root itself
        let err = walk
            .next()
            .expect("the walk yields the dangling entry")
            .expect_err("following the still-missing target must fail");
        assert_eq!(
            err.io_error().map(|e| e.kind()),
            Some(std::io::ErrorKind::NotFound)
        );

        // The target now exists — the classifier must re-check rather than
        // trust the walk's (now stale) NotFound result.
        fs::write(&target, "now exists").unwrap();

        assert!(
            dangling_symlink_from_walk_error(&err).is_none(),
            "a symlink whose target now resolves must not be classified as \
             dangling, even though the original walk error reported NotFound"
        );
    }

    /// Pins the exact text shared by both emission channels in
    /// `skip_dangling_symlink_or_fail` — including the user-visible
    /// `eprintln!("zfb warn: {msg}")` form, which cannot be asserted on
    /// directly without capturing the process's real stderr fd.
    #[test]
    fn dangling_symlink_warning_message_names_link_and_target() {
        let link = Path::new("/proj/components/dangling.ts");
        let target = Path::new("/proj/components/missing-target.ts");
        let msg = dangling_symlink_warning_message(link, target);
        assert!(msg.contains("dangling symlink"), "{msg}");
        assert!(msg.contains("/proj/components/dangling.ts"), "{msg}");
        assert!(msg.contains("/proj/components/missing-target.ts"), "{msg}");
        assert!(msg.contains("cannot be resolved"), "{msg}");
    }

    /// A dangling symlink (target deleted) under a `copy_mode` tree must not
    /// abort `preflight_raw_tree` — it used to propagate a bare `os error 2`
    /// with no mention of a symlink. Also pins the warning text names both
    /// the link and its (missing) target.
    #[cfg(unix)]
    #[test]
    #[tracing_test::traced_test]
    fn preflight_raw_tree_skips_dangling_symlink_with_warning() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let components = root.join("components");
        fs::create_dir_all(&components).unwrap();
        fs::write(components.join("real.ts"), "export default 1;\n").unwrap();
        let target = components.join("missing-target.ts");
        let link = components.join("dangling.ts");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let shadow = tempfile::tempdir().unwrap();
        let shadow_components = shadow.path().join("components");
        let exclude = no_bundle_exclude();
        let writer = ShadowWriter::new(shadow.path().to_path_buf(), None, true, None).unwrap();
        let ctx = MaterialiseCtx {
            pipeline_spec: zfb_content::PipelineSpec::default(),
            copy_mode: true,
            bundle_exclude: &exclude,
            project_root: root,
            writer: &writer,
            glob_matched_files: RefCell::new(BTreeSet::new()),
            raw_import_edges: RefCell::new(BTreeSet::new()),
            raw_import_aliases: RawImportAliasContext::empty(),
            module_worker_dependencies: RefCell::new(BTreeSet::new()),
            worker_build_context: ModuleWorkerBuildContext::default(),
            raw_preflight_complete: Cell::new(false),
            snapshot_specifiers: None,
        };

        preflight_raw_tree(&components, &shadow_components, &ctx)
            .expect("a dangling symlink must be skipped, not fatal");

        assert!(
            logs_contain("dangling symlink"),
            "expected a warning naming the dangling symlink"
        );
        assert!(
            logs_contain(&link.display().to_string()),
            "expected the warning to name the link path"
        );
        assert!(
            logs_contain(&target.display().to_string()),
            "expected the warning to name the missing target"
        );
    }

    /// `materialise_symlinked_dir` gets the same classifier via
    /// `skip_dangling_symlink_or_fail` as `preflight_raw_tree` — the two were
    /// hand-copied `match` blocks before the shared helper existed, so a
    /// refactor could silently revert either without either's own test
    /// catching it. Direct coverage per call site (review-mandated).
    #[cfg(unix)]
    #[test]
    fn materialise_symlinked_dir_skips_dangling_symlink_with_warning() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let symlinked_dir = root.join("aliased");
        fs::create_dir_all(&symlinked_dir).unwrap();
        fs::write(symlinked_dir.join("real.ts"), "export default 1;\n").unwrap();
        let target = symlinked_dir.join("missing-target.ts");
        let link = symlinked_dir.join("dangling.ts");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let shadow = tempfile::tempdir().unwrap();
        let dest = shadow.path().join("aliased");
        let exclude = no_bundle_exclude();
        let writer = ShadowWriter::new(shadow.path().to_path_buf(), None, true, None).unwrap();
        let ctx = MaterialiseCtx {
            pipeline_spec: zfb_content::PipelineSpec::default(),
            copy_mode: true,
            bundle_exclude: &exclude,
            project_root: root,
            writer: &writer,
            glob_matched_files: RefCell::new(BTreeSet::new()),
            raw_import_edges: RefCell::new(BTreeSet::new()),
            raw_import_aliases: RawImportAliasContext::empty(),
            module_worker_dependencies: RefCell::new(BTreeSet::new()),
            worker_build_context: ModuleWorkerBuildContext::default(),
            raw_preflight_complete: Cell::new(false),
            snapshot_specifiers: None,
        };
        let is_excluded = |_: &Path| false;

        materialise_symlinked_dir(&symlinked_dir, &dest, &ctx, &is_excluded)
            .expect("a dangling symlink inside a symlinked source dir must be skipped, not fatal");

        assert!(
            dest.join("real.ts").is_file(),
            "the real sibling file must still be staged"
        );
        assert!(
            !dest.join("dangling.ts").exists(),
            "the dangling symlink must not be staged"
        );
    }

    /// `materialise_isolated_exact_dir` gets the same classifier — direct
    /// coverage per call site (review-mandated), see the doc comment on
    /// `materialise_symlinked_dir_skips_dangling_symlink_with_warning` above.
    #[cfg(unix)]
    #[test]
    fn materialise_isolated_exact_dir_skips_dangling_symlink_with_warning() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let source_root = root.join("vendored-pkg");
        fs::create_dir_all(&source_root).unwrap();
        fs::write(source_root.join("real.ts"), "export default 1;\n").unwrap();
        let target = source_root.join("missing-target.ts");
        let link = source_root.join("dangling.ts");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let shadow = tempfile::tempdir().unwrap();
        let dest = shadow.path().join("vendored-pkg");
        let writer = ShadowWriter::new(shadow.path().to_path_buf(), None, true, None).unwrap();
        let is_excluded = |_: &Path| false;

        materialise_isolated_exact_dir(
            &source_root,
            &source_root,
            &dest,
            &writer,
            &is_excluded,
            false,
        )
        .expect(
            "a dangling symlink inside an isolated exact-target dir must be \
                 skipped, not fatal",
        );

        assert!(
            dest.join("real.ts").is_file(),
            "the real sibling file must still be staged"
        );
        assert!(
            !dest.join("dangling.ts").exists(),
            "the dangling symlink must not be staged"
        );
    }

    #[test]
    fn materialise_workspace_package_is_real_bounded_copy_on_every_platform() {
        let source = tempfile::tempdir().unwrap();
        let source_root = source.path().join("ui");
        fs::create_dir_all(source_root.join("src/assets")).unwrap();
        fs::create_dir_all(source_root.join("dist")).unwrap();
        fs::create_dir_all(source_root.join("node_modules/vendor")).unwrap();
        fs::write(source_root.join("package.json"), r#"{"name":"@acme/ui"}"#).unwrap();
        fs::write(source_root.join("src/index.ts"), "export default 1;\n").unwrap();
        fs::write(source_root.join("src/assets/icon.svg"), "<svg/>\n").unwrap();
        fs::write(source_root.join("dist/leak.js"), "generated\n").unwrap();
        fs::write(
            source_root.join("node_modules/vendor/index.js"),
            "vendored\n",
        )
        .unwrap();

        let shadow = tempfile::tempdir().unwrap();
        let dest = shadow.path().join("node_modules/@acme/ui");
        let writer = ShadowWriter::new(shadow.path().to_path_buf(), None, true, None).unwrap();
        materialise_isolated_exact_dir(
            &source_root,
            Path::new("node_modules/@acme/ui"),
            &dest,
            &writer,
            &|_| false,
            true,
        )
        .unwrap();

        assert!(dest.is_dir());
        assert!(dest.join("package.json").is_file());
        assert!(dest.join("src/index.ts").is_file());
        assert!(dest.join("src/assets/icon.svg").is_file());
        assert!(!dest.join("dist").exists());
        assert!(!dest.join("node_modules").exists());
        assert!(
            !fs::symlink_metadata(&dest)
                .unwrap()
                .file_type()
                .is_symlink(),
            "copy-mode package staging must not require symlink privileges"
        );
    }

    /// A genuine non-`NotFound` walk error (an unreadable directory) must
    /// still propagate as fatal — the dangling-symlink classifier must not
    /// swallow errors it wasn't built for. Some environments (root, certain
    /// sandboxes) bypass directory permission checks entirely, so this test
    /// only asserts when the walk actually hits a non-NotFound error (same
    /// skip-rather-than-false-pass guard used by
    /// `stale_sweep_keeps_session_when_lock_open_fails_non_notfound` in
    /// `crates/zfb/src/commands/watcher_liveness_probe.rs`). Perms are
    /// restored BEFORE the assertion (not after) so a failing assertion
    /// can't leave a chmod-000 directory behind for the tempdir's `Drop` to
    /// choke on.
    #[cfg(unix)]
    #[test]
    fn preflight_raw_tree_propagates_non_dangling_walk_errors() {
        use std::os::unix::fs::PermissionsExt;

        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let components = root.join("components");
        let locked = components.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::write(locked.join("entry.ts"), "export default 1;\n").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let enforced = fs::read_dir(&locked).is_err();

        let result = enforced.then(|| {
            let shadow = tempfile::tempdir().unwrap();
            let shadow_components = shadow.path().join("components");
            let exclude = no_bundle_exclude();
            let writer = ShadowWriter::new(shadow.path().to_path_buf(), None, true, None).unwrap();
            let ctx = MaterialiseCtx {
                pipeline_spec: zfb_content::PipelineSpec::default(),
                copy_mode: true,
                bundle_exclude: &exclude,
                project_root: root,
                writer: &writer,
                glob_matched_files: RefCell::new(BTreeSet::new()),
                raw_import_edges: RefCell::new(BTreeSet::new()),
                raw_import_aliases: RawImportAliasContext::empty(),
                module_worker_dependencies: RefCell::new(BTreeSet::new()),
                worker_build_context: ModuleWorkerBuildContext::default(),
                raw_preflight_complete: Cell::new(false),
                snapshot_specifiers: None,
            };
            preflight_raw_tree(&components, &shadow_components, &ctx)
        });

        // Restore perms BEFORE asserting, so a panicking assertion can never
        // skip this and leave a 0o000 directory behind.
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        match result {
            Some(result) => assert!(
                result.is_err(),
                "a genuine permission-denied walk error must still be fatal, not silently skipped"
            ),
            None => eprintln!(
                "[preflight_raw_tree_propagates_non_dangling_walk_errors] directory \
                 permissions not enforced (running as root?); skipping assertion."
            ),
        }
    }

    #[test]
    fn raw_target_excluded_by_bundle_exclude_is_a_named_error() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("components")).unwrap();
        let target = tmp.path().join("components/secret.txt");
        let importer = tmp.path().join("components/entry.ts");
        fs::write(&target, "secret").unwrap();
        fs::write(&importer, "import text from './secret.txt?raw';\n").unwrap();
        let shadow = tempfile::tempdir().unwrap();
        let writer = ShadowWriter::new(shadow.path().to_path_buf(), None, false, None).unwrap();
        let glob_matched_files = RefCell::new(BTreeSet::new());
        let edges = RefCell::new(BTreeSet::new());
        let worker_dependencies = RefCell::new(BTreeSet::new());
        let matcher = BundleExcludeMatcher::new(&["components/*.txt".to_string()]).unwrap();
        let error = materialise_source_file(
            &importer,
            &importer,
            &shadow.path().join("entry.ts"),
            &|path| matcher.is_excluded(path, tmp.path()),
            false,
            &writer,
            &glob_matched_files,
            &edges,
            &RawImportAliasContext::empty(),
            &worker_dependencies,
            tmp.path(),
            &ModuleWorkerBuildContext::default(),
        )
        .unwrap_err();
        let error = format!("{error:#}");
        assert!(error.contains("bundle.exclude"), "{error}");
        assert!(error.contains("secret.txt"), "{error}");
    }

    #[test]
    fn route_module_deps_include_original_raw_target_not_generated_wrapper() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let page = root.join("pages/index.tsx");
        let importer = root.join("components/shader.ts");
        let target = root.join("components/noise.frag");
        fs::create_dir_all(page.parent().unwrap()).unwrap();
        fs::create_dir_all(importer.parent().unwrap()).unwrap();
        fs::write(&page, "page").unwrap();
        fs::write(&importer, "import raw").unwrap();
        fs::write(&target, "noise").unwrap();
        let importer_real = importer.canonicalize().unwrap();
        let target_real = target.canonicalize().unwrap();
        let mut deps = vec![crate::metafile_deps::RouteModuleDeps {
            source_path: PathBuf::from("pages/index.tsx"),
            module_deps: BTreeSet::from([importer_real.clone()]),
        }];
        let edges = BTreeSet::from([RawImportEdge { importer, target }]);
        augment_route_deps_with_raw_targets(&mut deps, &edges, root);
        assert!(deps[0].module_deps.contains(&target_real));
        assert!(deps[0].module_deps.contains(&importer_real));
    }

    #[test]
    fn route_module_deps_include_full_browser_only_worker_closure() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let page = root.join("pages/index.tsx");
        let importer = root.join("components/Island.tsx");
        let worker = root.join("components/search.worker.ts");
        let helper = root.join("components/search-helper.ts");
        let required = root.join("components/required.ts");
        let css = root.join("components/search.css");
        let tokens = root.join("components/tokens.css");
        let icon = root.join("components/icon.bin");
        let payload = root.join("components/payload.txt");
        for path in [
            &page, &importer, &worker, &helper, &required, &css, &tokens, &icon, &payload,
        ] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
        }
        fs::write(&page, "page").unwrap();
        let importer_source =
            "new Worker(new URL('./search.worker.ts', import.meta.url), { type: 'module' });";
        fs::write(&importer, importer_source).unwrap();
        fs::write(
            root.join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["components/*"]}}}"#,
        )
        .unwrap();
        fs::write(
            &worker,
            "import { helper } from '@/search-helper'; import required = require('./required'); import './search.css'; import payload from './payload.txt?raw'; self.postMessage([helper, required, payload]);",
        )
        .unwrap();
        fs::write(&helper, "export const helper = 1;").unwrap();
        fs::write(&required, "module.exports = 2;").unwrap();
        fs::write(
            &css,
            "@import './tokens.css'; .worker { background: url('./icon.bin?v=1#icon'); }",
        )
        .unwrap();
        fs::write(&tokens, ":root { --worker: red; }").unwrap();
        fs::write(&icon, [1_u8, 2, 3]).unwrap();
        fs::write(&payload, "worker payload").unwrap();
        let importer_real = importer.canonicalize().unwrap();
        let mut deps = vec![crate::metafile_deps::RouteModuleDeps {
            source_path: PathBuf::from("pages/index.tsx"),
            module_deps: BTreeSet::from([importer_real.clone()]),
        }];
        let rewrite =
            crate::module_worker::rewrite_module_worker_urls(importer_source, &importer, root)
                .unwrap();
        let worker_dependencies: BTreeSet<_> = rewrite
            .dependencies
            .into_iter()
            .chain(rewrite.config_dependencies)
            .collect();
        augment_route_deps_with_worker_targets(&mut deps, &worker_dependencies, root);
        assert!(deps[0].module_deps.contains(&importer_real));
        for dependency in [
            &worker,
            &helper,
            &required,
            &css,
            &tokens,
            &icon,
            &payload,
            &root.join("tsconfig.json"),
        ] {
            assert!(
                deps[0]
                    .module_deps
                    .contains(&dependency.canonicalize().unwrap()),
                "route closure omitted {}: {:?}",
                dependency.display(),
                deps[0].module_deps
            );
        }
    }

    /// Run ONE tick of `mirror_sibling_root` against `session`, mirroring
    /// the writer lifecycle `run_source_tick` uses for
    /// `materialise_source_file` (issue #1777). `first_party_root` doubles
    /// as `session`'s work root's ancestor; `shadow`/`work` are both the
    /// session's persistent shadow root — the project mirror is unused by
    /// these tests, only the sibling-workspace-relative slot
    /// (`shadow_path_for_project_path`'s first-party tier) is exercised.
    fn run_mirror_tick(
        session: &mut ShadowSession,
        mirror_root: &Path,
        project_root: &Path,
        first_party_root: &Path,
    ) {
        let work = session.shadow_root().to_path_buf();
        let fingerprint = spec_fingerprint(&zfb_content::PipelineSpec::default());
        let writer = ShadowWriter::new(work.clone(), Some(session), false, fingerprint)
            .expect("session writer construction");
        let bundle_exclude = BundleExcludeMatcher::new(&[]).expect("empty exclude matcher");
        mirror_sibling_root(
            mirror_root,
            project_root,
            first_party_root,
            &work,
            &work,
            None,
            &writer,
            &bundle_exclude,
        )
        .expect("mirror_sibling_root tick");
        writer.prune_stale().expect("prune");
        writer.mark_clean();
    }

    /// Like `run_mirror_tick`, but ALSO runs `materialise_source_file` on
    /// `preprocess_from` -> `preprocess_dest` immediately after the mirror
    /// pass, in the same tick — mirroring the real `bundle_with_session`
    /// ordering (2a-sibling mirror, THEN the preprocessing-materialise
    /// pass) so a same-tick mirror-skip entry can be proven never to
    /// satisfy the LATER preprocessing lookup. Returns the dest's final
    /// contents.
    fn run_mirror_then_materialise_tick(
        session: &mut ShadowSession,
        mirror_root: &Path,
        project_root: &Path,
        first_party_root: &Path,
        preprocess_from: &Path,
        preprocess_dest: &Path,
    ) -> String {
        let work = session.shadow_root().to_path_buf();
        let fingerprint = spec_fingerprint(&zfb_content::PipelineSpec::default());
        let writer = ShadowWriter::new(work.clone(), Some(session), false, fingerprint)
            .expect("session writer construction");
        let bundle_exclude = BundleExcludeMatcher::new(&[]).expect("empty exclude matcher");
        mirror_sibling_root(
            mirror_root,
            project_root,
            first_party_root,
            &work,
            &work,
            None,
            &writer,
            &bundle_exclude,
        )
        .expect("mirror_sibling_root tick");

        let glob_matched_files = RefCell::new(BTreeSet::new());
        let raw_import_edges = RefCell::new(BTreeSet::new());
        let module_worker_dependencies = RefCell::new(BTreeSet::new());
        materialise_source_file(
            preprocess_from,
            preprocess_from,
            preprocess_dest,
            &|_| false,
            false,
            &writer,
            &glob_matched_files,
            &raw_import_edges,
            &RawImportAliasContext::empty(),
            &module_worker_dependencies,
            preprocess_from.parent().unwrap_or(preprocess_from),
            &ModuleWorkerBuildContext::default(),
        )
        .expect("materialise_source_file tick");

        writer.prune_stale().expect("prune");
        writer.mark_clean();
        fs::read_to_string(preprocess_dest).expect("read materialised dest")
    }

    #[test]
    fn mirror_skip_unchanged_css_module_is_skipped_on_second_tick() {
        // A sibling `.module.css` that `mirror_sibling_root` wholesale-copies
        // must be SKIPPED on tick 2 when unchanged (issue #1777) — zero
        // writes, proven white-box exactly like
        // `source_skip_plain_file_is_skipped_on_second_tick`: corrupt the
        // dest + drop its `written` hash; a full re-copy would restore the
        // real bytes, a skip leaves the corruption untouched.
        let tmp = tempfile::tempdir().unwrap();
        let first_party_root = tmp.path();
        let project_root = first_party_root.join("app");
        let mirror_root = first_party_root.join("pkg");
        fs::create_dir_all(&mirror_root).unwrap();
        fs::write(
            mirror_root.join("button.module.css"),
            ".btn { color: red; }\n",
        )
        .unwrap();

        let mut session = ShadowSession::new(first_party_root).unwrap();
        run_mirror_tick(&mut session, &mirror_root, &project_root, first_party_root);

        let dest_rel = PathBuf::from("pkg/button.module.css");
        let dest = session.shadow_root().join(&dest_rel);
        assert!(
            session.mirror_skip.contains_key(&dest_rel),
            "a mirrored file must be entered into the mirror_skip cache"
        );
        assert!(
            !session.source_skip.contains_key(&dest_rel),
            "mirror_sibling_root must NEVER store into the shared source_skip \
             namespace (issue #1777) — the two caches must stay separate"
        );

        // White-box: corrupt the dest + drop its `written` hash.
        fs::remove_file(&dest).ok();
        fs::write(&dest, b"__CORRUPT_NOT_THE_SOURCE__").unwrap();
        session.written.remove(&dest_rel);

        run_mirror_tick(&mut session, &mirror_root, &project_root, first_party_root);
        assert_eq!(
            fs::read(&dest).unwrap(),
            b"__CORRUPT_NOT_THE_SOURCE__",
            "an unchanged mirrored file must be SKIPPED on tick 2 — zero writes"
        );
    }

    #[test]
    fn mirror_skip_cache_never_satisfies_a_preprocessing_lookup() {
        // Regression for issue #1777: a sibling source that needs
        // `import.meta.glob` preprocessing must still be rewritten every
        // applicable tick, EVEN THOUGH the mirror pass wholesale-copies its
        // RAW (unexpanded) bytes into the same dest just beforehand, and
        // even after that dest has a matching mirror_skip entry from a
        // PRIOR tick. If the mirror fast path stored into (or was
        // consulted from) the same namespace `materialise_source_file`
        // reads, the second-tick preprocessing pass would wrongly skip and
        // leave the literal macro text in place.
        let tmp = tempfile::tempdir().unwrap();
        let first_party_root = tmp.path();
        let project_root = first_party_root.join("app");
        let mirror_root = first_party_root.join("pkg");
        fs::create_dir_all(mirror_root.join("widgets")).unwrap();
        fs::write(mirror_root.join("widgets/a.tsx"), "export const a = 1;\n").unwrap();
        let barrel = mirror_root.join("barrel.ts");
        fs::write(
            &barrel,
            "const m = import.meta.glob('./widgets/*.tsx', { eager: true });\nexport default m;\n",
        )
        .unwrap();

        let mut session = ShadowSession::new(first_party_root).unwrap();
        let dest_rel = PathBuf::from("pkg/barrel.ts");

        // Tick 1: mirror raw-copies barrel.ts, then the same-tick
        // preprocessing pass immediately overwrites it with the expanded
        // form.
        let dest = session.shadow_root().join(&dest_rel);
        let first = run_mirror_then_materialise_tick(
            &mut session,
            &mirror_root,
            &project_root,
            first_party_root,
            &barrel,
            &dest,
        );
        assert!(
            !first.contains("import.meta.glob(") && first.contains("__glob_0"),
            "tick 1 must expand the glob after the mirror raw-copy; got:\n{first}"
        );
        assert!(
            !session.mirror_skip.contains_key(&dest_rel),
            "the same-tick preprocessing overwrite must EVICT the mirror_skip \
             entry the raw-copy step just stored — otherwise a later tick could \
             skip a stale mirror re-copy over this dest's expanded content \
             (issue #1777 codex-review finding)"
        );
        assert!(
            session
                .source_skip
                .get(&dest_rel)
                .is_some_and(|entry| entry.has_glob),
            "the preprocessing pass must cache its own has_glob=true entry"
        );

        // Tick 2: barrel.ts is byte-unchanged, so the mirror pass's OWN
        // copy is skippable — but the preprocessing pass must still
        // re-expand (its `has_glob` gate refuses ANY skip).
        let second = run_mirror_then_materialise_tick(
            &mut session,
            &mirror_root,
            &project_root,
            first_party_root,
            &barrel,
            &dest,
        );
        assert!(
            !second.contains("import.meta.glob(") && second.contains("__glob_0"),
            "tick 2 must STILL expand the glob, not reuse the mirror's raw copy; got:\n{second}"
        );
    }

    #[test]
    fn mirror_skip_does_not_preserve_stale_preprocessing_output_once_unreachable() {
        // Regression for a codex-review finding on issue #1777: a sibling
        // file that WAS preprocessed (glob-expanded) in an earlier tick,
        // then falls OUT of the reachable/preprocessing set on a later
        // tick (nothing calls `materialise_source_file` for it anymore)
        // while its own bytes stay byte-unchanged, must NOT have its stale
        // expanded dest preserved by the mirror pass's skip check — a
        // fresh build would raw-copy it (mirror_sibling_root never expands
        // anything), so the incremental path must converge to the same
        // raw bytes, not freeze on whatever the last preprocessing pass
        // wrote there.
        let tmp = tempfile::tempdir().unwrap();
        let first_party_root = tmp.path();
        let project_root = first_party_root.join("app");
        let mirror_root = first_party_root.join("pkg");
        fs::create_dir_all(mirror_root.join("widgets")).unwrap();
        fs::write(mirror_root.join("widgets/a.tsx"), "export const a = 1;\n").unwrap();
        let barrel = mirror_root.join("barrel.ts");
        fs::write(
            &barrel,
            "const m = import.meta.glob('./widgets/*.tsx', { eager: true });\nexport default m;\n",
        )
        .unwrap();

        let mut session = ShadowSession::new(first_party_root).unwrap();
        let dest_rel = PathBuf::from("pkg/barrel.ts");
        let dest = session.shadow_root().join(&dest_rel);

        // Tick 1: barrel.ts is reachable — mirror raw-copies, then the
        // preprocessing pass expands it in place.
        let tick1 = run_mirror_then_materialise_tick(
            &mut session,
            &mirror_root,
            &project_root,
            first_party_root,
            &barrel,
            &dest,
        );
        assert!(
            tick1.contains("__glob_0"),
            "tick 1 must hold the expanded form; got:\n{tick1}"
        );

        // Tick 2: barrel.ts's bytes are UNCHANGED, but it is now
        // UNREACHABLE — nothing calls `materialise_source_file` for it
        // this tick (only the mirror pass runs, exactly like a real
        // `bundle_with_session` tick where discovery no longer reaches
        // this file). Without the eviction fix this would find a stale
        // `mirror_skip` hit and leave the tick-1 expanded bytes in place;
        // with the fix, tick 1's preprocessing overwrite already evicted
        // the entry, so this falls through to a full raw re-copy.
        run_mirror_tick(&mut session, &mirror_root, &project_root, first_party_root);
        let tick2 = fs::read_to_string(&dest).unwrap();
        assert!(
            tick2.contains("import.meta.glob(") && !tick2.contains("__glob_0"),
            "tick 2 (unreachable, mirror-only) must restore the RAW mirror \
             copy, not preserve tick 1's stale expanded output; got:\n{tick2}"
        );
        assert_eq!(
            tick2,
            fs::read_to_string(&barrel).unwrap(),
            "the raw re-copy must be byte-identical to the live source"
        );

        // Tick 3: still unreachable, still byte-unchanged — the mirror
        // pass's OWN entry from tick 2 is now valid, so THIS tick is a
        // genuine skip (white-box: corrupt the dest + drop its `written`
        // hash first; a full re-copy would restore the real bytes).
        fs::remove_file(&dest).ok();
        fs::write(&dest, b"__CORRUPT_NOT_THE_SOURCE__").unwrap();
        session.written.remove(&dest_rel);
        run_mirror_tick(&mut session, &mirror_root, &project_root, first_party_root);
        assert_eq!(
            fs::read(&dest).unwrap(),
            b"__CORRUPT_NOT_THE_SOURCE__",
            "tick 3 must be a genuine skip once the mirror's own raw-copy \
             entry is fresh again"
        );
    }

    #[test]
    fn mirror_skip_changed_sibling_source_still_propagates() {
        // A sibling source whose bytes change must take the full mirror
        // copy path again — the dest reflects the new content. Uses a
        // SIZE-changing edit (not just an mtime bump) so the skip-key miss
        // is deterministic on coarse-mtime filesystems too, matching
        // `source_skip_changed_plain_file_is_rematerialised`'s idiom.
        let tmp = tempfile::tempdir().unwrap();
        let first_party_root = tmp.path();
        let project_root = first_party_root.join("app");
        let mirror_root = first_party_root.join("pkg");
        fs::create_dir_all(&mirror_root).unwrap();
        let src = mirror_root.join("shared.ts");
        fs::write(&src, "export const v = 1;\n").unwrap();

        let mut session = ShadowSession::new(first_party_root).unwrap();
        run_mirror_tick(&mut session, &mirror_root, &project_root, first_party_root);
        let dest_rel = PathBuf::from("pkg/shared.ts");
        let dest = session.shadow_root().join(&dest_rel);
        assert_eq!(fs::read_to_string(&dest).unwrap(), "export const v = 1;\n");

        fs::write(&src, "export const v = 1;\nexport const w = 2;\n").unwrap();
        run_mirror_tick(&mut session, &mirror_root, &project_root, first_party_root);
        assert_eq!(
            fs::read_to_string(&dest).unwrap(),
            "export const v = 1;\nexport const w = 2;\n",
            "a changed sibling source must still propagate through the mirror pass"
        );
    }

    #[test]
    fn mirror_skip_final_bytes_match_full_copy_pipeline() {
        // Correctness-neutrality: a session that hits the mirror skip fast
        // path on tick 2 must emit BYTE-IDENTICAL output to a session
        // forced through the full copy path every tick (mirror_skip
        // cleared between ticks) — proving the fast path changes nothing
        // observable.
        let tmp = tempfile::tempdir().unwrap();
        let first_party_root = tmp.path();
        let project_root = first_party_root.join("app");
        let mirror_root = first_party_root.join("pkg");
        fs::create_dir_all(&mirror_root).unwrap();
        fs::write(mirror_root.join("data.json"), b"{\"a\":1}").unwrap();
        let dest_rel = PathBuf::from("pkg/data.json");

        let mut skipping_session = ShadowSession::new(first_party_root).unwrap();
        run_mirror_tick(
            &mut skipping_session,
            &mirror_root,
            &project_root,
            first_party_root,
        );
        run_mirror_tick(
            &mut skipping_session,
            &mirror_root,
            &project_root,
            first_party_root,
        );
        let skip_bytes = fs::read(skipping_session.shadow_root().join(&dest_rel)).unwrap();

        let mut forced_session = ShadowSession::new(first_party_root).unwrap();
        run_mirror_tick(
            &mut forced_session,
            &mirror_root,
            &project_root,
            first_party_root,
        );
        // Force tick 2 through the full copy path by evicting the mirror
        // skip cache — the ONLY difference from the skipping session above.
        forced_session.mirror_skip.clear();
        run_mirror_tick(
            &mut forced_session,
            &mirror_root,
            &project_root,
            first_party_root,
        );
        let forced_bytes = fs::read(forced_session.shadow_root().join(&dest_rel)).unwrap();

        assert_eq!(
            skip_bytes, forced_bytes,
            "the mirror skip fast path must be byte-identical to the full copy path"
        );
        assert_eq!(skip_bytes, fs::read(mirror_root.join("data.json")).unwrap());
    }

    #[test]
    fn mdx_components_raw_target_is_a_dependency_of_every_route() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let importer = root.join(MDX_COMPONENTS_FILENAME);
        let target = root.join("theme.txt");
        fs::write(&importer, "mdx components").unwrap();
        fs::write(&target, "theme").unwrap();
        let mut deps = vec![
            crate::metafile_deps::RouteModuleDeps {
                source_path: PathBuf::from("pages/a.tsx"),
                module_deps: BTreeSet::new(),
            },
            crate::metafile_deps::RouteModuleDeps {
                source_path: PathBuf::from("pages/b.tsx"),
                module_deps: BTreeSet::new(),
            },
        ];
        let edges = BTreeSet::from([RawImportEdge {
            importer,
            target: target.clone(),
        }]);
        augment_route_deps_with_raw_targets(&mut deps, &edges, root);
        let target = target.canonicalize().unwrap();
        assert!(deps.iter().all(|route| route.module_deps.contains(&target)));
    }

    #[cfg(unix)]
    #[test]
    fn route_raw_deps_keep_symlink_alias_and_canonical_target() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let page = root.join("pages/index.tsx");
        let target = root.join("data/actual.txt");
        let alias = root.join("data/current.txt");
        fs::create_dir_all(page.parent().unwrap()).unwrap();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&page, "page").unwrap();
        fs::write(&target, "actual").unwrap();
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        let mut deps = vec![crate::metafile_deps::RouteModuleDeps {
            source_path: PathBuf::from("pages/index.tsx"),
            module_deps: BTreeSet::new(),
        }];
        let edges = BTreeSet::from([RawImportEdge {
            importer: page,
            target: alias.clone(),
        }]);
        augment_route_deps_with_raw_targets(&mut deps, &edges, root);
        assert!(deps[0]
            .module_deps
            .contains(&normalize_path_lexical(&alias)));
        assert!(deps[0]
            .module_deps
            .contains(&target.canonicalize().unwrap()));
    }

    #[test]
    fn source_skip_changed_plain_file_is_rematerialised() {
        // A plain file whose bytes (size) change must take the full path on
        // tick 2 — the dest reflects the new content.
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("util.ts");
        fs::write(&from, "export const x = 1;\n").unwrap();
        let dest_rel = "src/util.ts";

        let mut session = ShadowSession::new(tmp.path()).unwrap();
        run_source_tick(&mut session, &[(&from, dest_rel)]);

        // White-box: corrupt the dest + drop its written hash so a full
        // re-materialise is observable (re-symlink replaces the file).
        let dest = session.shadow_root().join(dest_rel);
        fs::remove_file(&dest).ok();
        fs::write(&dest, b"__CORRUPT__").unwrap();
        session.written.remove(&PathBuf::from(dest_rel));

        // Change the source (different length ⇒ size differs ⇒ skip miss).
        fs::write(&from, "export const x = 1;\nexport const y = 2;\n").unwrap();
        run_source_tick(&mut session, &[(&from, dest_rel)]);

        // The dest is now a symlink to the (new) source again, NOT the
        // corruption — so reading it follows the link to the source body.
        let resolved = fs::read_to_string(&dest).unwrap();
        assert!(
            resolved.contains("export const y = 2;"),
            "a changed plain file must be RE-MATERIALISED; got:\n{resolved}"
        );
    }

    #[test]
    fn source_skip_binary_asset_is_skipped_on_second_tick() {
        // A binary/asset file (non-UTF-8, non-JS) takes the copy/symlink
        // path with has_glob=false (no UTF-8 pre-read), so it is skipped on
        // tick 2 exactly like a plain text file.
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("logo.png");
        // Invalid UTF-8 bytes → `fs::read_to_string` would fail; this path
        // never reaches the glob pre-read (extension is not JS-like anyway).
        fs::write(&from, [0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10]).unwrap();
        let dest_rel = "static/logo.png";

        let mut session = ShadowSession::new(tmp.path()).unwrap();
        run_source_tick(&mut session, &[(&from, dest_rel)]);
        let key = PathBuf::from(dest_rel);
        let entry = session
            .source_skip
            .get(&key)
            .expect("binary asset must be skip-cached");
        assert!(
            !entry.has_glob,
            "a binary asset's entry must record has_glob=false"
        );

        // White-box: corrupt the dest + drop written hash.
        let dest = session.shadow_root().join(dest_rel);
        fs::remove_file(&dest).ok();
        fs::write(&dest, b"__CORRUPT__").unwrap();
        session.written.remove(&key);

        run_source_tick(&mut session, &[(&from, dest_rel)]);
        assert_eq!(
            fs::read(&dest).unwrap(),
            b"__CORRUPT__",
            "an unchanged binary asset must be SKIPPED — dest not re-materialised"
        );
    }

    #[test]
    fn jsx_breakage_heuristic_flags_bare_backslash_expressions() {
        // Positive cases — these are exactly the patterns the MDX
        // emitter produces from un-escaped LaTeX (`$$\int_{-\infty}…$$`).
        assert!(jsx_likely_breaks_downstream_parser(
            r"<_components.p>{-\infty}{\infty}</_components.p>"
        ));
        assert!(jsx_likely_breaks_downstream_parser(
            r"const x = {\foo}; // no quote"
        ));

        // Negative cases — these all parse cleanly under esbuild's
        // JSX pass and must NOT be flagged.
        // 1. `\` inside a JSX-expression string literal.
        assert!(!jsx_likely_breaks_downstream_parser(r#"{"\\infty"}"#));
        assert!(!jsx_likely_breaks_downstream_parser(r#"{"$$\n\\int"}"#));
        // 2. Curly + escape sequence INSIDE a multi-line string
        //    literal — emitter output for fenced code blocks
        //    contains `"...{\n  site:..."` (literal `{` followed
        //    by an `\n` newline escape, all inside the string).
        assert!(!jsx_likely_breaks_downstream_parser(
            r#"{"export default {\n  site: \"x\"\n};"}"#
        ));
        // 3. Template literal carrying the same shape.
        assert!(!jsx_likely_breaks_downstream_parser(
            "`export default {\\n  site: \\\"x\\\"};`"
        ));
        // 4. Plain JS expressions.
        assert!(!jsx_likely_breaks_downstream_parser(r#"{ a + b }"#));
        assert!(!jsx_likely_breaks_downstream_parser(r#"{props.children}"#));
        assert!(!jsx_likely_breaks_downstream_parser(r#"export default x;"#));
        // 5. Comments may carry arbitrary text including `{\foo`.
        assert!(!jsx_likely_breaks_downstream_parser(
            r"// {\infty} explained inline"
        ));
        assert!(!jsx_likely_breaks_downstream_parser(
            r"/* {\infty} block-commented */"
        ));
        // 6. Issue #206 shapes — inline-code values containing
        //    HTML-tag-like text and curly-brace patterns. The MDX
        //    emitter wraps each value in `js_string_literal_in_braces`
        //    (see `crates/zfb-content/src/mdx_jsx_emit.rs`), producing
        //    `{"…escaped…"}` shapes. The `{` is followed by `"` —
        //    neither `-` nor `\\` — so the heuristic must skip.
        assert!(!jsx_likely_breaks_downstream_parser(
            r#"<_components.code>{"<link rel=\"stylesheet\">"}</_components.code>"#
        ));
        assert!(!jsx_likely_breaks_downstream_parser(
            r#"<_components.code>{"<script type=\"module\">"}</_components.code>"#
        ));
        assert!(!jsx_likely_breaks_downstream_parser(
            r#"<_components.code>{"{main-deploy,preview-deploy,pr-checks}.yml"}</_components.code>"#
        ));
        assert!(!jsx_likely_breaks_downstream_parser(
            r#"<_components.code>{"@theme { --color-*: initial; }"}</_components.code>"#
        ));
    }

    #[test]
    fn materialise_collection_treats_missing_root_as_empty() {
        // A `CollectionConfig` whose source path no longer exists on
        // disk (stale `zfb.config.ts` entry) must not abort the build —
        // mirrors `zfb_content::build_snapshot`'s lenient handling.
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("shadow_content").join("ghost");
        let mut imports: Vec<ContentImport> = Vec::new();
        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(tmp.path(), &exclude);
        materialise_collection(
            &tmp.path().join("does-not-exist"),
            &dest,
            "ghost",
            &mut imports,
            &ctx,
            None,
            None,
            None,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(imports.is_empty());
    }

    #[test]
    fn entry_module_emits_default_fetch_when_routes_are_empty() {
        // The wrapper is emitted unconditionally so the embedded V8 host
        // sees a Workers-shaped bundle even when
        // no pages exist yet (e.g. a brand-new `pages/` dir). The empty
        // Hono app inside `createPageRouter` 404s every request — that
        // is the documented zero-routes behaviour.
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path();
        write_entry_module(
            shadow,
            &[],
            &EntryModuleInputs {
                render_to_string_module: "react-dom/server",
                content_snapshot_json: None,
                content_imports: &[],
                site: None,
                prefetch_disabled: false,
                mdx_components_import_spec: None,
                base_prefix: None,
            },
        )
        .unwrap();

        let body = fs::read_to_string(shadow.join(SHADOW_ENTRY_FILENAME)).unwrap();
        assert!(body.contains("\"react-dom/server\""));
        assert!(body.contains("export default {"));
        assert!(body.contains("fetch: (request) => __zfb_router(request)"));
        // pages array exists but is empty.
        assert!(body.contains("const __zfb_pages = [\n];"));
    }

    /// Helper: emit `entry.mjs` with a given snapshot string and return
    /// the snapshot literal embedded in the generated source.
    fn entry_module_snapshot_literal(snapshot: Option<&str>) -> String {
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path();
        write_entry_module(
            shadow,
            &[],
            &EntryModuleInputs {
                render_to_string_module: "react-dom/server",
                content_snapshot_json: snapshot,
                content_imports: &[],
                site: None,
                prefetch_disabled: false,
                mdx_components_import_spec: None,
                base_prefix: None,
            },
        )
        .unwrap();
        let body = fs::read_to_string(shadow.join(SHADOW_ENTRY_FILENAME)).unwrap();
        // Pull just the assignment line so the assertion is precise.
        let prefix = "const __zfb_content_snapshot = ";
        let idx = body.find(prefix).expect("snapshot literal assignment");
        let after = &body[idx + prefix.len()..];
        // The line ends with `;\n`. Strip from the first `;` onward.
        let end = after.find(';').expect("statement terminator");
        after[..end].trim().to_string()
    }

    #[test]
    fn snapshot_literal_falls_back_when_json_is_malformed() {
        // M3: validation must reject bare-syntax JSON values that aren't
        // objects with a `collections` field. Each of these would crash
        // `getCollection` at request time, so the bundler swaps in the
        // safe empty-snapshot literal instead.
        for malformed in &["null", "42", "[]", "\"string\"", "{}", "{ \"foo\": 1 }"] {
            let literal = entry_module_snapshot_literal(Some(malformed));
            assert_eq!(
                literal, "{ \"collections\": {} }",
                "malformed snapshot {malformed:?} should fall back to empty; got {literal:?}"
            );
        }
        // Truly invalid JSON also falls back.
        let literal = entry_module_snapshot_literal(Some("not json at all"));
        assert_eq!(literal, "{ \"collections\": {} }");
    }

    #[test]
    fn snapshot_literal_preserves_valid_snapshot() {
        // A well-formed snapshot — a top-level object with a
        // `collections` object — must round-trip through the validator
        // unchanged so the worker sees the real content map.
        let valid = r#"{"collections":{"blog":[{"slug":"hello","frontmatter":{"title":"Hello"},"body":"","module_specifier":"mdx://blog/hello","rel_path":"hello.mdx"}]}}"#;
        let literal = entry_module_snapshot_literal(Some(valid));
        assert_eq!(literal, valid);
    }

    #[test]
    fn snapshot_literal_falls_back_when_collections_is_not_object() {
        // The `collections` field exists but is the wrong shape (an
        // array). The validator must still reject — the runtime indexes
        // collections by name, so an array would crash.
        let bad = r#"{"collections":["a","b"]}"#;
        let literal = entry_module_snapshot_literal(Some(bad));
        assert_eq!(literal, "{ \"collections\": {} }");
    }

    #[test]
    fn snapshot_literal_uses_empty_when_input_is_none() {
        let literal = entry_module_snapshot_literal(None);
        assert_eq!(literal, "{ \"collections\": {} }");
    }

    #[test]
    fn yaml_frontmatter_is_stripped_for_mdx() {
        let raw = "---\ntitle: Hello\n---\n# Body\n";
        assert_eq!(strip_yaml_frontmatter(raw), "# Body\n");
        let no_fm = "# Body only\n";
        assert_eq!(strip_yaml_frontmatter(no_fm), no_fm);
    }

    #[test]
    fn bundle_mock_path_writes_bundle_and_records_routes() {
        let tmp = tempfile::tempdir().unwrap();
        let input = make_minimal_input(&tmp);

        let out = bundle(input).expect("mock bundle should succeed");
        assert!(out.bundle_path.exists());
        assert_eq!(out.manifest.framework, "preact");
        assert_eq!(out.manifest.jsx_import_source, "preact");
        assert!(out
            .manifest
            .hydrate_shim_specifier
            .starts_with("zfb:internal/"));
        assert_eq!(out.manifest.routes.len(), 1);
        assert_eq!(out.manifest.routes[0].route, "/");
        assert_eq!(
            out.manifest.routes[0].source_path,
            PathBuf::from("pages/index.tsx")
        );
    }

    #[test]
    fn for_project_defaults_worker_only_routes_and_bundle_basename_to_none() {
        // Defaults for the runtime-trim fields (zfb#283). `None` keeps
        // legacy single-bundle behavior — write_entry_module imports every
        // discovered route, and the bundle filename stays `bundle.mjs`.
        let input = BundlerInput::for_project(
            PathBuf::from("/tmp/dummy"),
            Framework::Preact,
            BundleMode::Production,
            PathBuf::from("/tmp/dummy/dist"),
            None,
        );
        assert!(
            input.worker_only_routes.is_none(),
            "worker_only_routes should default to None"
        );
        assert!(
            input.bundle_basename.is_none(),
            "bundle_basename should default to None (so callers see legacy bundle.mjs filename)"
        );
    }

    #[test]
    fn bundle_basename_override_changes_emitted_filename() {
        // The bundle filename is selectable so two passes (full SSG +
        // runtime-only) can coexist in the same outdir without clobber.
        let tmp = tempfile::tempdir().unwrap();
        let mut input = make_minimal_input(&tmp);
        input.bundle_basename = Some("bundle-runtime.mjs".to_string());

        let out = bundle(input).expect("mock bundle with custom basename should succeed");

        assert_eq!(
            out.bundle_path.file_name().and_then(|s| s.to_str()),
            Some("bundle-runtime.mjs"),
            "bundle_path should honor bundle_basename"
        );
        assert!(out.bundle_path.exists(), "renamed bundle should be on disk");
        assert_eq!(
            out.sourcemap_path.file_name().and_then(|s| s.to_str()),
            Some("bundle-runtime.mjs.map"),
            "sourcemap suffix should track the renamed bundle"
        );
        // Manifest's bundle_basename is derived from the on-disk filename
        // — it must reflect the override too.
        assert_eq!(out.manifest.bundle_basename, "bundle-runtime.mjs");
    }

    #[test]
    fn worker_only_routes_preserves_full_manifest_routes() {
        // The filter only narrows what `write_entry_module` imports into
        // the synthetic entry — `BundlerOutput.manifest.routes` continues
        // to report every discovered route so build-time bookkeeping
        // (post-build manifest, route-table dumps) sees the full picture.
        // This is the documented contract on the field.
        let tmp = tempfile::tempdir().unwrap();
        // Fixture: two pages. `pages/index.tsx` (SSG) + `pages/api.tsx`
        // (SSR-only, by intent in this test).
        let root = tmp.path().to_path_buf();
        fs::create_dir_all(root.join("pages")).unwrap();
        fs::write(
            root.join("pages/index.tsx"),
            "export default function Home() { return null; }\n",
        )
        .unwrap();
        fs::write(
            root.join("pages/api.tsx"),
            "export const prerender = false;\n\
             export default function Api() { return null; }\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("content")).unwrap();
        fs::create_dir_all(root.join("components")).unwrap();
        fs::create_dir_all(root.join("layouts")).unwrap();

        let mut input = make_minimal_input(&tmp);
        // make_minimal_input only created pages/index.tsx; re-point the
        // input at our two-page tree above.
        input.project_root = root.clone();
        input.outdir = root.join("dist");
        let mut filter: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        filter.insert("/api".to_string());
        input.worker_only_routes = Some(filter);

        let out = bundle(input).expect("mock bundle with filter should succeed");

        let route_keys: Vec<&str> = out
            .manifest
            .routes
            .iter()
            .map(|r| r.route.as_str())
            .collect();
        assert!(
            route_keys.contains(&"/"),
            "manifest.routes must still include the prerendered route, got {:?}",
            route_keys
        );
        assert!(
            route_keys.contains(&"/api"),
            "manifest.routes must still include the SSR route, got {:?}",
            route_keys
        );
        assert_eq!(
            route_keys.len(),
            2,
            "manifest.routes should report every discovered route regardless of worker_only_routes"
        );
    }

    // ── Issue #1668 part 1: SSR shadow workspace layout re-root ─────────────

    /// A mock-subprocess SSR input for `project_root` — skips the esbuild
    /// subprocess so a fixture needs no real binary, but still runs the full
    /// materialise → prune pipeline whose shadow layout these tests inspect.
    fn mock_ssr_input(project_root: &Path) -> BundlerInput {
        BundlerInput {
            mock_subprocess_output: Some("export default {};\n".to_string()),
            external: vec![
                "preact".into(),
                "preact-render-to-string".into(),
                "@takazudo/zfb-runtime".into(),
            ],
            ..BundlerInput::for_project(
                project_root.to_path_buf(),
                Framework::Preact,
                BundleMode::Production,
                project_root.join("dist"),
                None,
            )
        }
    }

    /// Every FILE (not directory) under `root`, as sorted forward-slash
    /// relative paths. Symlinks (e.g. a `node_modules` link) are recorded as a
    /// single entry, never descended into, so the set stays the shadow's own
    /// materialised layout.
    fn collect_shadow_files(root: &Path) -> Vec<String> {
        fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let file_type = entry.file_type().expect("dirent file type");
                if file_type.is_dir() {
                    walk(&entry.path(), base, out);
                } else {
                    let rel = entry.path().strip_prefix(base).unwrap().to_path_buf();
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        let mut out = Vec::new();
        walk(root, root, &mut out);
        out.sort();
        out
    }

    #[test]
    fn ssr_shadow_layout_reroots_under_project_rel_and_is_a_noop_without_a_workspace() {
        // The SAME project content, built two ways: standalone (no workspace)
        // and as a claimed member of a pnpm workspace. The workspace build must
        // nest the WHOLE project mirror under the project's workspace-relative
        // subpath, and its layout — stripped of that prefix — must equal the
        // standalone (flat) layout. That single equality proves BOTH acceptance
        // criteria: a non-workspace build is a byte-identical no-op, and a
        // workspace build nests the project mirror carrying project-only
        // content.
        fn seed_project(root: &Path) {
            for dir in ["pages", "content", "components", "layouts"] {
                fs::create_dir_all(root.join(dir)).unwrap();
            }
            fs::write(
                root.join("layouts/default.tsx"),
                "export default function L({ children }) { return children; }\n",
            )
            .unwrap();
            fs::write(
                root.join("pages/index.tsx"),
                "export default function Home() { return null; }\n",
            )
            .unwrap();
        }

        // (a) Standalone project — no workspace marker anywhere above it.
        let flat_tmp = tempfile::tempdir().unwrap();
        let flat_root = flat_tmp.path();
        seed_project(flat_root);
        let mut flat_session = ShadowSession::new(flat_root).unwrap();
        bundle_with_session(mock_ssr_input(flat_root), Some(&mut flat_session))
            .expect("standalone mock bundle should succeed");
        let flat_files = collect_shadow_files(flat_session.shadow_root());

        // (b) The same project as a claimed pnpm-workspace member.
        let ws_tmp = tempfile::tempdir().unwrap();
        let ws_root = ws_tmp.path();
        fs::write(
            ws_root.join("pnpm-workspace.yaml"),
            "packages:\n  - '.'\n  - 'sub-packages/*'\n",
        )
        .unwrap();
        let ws_project = ws_root.join("sub-packages/host");
        seed_project(&ws_project);
        let mut ws_session = ShadowSession::new(&ws_project).unwrap();
        bundle_with_session(mock_ssr_input(&ws_project), Some(&mut ws_session))
            .expect("workspace mock bundle should succeed");
        let ws_files = collect_shadow_files(ws_session.shadow_root());

        let prefix = "sub-packages/host/";
        assert!(
            ws_files.iter().all(|file| file.starts_with(prefix)),
            "every workspace shadow file must nest under {prefix}: {ws_files:?}"
        );
        assert!(
            !ws_session.shadow_root().join("entry.mjs").exists(),
            "entry.mjs must live in the nested project mirror, not the flat work root"
        );
        assert!(
            ws_session
                .shadow_root()
                .join("sub-packages/host/entry.mjs")
                .exists(),
            "the nested project mirror must carry entry.mjs"
        );
        let ws_stripped: Vec<String> = ws_files
            .iter()
            .map(|file| file.strip_prefix(prefix).unwrap().to_string())
            .collect();
        assert_eq!(
            ws_stripped, flat_files,
            "the workspace layout minus its project-rel prefix must equal the flat layout"
        );
        for expected in [
            "__zfb_internal_hydrate.jsx",
            "entry.mjs",
            "layouts/default.tsx",
            "pages/index.tsx",
            "tsconfig.json",
        ] {
            assert!(
                flat_files.iter().any(|file| file == expected),
                "the flat layout must contain {expected}: {flat_files:?}"
            );
        }
    }

    #[test]
    fn ssr_shadow_session_prunes_and_skips_workspace_root_relative_destinations() {
        // Same-session edit/delete against a workspace project, proving the
        // ShadowWriter/ShadowSession bookkeeping tracks the re-rooted,
        // workspace-root-relative destinations. `ShadowSession` owns a fresh
        // random tempdir per process and has no layout version field, so there
        // is nothing to "bump" — the guarantee is that the visited/prune pass
        // and the (mtime,size) skip cache both key off the nested dest.
        //
        // Plain `.tsx` pages are SYMLINKED into the shadow (no transform to
        // preserve), so they live in the `source_skip` cache + visited/prune
        // bookkeeping — symlinks are never recorded in `written`. The
        // white-box discriminator is the `source_skip_plain_file` idiom:
        // replace a dest with a corrupt REAL file — a re-materialise removes
        // it and re-symlinks; a correct (mtime,size) skip leaves it.
        let ws_tmp = tempfile::tempdir().unwrap();
        let ws_root = ws_tmp.path();
        fs::write(
            ws_root.join("pnpm-workspace.yaml"),
            "packages:\n  - '.'\n  - 'sub-packages/*'\n",
        )
        .unwrap();
        let project = ws_root.join("sub-packages/host");
        for dir in ["pages", "content", "components", "layouts"] {
            fs::create_dir_all(project.join(dir)).unwrap();
        }
        fs::write(
            project.join("layouts/default.tsx"),
            "export default function L({ children }) { return children; }\n",
        )
        .unwrap();
        fs::write(project.join("pages/index.tsx"), "export const V = 1;\n").unwrap();
        fs::write(project.join("pages/about.tsx"), "export const V = 2;\n").unwrap();

        let mut session = ShadowSession::new(&project).unwrap();
        bundle_with_session(mock_ssr_input(&project), Some(&mut session))
            .expect("tick 1 mock bundle");

        let ws_rel = Path::new("sub-packages/host");
        let shadow_root = session.shadow_root().to_path_buf();
        let index_dest = shadow_root.join(ws_rel).join("pages/index.tsx");
        let about_dest = shadow_root.join(ws_rel).join("pages/about.tsx");
        let layout_dest = shadow_root.join(ws_rel).join("layouts/default.tsx");
        assert!(
            index_dest.exists() && about_dest.exists() && layout_dest.exists(),
            "tick 1 must materialise the project mirror under {ws_rel:?}"
        );
        // The incremental skip cache is keyed by the workspace-root-relative
        // destination — direct proof the writer's `rel_of` strips the WORK
        // root, not the project mirror.
        assert!(
            session
                .source_skip
                .contains_key(&ws_rel.join("pages/index.tsx")),
            "source_skip must be keyed by the workspace-root-relative dest: {:?}",
            session.source_skip.keys().collect::<Vec<_>>()
        );

        // White-box discriminators against the RE-ROOTED dest keys: replace
        // each dest symlink with a corrupt REAL file, then leave layouts
        // UNCHANGED but EDIT index (a size change forces re-materialise).
        for dest in [&index_dest, &layout_dest] {
            fs::remove_file(dest).unwrap();
            fs::write(dest, b"__CORRUPT_NOT_THE_SOURCE__").unwrap();
        }
        fs::write(
            project.join("pages/index.tsx"),
            "export const V = 1234567;\n",
        )
        .unwrap();
        fs::remove_file(project.join("pages/about.tsx")).unwrap();

        bundle_with_session(mock_ssr_input(&project), Some(&mut session))
            .expect("tick 2 mock bundle");

        // Deleted source → its workspace-root-relative dest is pruned.
        assert!(
            !about_dest.exists(),
            "the deleted page's workspace-root-relative dest must be pruned"
        );
        // Edited source (size changed) → NOT skipped → re-materialised at the
        // re-rooted dest, so the corruption is replaced by a symlink reading
        // the new source bytes.
        assert_eq!(
            fs::read_to_string(&index_dest).unwrap(),
            "export const V = 1234567;\n",
            "the edited page must be re-materialised at its re-rooted dest"
        );
        // Unchanged source → skipped via (mtime,size) on the re-rooted key, so
        // the corruption survives untouched.
        assert_eq!(
            fs::read(&layout_dest).unwrap(),
            b"__CORRUPT_NOT_THE_SOURCE__",
            "an unchanged page must be skipped via (mtime,size) on its re-rooted dest key"
        );
    }

    #[test]
    fn shadow_path_for_project_path_maps_workspace_siblings_into_the_work_mirror() {
        let work = Path::new("/work");
        let first_party = Path::new("/ws");
        let project = Path::new("/ws/sub-packages/host");
        let shadow = Path::new("/work/sub-packages/host");

        // Under project_root → the project mirror (unchanged tier).
        assert_eq!(
            shadow_path_for_project_path(
                &project.join("pages/index.tsx"),
                project,
                first_party,
                shadow,
                work,
                None,
            ),
            shadow.join("pages/index.tsx"),
        );
        // The project root itself → the project mirror root.
        assert_eq!(
            shadow_path_for_project_path(project, project, first_party, shadow, work, None),
            shadow.to_path_buf(),
        );
        // node_modules under the project → the isolation slot (unchanged tier).
        assert_eq!(
            shadow_path_for_project_path(
                &project.join("node_modules/preact/index.js"),
                project,
                first_party,
                shadow,
                work,
                None,
            ),
            shadow
                .join(".zfb-exact-isolation")
                .join("node_modules/preact/index.js"),
        );
        // A workspace sibling (under first_party_root, outside the project) →
        // its workspace-relative slot in the work mirror (the new tier).
        assert_eq!(
            shadow_path_for_project_path(
                &first_party.join("lib/shared/contract.ts"),
                project,
                first_party,
                shadow,
                work,
                None,
            ),
            work.join("lib/shared/contract.ts"),
        );
        // Beyond the first-party root entirely → the real path (live-tree
        // escape, unchanged).
        let outside = Path::new("/elsewhere/thing.ts");
        assert_eq!(
            shadow_path_for_project_path(outside, project, first_party, shadow, work, None),
            outside.to_path_buf(),
        );
        // No workspace (first_party_root == project_root): the sibling tier is
        // inert, so a non-project path stays the real path — byte-identical to
        // the pre-#1668 two-tier mapping.
        let flat_project = Path::new("/proj");
        let flat_shadow = Path::new("/work2");
        let non_project = Path::new("/proj-adjacent/x.ts");
        assert_eq!(
            shadow_path_for_project_path(
                non_project,
                flat_project,
                flat_project,
                flat_shadow,
                flat_shadow,
                None,
            ),
            non_project.to_path_buf(),
        );
    }

    #[test]
    fn ssr_shadow_workspace_node_modules_link_created_under_bundle_exclude() {
        // INVERSION of the former `ssr_shadow_workspace_node_modules_link_
        // honors_bundle_exclude` (issue #1693, #1672-style guard-inversion
        // ritual): that test pinned a guard which NEVER created the
        // workspace-hoisted `<work>/node_modules` link once `bundle.exclude`
        // went active, reasoning that the link was an unauditable
        // resurrection hatch. Once #1692's wholesale sibling mirror stages
        // sibling sources directly under `<work>/<workspace_rel>/...`
        // (outside `<shadow>`), those siblings have no ancestor
        // `node_modules` to resolve a bare import against unless this link
        // exists — so removing it under exclusions broke every sibling bare
        // import, not just excluded ones.
        //
        // The link is now created REGARDLESS of `bundle.exclude`. The old
        // "unauditable" premise is gone too: the fail-closed metafile audit
        // (`audit_metafile_exclusions_at_path`, exercised directly in
        // `metafile_audit_hard_fails_on_workspace_node_modules_resurrection`
        // below) checks every esbuild-resolved input against the TIER-2
        // workspace-relative `BundleExcludeMatcher` (issue #1676), which is
        // armed for exactly a path under `first_party_root` but outside
        // `project_root` — so a resurrection climbing this link is caught,
        // not silently missed.
        let ws_tmp = tempfile::tempdir().unwrap();
        let ws_root = ws_tmp.path();
        fs::write(
            ws_root.join("pnpm-workspace.yaml"),
            "packages:\n  - '.'\n  - 'sub-packages/*'\n",
        )
        .unwrap();
        // A real workspace-hoisted node_modules so the link has a target.
        fs::create_dir_all(ws_root.join("node_modules/left-pad")).unwrap();
        fs::write(
            ws_root.join("node_modules/left-pad/index.js"),
            "module.exports = 0;\n",
        )
        .unwrap();
        let project = ws_root.join("sub-packages/host");
        for dir in ["pages", "content", "components", "layouts"] {
            fs::create_dir_all(project.join(dir)).unwrap();
        }
        fs::write(
            project.join("pages/index.tsx"),
            "export default function Home() { return null; }\n",
        )
        .unwrap();

        let work_nm = |session: &ShadowSession| session.shadow_root().join("node_modules");
        let link_targets_workspace_node_modules = |session: &ShadowSession| {
            fs::read_link(work_nm(session))
                .map(|target| target == ws_root.join("node_modules"))
                .unwrap_or(false)
        };

        // Tick 1 — empty exclude: the workspace-hoisted link IS created
        // (unchanged from before #1693).
        let mut session = ShadowSession::new(&project).unwrap();
        bundle_with_session(mock_ssr_input(&project), Some(&mut session))
            .expect("tick 1 (empty exclude) mock bundle");
        assert!(
            link_targets_workspace_node_modules(&session),
            "an empty bundle.exclude must create the workspace-hoisted node_modules link"
        );

        // Tick 2 — same session, exclusions now active: the link stays
        // PRESENT (the #1693 inversion) instead of being torn down.
        let excluded_input = BundlerInput {
            bundle_exclude: vec!["**/*.secret".to_string()],
            ..mock_ssr_input(&project)
        };
        bundle_with_session(excluded_input, Some(&mut session))
            .expect("tick 2 (active exclude) mock bundle");
        assert!(
            link_targets_workspace_node_modules(&session),
            "an active bundle.exclude must NOT remove the workspace-hoisted node_modules link \
             (#1693) — sibling bare imports need it; the metafile audit is the enforcement point"
        );
    }

    #[test]
    fn metafile_audit_hard_fails_on_workspace_node_modules_resurrection() {
        // Fail-closed proof for the #1693 contract: now that the workspace
        // -hoisted `<work>/node_modules` link is always created (see the
        // inverted guard test above), a synthetic metafile whose `inputs`
        // record esbuild having resolved THROUGH that link into a package an
        // active `bundle.exclude` pattern matches — workspace-relative,
        // tier-2 (#1676) — must be caught by `audit_metafile_exclusions`,
        // exactly the way it already catches a project-relative leak. This
        // is the direct replacement for the deleted "never create the link"
        // guard: enforcement moved from link-absence to audit-detection.
        let ws_tmp = tempfile::tempdir().unwrap();
        let ws_root = ws_tmp.path();
        fs::write(
            ws_root.join("pnpm-workspace.yaml"),
            "packages:\n  - '.'\n  - 'sub-packages/*'\n",
        )
        .unwrap();
        let secret_pkg = ws_root.join("node_modules/secret-pkg/index.js");
        fs::create_dir_all(secret_pkg.parent().unwrap()).unwrap();
        fs::write(&secret_pkg, "module.exports = 'leaked';\n").unwrap();

        let project = ws_root.join("sub-packages/host");
        fs::create_dir_all(&project).unwrap();
        let shadow_root = ws_tmp.path().join("shadow");
        fs::create_dir_all(&shadow_root).unwrap();

        // The excluded package resolved through the always-present
        // `<work>/node_modules` link: esbuild records the absolute path it
        // climbed to, which lands under `first_party_root` (the workspace
        // root) but OUTSIDE `project_root` — precisely the tier-2 shape.
        // Deliberately NOT canonicalised: spelling 1 below (the logical key
        // joined onto `project_root`) never canonicalises either, and
        // `ws_root` is used lexically throughout this test, so comparing
        // like-for-like avoids a spurious `/var` vs `/private/var` mismatch
        // on macOS that a canonicalised key would introduce here.
        let metafile = format!(
            r#"{{"inputs": {{ {:?}: {{ "imports": [] }} }} }}"#,
            secret_pkg.to_string_lossy()
        );

        let matcher = BundleExcludeMatcher::new(&["node_modules/secret-pkg/**".to_string()])
            .unwrap()
            .with_workspace_root(&project);
        let is_excluded = |abs: &Path| matcher.is_excluded(abs, &project);

        let error = crate::metafile_deps::audit_metafile_exclusions(
            metafile.as_bytes(),
            &is_excluded,
            &shadow_root,
            &project,
        )
        .expect_err(
            "the metafile audit must hard-fail when an excluded sibling-workspace package \
             resolved through the workspace-hoisted node_modules link",
        );
        let message = format!("{error:#}");
        assert!(
            message.contains("bundle.exclude leak"),
            "the audit error must name the leak: {message}"
        );
        assert!(
            message.contains("secret-pkg"),
            "the audit error must list the offending resolved path: {message}"
        );
    }

    // ---- Issue #1706, Stage Escape Guards — Guard (b): SSR work-mirror
    // stage-escape audit. The primitive itself is exhaustively unit-tested in
    // `metafile_deps.rs`; these tests pin the BUNDLER's wiring arg-shape —
    // metafile_cwd = `shadow` (esbuild's cwd, nested BELOW the stage root),
    // stage_roots = [`work`], first_party_root = the widened workspace root —
    // which the real-esbuild end-to-end proof (#1708) exercises through a live
    // `zfb build`. -------------------------------------------------------------

    /// A bare first-party PACKAGE-NAME import that climbed the always-present
    /// `<work>/node_modules` symlink to a LIVE workspace sibling (case 2) must
    /// be flagged with the bundler's exact arg shape, regardless of
    /// `bundle.exclude` (the stage-escape audit takes no exclusion predicate).
    #[cfg(unix)]
    #[test]
    fn stage_escape_audit_flags_workspace_package_name_reach_through_work_node_modules() {
        let tmp = tempfile::tempdir().unwrap();
        // Live sibling source, OUTSIDE the stage.
        let ws_root = tmp.path().join("ws");
        let live_sibling = ws_root.join("packages/shared");
        fs::create_dir_all(&live_sibling).unwrap();
        fs::write(live_sibling.join("index.ts"), "export const x = 1;\n").unwrap();

        // Work mirror (stage root) with esbuild's cwd nested below it.
        let work = tmp.path().join("work");
        let shadow = work.join("host"); // esbuild cwd == metafile_cwd
        fs::create_dir_all(&shadow).unwrap();
        // The wholesale `<work>/node_modules` layout: `@acme` is a real dir;
        // the package itself is the pnpm-style symlink to the live sibling
        // (canonical target carries NO further node_modules segment).
        fs::create_dir_all(work.join("node_modules/@acme")).unwrap();
        std::os::unix::fs::symlink(&live_sibling, work.join("node_modules/@acme/shared")).unwrap();

        // esbuild (cwd = `shadow`) records the input it resolved through the
        // link, keyed relative to its cwd: `../node_modules/@acme/shared/...`.
        let metafile = r#"{"inputs": {"../node_modules/@acme/shared/index.ts": {"imports": []}}}"#;

        let error = crate::metafile_deps::audit_metafile_stage_escape(
            metafile.as_bytes(),
            &shadow,
            &[work.as_path()],
            &ws_root,
        )
        .expect_err(
            "a package-name import climbing <work>/node_modules to a live sibling must be flagged",
        );
        let message = format!("{error:#}");
        assert!(message.contains("stage-escape audit"), "{message}");
        assert!(
            message.contains("shared"),
            "the offending input must be listed: {message}"
        );
    }

    /// No false positive: ordinary staged inputs — the project shadow's own
    /// source, a wholesale-mirrored sibling staged under `work`, and a genuine
    /// third-party dep resolving to `.pnpm` (case 3) — must ALL pass. Even
    /// when a staged file's canonical path escapes the stage, the LOGICAL
    /// spelling still names a location inside `work` (case 1, allowed).
    #[cfg(unix)]
    #[test]
    fn stage_escape_audit_allows_staged_project_sibling_and_third_party_inputs() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_root = tmp.path().join("ws");
        fs::create_dir_all(&ws_root).unwrap();
        let work = tmp.path().join("work");
        let shadow = work.join("host");
        fs::create_dir_all(&shadow).unwrap();
        // Staged project source (a real file inside the stage).
        fs::write(shadow.join("app.ts"), "export const a = 1;\n").unwrap();
        // Wholesale-mirrored sibling staged under `work`.
        fs::create_dir_all(work.join("lib/shared")).unwrap();
        fs::write(work.join("lib/shared/index.ts"), "export const b = 2;\n").unwrap();
        // A genuine third-party dep: the `node_modules/<pkg>` symlink points
        // into `.pnpm`, so its canonical path keeps a node_modules segment.
        let pnpm_pkg = work.join("node_modules/.pnpm/left-pad@1.0.0/node_modules/left-pad");
        fs::create_dir_all(&pnpm_pkg).unwrap();
        fs::write(pnpm_pkg.join("index.js"), "module.exports = 1;\n").unwrap();
        std::os::unix::fs::symlink(&pnpm_pkg, work.join("node_modules/left-pad")).unwrap();

        let metafile = r#"{"inputs": {
            "app.ts": {"imports": []},
            "../lib/shared/index.ts": {"imports": []},
            "../node_modules/left-pad/index.js": {"imports": []}
        }}"#;

        crate::metafile_deps::audit_metafile_stage_escape(
            metafile.as_bytes(),
            &shadow,
            &[work.as_path()],
            &ws_root,
        )
        .expect(
            "staged project source, staged sibling, and third-party dep must all pass the audit",
        );
    }

    #[test]
    fn sibling_bare_import_resolves_via_workspace_node_modules_under_exclude() {
        // Mock-level staging assertion (issue #1693 acceptance criterion 3):
        // a workspace-sibling file discovered through the existing
        // preprocessing-graph closure (issue #1668 part 2 — the wholesale
        // directory mirror lands in #1692) is staged into `<work>/lib/
        // shared/...`, and — with `bundle.exclude` active for an UNRELATED
        // pattern — the workspace-hoisted `<work>/node_modules` link still
        // exists as this test's sibling directory's DIRECT ancestor within
        // `<work>`. That is the exact filesystem precondition esbuild's
        // ordinary node_modules walk-up needs to resolve a bare import
        // (`left-pad`) from the staged sibling: no real esbuild subprocess
        // runs here (real-esbuild confirmation belongs to sub-issue #1698),
        // this asserts the staged tree SHAPE esbuild would walk.
        let ws_tmp = tempfile::tempdir().unwrap();
        let ws_root = ws_tmp.path();
        fs::write(
            ws_root.join("pnpm-workspace.yaml"),
            "packages:\n  - '.'\n  - 'sub-packages/*'\n",
        )
        .unwrap();
        fs::create_dir_all(ws_root.join("node_modules/left-pad")).unwrap();
        fs::write(
            ws_root.join("node_modules/left-pad/index.js"),
            "module.exports = function concat() {};\n",
        )
        .unwrap();

        let project = ws_root.join("sub-packages/host");
        for dir in ["pages", "content", "components", "layouts", "src"] {
            fs::create_dir_all(project.join(dir)).unwrap();
        }
        fs::write(
            project.join("pages/index.tsx"),
            "export default function Home() { return null; }\n",
        )
        .unwrap();

        // A plain project entry (staged via `plugin_alias_entries`, matching
        // the existing exact-target discovery path) that plainly imports a
        // workspace-sibling module — no `?raw`/glob/worker syntax needed:
        // `discover_module_preprocessing_with_context` walks the whole
        // relative-import closure, not just preprocessing-triggering edges.
        let entry = project.join("src/entry.ts");
        fs::write(
            &entry,
            "import { value } from '../../../lib/shared/widget';\n\
             export const entry = value;\n",
        )
        .unwrap();
        let sibling = ws_root.join("lib/shared/widget.ts");
        fs::create_dir_all(sibling.parent().unwrap()).unwrap();
        fs::write(
            &sibling,
            "import concat from 'left-pad';\n\
             export const value = concat('x', 5, '0');\n",
        )
        .unwrap();

        let input = BundlerInput {
            // Excludes something unrelated — proves the link/staging above
            // is not gated on exclusions being empty.
            bundle_exclude: vec!["**/*.secret".to_string()],
            plugin_alias_entries: vec![(
                "plugin:sibling-bare-entry".to_string(),
                entry.to_string_lossy().into_owned(),
            )],
            ..mock_ssr_input(&project)
        };

        let mut session = ShadowSession::new(&project).unwrap();
        bundle_with_session(input, Some(&mut session))
            .expect("a workspace sibling with a bare import must bundle under active exclusions");

        let work_root = fs::canonicalize(session.shadow_root()).unwrap();
        let staged_sibling = work_root.join("lib/shared/widget.ts");
        assert!(
            staged_sibling.is_file(),
            "the workspace-sibling module must be staged into the work mirror: {}",
            staged_sibling.display()
        );
        let work_node_modules = work_root.join("node_modules");
        assert!(
            fs::symlink_metadata(&work_node_modules)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false),
            "the workspace-hoisted node_modules link must exist under active exclusions"
        );
        assert!(
            work_node_modules.join("left-pad/index.js").is_file(),
            "the bare dependency must be reachable from the staged sibling's nearest ancestor \
             node_modules ({}) — the exact resolution root esbuild's node_modules walk-up would \
             find climbing from {}",
            work_node_modules.display(),
            staged_sibling.display()
        );
        // Both the staged sibling's containing dir and the node_modules link
        // are direct children of the SAME work root, so an ancestor walk
        // from the sibling (lib/shared → lib → work root) reaches
        // `<work>/node_modules` without crossing any other node_modules.
        assert_eq!(
            staged_sibling
                .strip_prefix(&work_root)
                .unwrap()
                .components()
                .count(),
            3,
            "sibling must be nested exactly 2 dirs below the work root (lib/shared/widget.ts)"
        );
    }

    #[test]
    fn server_secrets_are_not_bundled() {
        // Real esbuild test (gated). Verifies a SECRET_ env var never
        // appears in the output, while a PUBLIC_ var does. It also locks the
        // exact-expression precedence seam: an operator `bundle.define`
        // overrides the generated PUBLIC value for both supported spellings.
        let Some(bin) = locate_real_esbuild() else {
            eprintln!("[server_secrets_are_not_bundled] no esbuild binary on PATH; skipping");
            return;
        };

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        fs::create_dir_all(root.join("pages")).unwrap();
        fs::create_dir_all(root.join("content")).unwrap();
        fs::create_dir_all(root.join("components")).unwrap();
        fs::create_dir_all(root.join("layouts")).unwrap();
        fs::write(
            root.join("pages/index.tsx"),
            r#"
                const apiUrl = process.env.PUBLIC_API_URL;
                const processCollision = process.env.PUBLIC_COLLISION;
                const importMetaCollision = import.meta.env.PUBLIC_COLLISION;
                const secret = process.env.SECRET_KEY;
                export default function Home() {
                  return apiUrl + " " + processCollision + " " + importMetaCollision + " " + secret;
                }
            "#,
        )
        .unwrap();

        let mut defs = HashMap::new();
        defs.insert("PUBLIC_API_URL".into(), "https://example.test".into());
        defs.insert("PUBLIC_COLLISION".into(), "public-env-must-not-win".into());
        defs.insert(
            "SECRET_KEY".into(),
            "this-must-not-appear-in-the-bundle".into(),
        );

        // Locate workspace node_modules so esbuild can resolve
        // @takazudo/zfb-runtime + preact-render-to-string. Pre-#197 this test
        // was silently skipped because no esbuild was downloaded; now that
        // build.rs always populates the binary slot, the test runs and needs
        // real dependency resolution. In pnpm hoisted layouts these packages
        // live under .pnpm/, not at the top level, so check the realistic
        // location and skip if not present (CI / first-run after fresh clone).
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .expect("workspace root from CARGO_MANIFEST_DIR");
        let workspace_node_modules = workspace_root.join("node_modules");
        let zfb_pkg_node_modules = workspace_root.join("packages/zfb/node_modules");
        // Use packages/zfb/node_modules where pnpm symlinks the runtime deps;
        // fall back to root node_modules if that path doesn't exist.
        let nm_dir = if zfb_pkg_node_modules.join("@takazudo/zfb-runtime").exists() {
            Some(zfb_pkg_node_modules)
        } else if workspace_node_modules
            .join("@takazudo/zfb-runtime")
            .exists()
        {
            Some(workspace_node_modules)
        } else {
            eprintln!(
                "[server_secrets_are_not_bundled] @takazudo/zfb-runtime not found in workspace node_modules; skipping (run pnpm install first)"
            );
            return;
        };

        let input = BundlerInput {
            project_root: root.clone(),
            pages_dir: PathBuf::from("pages"),
            injected_pages_root: None,
            content_dir: PathBuf::from("content"),
            content_collections: Vec::new(),
            components_dir: PathBuf::from("components"),
            layouts_dir: PathBuf::from("layouts"),
            framework: Framework::Preact,
            define_vars: BTreeMap::from([
                (
                    "process.env.PUBLIC_COLLISION".to_string(),
                    "\"operator-process-define\"".to_string(),
                ),
                (
                    "import.meta.env.PUBLIC_COLLISION".to_string(),
                    "\"operator-import-meta-define\"".to_string(),
                ),
            ]),
            public_env_vars: defs,
            tsconfig_paths: BTreeMap::new(),
            external: vec!["preact".into()],
            main_fields: Vec::new(),
            extra_loader_args: Vec::new(),
            outdir: root.join("dist"),
            mode: BundleMode::Production,
            minify: false,
            esbuild_binary: Some(bin),
            mock_subprocess_output: None,
            content_snapshot_json: None,
            node_modules_dir: nm_dir,
            node_modules_preserve_symlinks: false,
            pipeline_spec: zfb_content::PipelineSpec::default(),
            resolve_markdown_links: None,
            site: None,
            prefetch_disabled: false,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            worker_only_routes: None,
            bundle_basename: None,
            css_module_class_maps: HashMap::new(),
            mdx_components_file: None,
            bundle_exclude: Vec::new(),
            base_prefix: None,
        };

        let out = bundle(input).expect("real esbuild bundle should succeed");
        let body = fs::read_to_string(&out.bundle_path).unwrap();
        // The contract is **value** secrecy: the secret value must not
        // appear in the bundle. The unreplaced `process.env.SECRET_KEY`
        // *access expression* may stay (esbuild does not delete dead
        // reads), but the secret value never does because we didn't
        // emit a `--define` for it.
        assert!(
            !body.contains("this-must-not-appear-in-the-bundle"),
            "SECRET_KEY value leaked into the bundle"
        );
        assert!(
            body.contains("https://example.test"),
            "PUBLIC_API_URL value should be inlined"
        );
        assert!(
            body.contains("operator-process-define"),
            "explicit process.env PUBLIC define should win: {body}"
        );
        assert!(
            body.contains("operator-import-meta-define"),
            "explicit import.meta.env PUBLIC define should win: {body}"
        );
        assert!(
            !body.contains("public-env-must-not-win"),
            "generated PUBLIC payload overrode an explicit define: {body}"
        );
    }

    #[test]
    fn mdx_components_file_resolves_through_esbuild_with_relative_import() {
        // Real esbuild test (gated). Proves the two resolutions the whole
        // #616 feature hinges on — neither is covered by the text/copy
        // unit tests:
        //   1. esbuild resolves the `import ... from "./mdx-components.tsx"`
        //      that `entry.mjs` emits from the shadow root.
        //   2. the override file's OWN relative import (`./components/MyH2`)
        //      resolves against the SHADOW tree — i.e. the file was copied
        //      into shadow (not symlinked back to the project root). This is
        //      the "easily-missed materialization step" the issue flags.
        let Some(bin) = locate_real_esbuild() else {
            eprintln!("[mdx_components_file_resolves_through_esbuild] no esbuild binary; skipping");
            return;
        };

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        fs::create_dir_all(root.join("pages")).unwrap();
        fs::create_dir_all(root.join("content")).unwrap();
        fs::create_dir_all(root.join("components")).unwrap();
        fs::create_dir_all(root.join("layouts")).unwrap();
        fs::write(
            root.join("pages/index.tsx"),
            "export default function Home() { return null; }\n",
        )
        .unwrap();
        // The override component lives under components/ — materialised to
        // shadow/components, so a shadow-ROOT importer reaching it via
        // `./components/MyH2` only resolves if mdx-components.tsx was
        // physically copied into the shadow root.
        fs::write(
            root.join("components/MyH2.tsx"),
            "export default function MyH2(props) { return props.children; }\n",
        )
        .unwrap();
        // Default export object — the single canonical contract (#616).
        fs::write(
            root.join("mdx-components.tsx"),
            "import MyH2 from \"./components/MyH2\";\nexport default { h2: MyH2 };\n",
        )
        .unwrap();

        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .expect("workspace root from CARGO_MANIFEST_DIR");
        let workspace_node_modules = workspace_root.join("node_modules");
        let zfb_pkg_node_modules = workspace_root.join("packages/zfb/node_modules");
        let nm_dir = if zfb_pkg_node_modules.join("@takazudo/zfb-runtime").exists() {
            Some(zfb_pkg_node_modules)
        } else if workspace_node_modules
            .join("@takazudo/zfb-runtime")
            .exists()
        {
            Some(workspace_node_modules)
        } else {
            eprintln!(
                "[mdx_components_file_resolves_through_esbuild] @takazudo/zfb-runtime not found; skipping (run pnpm install first)"
            );
            return;
        };

        let input = BundlerInput {
            esbuild_binary: Some(bin),
            external: vec!["preact".into()],
            node_modules_dir: nm_dir,
            // The override file is discovered at the project root.
            mdx_components_file: Some(root.join("mdx-components.tsx")),
            ..BundlerInput::for_project(
                root.clone(),
                Framework::Preact,
                BundleMode::Production,
                root.join("dist"),
                None,
            )
        };

        // The bundle must succeed: both `import "./mdx-components.tsx"` and
        // its transitive `import "./components/MyH2"` resolved in-shadow.
        let out =
            bundle(input).expect("real esbuild bundle with mdx-components.tsx should succeed");
        let body = fs::read_to_string(&out.bundle_path).unwrap();
        // The installer must be present in the final bundle.
        assert!(
            body.contains("mdxComponents"),
            "bundled output should install the mdxComponents slot"
        );
    }

    #[test]
    fn missing_esbuild_binary_returns_actionable_error() {
        // When neither an explicit override nor the env var nor the
        // default slot is present, the bundler must error with a
        // pointer to every escape hatch: `ZFB_ESBUILD_BIN`, the
        // release-tarball slot, AND (for out-of-repo embedders) the
        // `zfb.config.json` path that needs no esbuild (#1040). This
        // keeps both workspace operators and library embedders unstuck.
        //
        // Drive the env path via an injected getter and the slot path
        // via `slot_override` so the test does not mutate `std::env`
        // and does not chdir — both are `unsafe` / racy under a
        // multi-threaded test runner.
        let tmp = tempfile::tempdir().unwrap();
        let missing_slot = tmp.path().join("crates/zfb/binaries/esbuild/esbuild");

        let err = resolve_esbuild_binary_with_env(
            None,
            |_| None,
            None::<fn() -> Option<(tempfile::TempDir, PathBuf)>>,
            Some(&missing_slot),
        )
        .unwrap_err();
        let msg = format!("{err}");

        assert!(msg.contains("ZFB_ESBUILD_BIN"), "msg: {msg}");
        // This substring comes from the interpolated slot path
        // (`slot.display()` = the `DEFAULT_ESBUILD_SLOT`-shaped path), not
        // the error prose — the prose no longer names the workspace slot.
        assert!(msg.contains("crates/zfb/binaries/esbuild"), "msg: {msg}");
        assert!(msg.contains("zfb.config.json"), "msg: {msg}");
    }

    #[test]
    fn explicit_missing_binary_is_reported() {
        let err = resolve_esbuild_binary(Some(Path::new(
            "/nonexistent/zfb-bundler-please-do-not-create",
        )))
        .unwrap_err();
        assert!(format!("{err}").contains("not found at explicit path"));
    }

    // -----------------------------------------------------------------------
    // bracket_to_hono tests — verify FS bracket notation → Hono colon syntax.
    // -----------------------------------------------------------------------

    #[test]
    fn bracket_to_hono_converts_all_segment_types() {
        // Root route: no segments.
        assert_eq!(bracket_to_hono("/"), "/");
        // Static only.
        assert_eq!(bracket_to_hono("/about"), "/about");
        assert_eq!(bracket_to_hono("/blog"), "/blog");
        // Single dynamic segment.
        assert_eq!(bracket_to_hono("/blog/[slug]"), "/blog/:slug");
        // Nested dynamic segments.
        assert_eq!(bracket_to_hono("/[lang]/[slug]"), "/:lang/:slug");
        // Pagination (mixed static + dynamic).
        assert_eq!(bracket_to_hono("/blog/page/[page]"), "/blog/page/:page");
        // Catchall (spread) segment.
        assert_eq!(bracket_to_hono("/docs/[...slug]"), "/docs/:slug{.+}");
        // Fully dynamic catchall.
        assert_eq!(bracket_to_hono("/[...rest]"), "/:rest{.+}");
        // Optional catchall (zero or more segments).
        assert_eq!(bracket_to_hono("/docs/[[...slug]]"), "/docs/:slug{.+}?");
        // Fully dynamic optional catchall.
        assert_eq!(bracket_to_hono("/[[...rest]]"), "/:rest{.+}?");
    }

    #[test]
    fn optional_catchall_route_key_round_trips_with_router_template() {
        // The worker registers `bracket_to_hono(derive_route(file))` while
        // the render pipeline keys on `zfb_router::Route::template()`. The
        // two strings must stay bit-identical or the `pagesByRoute` /
        // `__paths__` lookup 404s silently. Pin the round trip for the
        // optional catchall form.
        let route = derive_route(Path::new("docs/[[...slug]].tsx")).expect("derive");
        assert_eq!(route, "/docs/[[...slug]]");
        assert_eq!(bracket_to_hono(&route), "/docs/:slug{.+}?");

        let scanned = {
            let tmp = tempfile::TempDir::new().unwrap();
            let docs = tmp.path().join("docs");
            fs::create_dir_all(&docs).unwrap();
            fs::write(
                docs.join("[[...slug]].tsx"),
                "export default function P() { return null; }\n",
            )
            .unwrap();
            zfb_router::scan_pages(tmp.path()).expect("scan")
        };
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].template(), bracket_to_hono(&route));
    }

    #[test]
    fn route_sort_key_orders_by_specificity() {
        use std::collections::BTreeMap;
        use tempfile::TempDir;

        // Build a minimal pages tree with all 7 route types from the
        // routing-rendering fixture and verify the Hono registration
        // order matches the expected specificity ordering.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let pages = root.join("pages");
        for d in [
            "pages",
            "pages/blog",
            "pages/blog/page",
            "pages/docs",
            "pages/[lang]",
        ] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        let stub = "export default function P() { return null; }\n";
        for f in [
            "pages/index.tsx",
            "pages/about.tsx",
            "pages/blog/index.tsx",
            "pages/blog/[slug].tsx",
            "pages/blog/page/[page].tsx",
            "pages/[lang]/[slug].tsx",
            "pages/docs/[...slug].tsx",
        ] {
            fs::write(root.join(f), stub).unwrap();
        }
        let mut routes = Vec::new();
        // dest must be named "pages" for is_pages_dir detection in materialise_shadow
        let shadow_pages_dest = root.join("shadow").join("pages");
        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(&root, &exclude);
        materialise_shadow(
            &pages,
            &shadow_pages_dest,
            &mut routes,
            &ctx,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();

        // Map route → registration index.
        let order: BTreeMap<&str, usize> = routes
            .iter()
            .enumerate()
            .map(|(i, r)| (r.route.as_str(), i))
            .collect();

        let idx = |r: &str| *order.get(r).unwrap_or_else(|| panic!("missing route {r}"));

        // Rules derived from specificity sort:
        //   more static segments → earlier
        //   fewer dynamic segments → earlier
        //   catchall → after plain dynamic

        // /blog/page/[page] has 2 static segs → comes before all 1-static routes
        assert!(
            idx("/blog/page/[page]") < idx("/blog/[slug]"),
            "/blog/page/[page] should be before /blog/[slug]"
        );
        // /blog/[slug] (1 static) before /[lang]/[slug] (0 static)
        assert!(
            idx("/blog/[slug]") < idx("/[lang]/[slug]"),
            "/blog/[slug] should be before /[lang]/[slug]"
        );
        // /docs/[...slug] (1 static, catchall) before /[lang]/[slug] (0 static)
        assert!(
            idx("/docs/[...slug]") < idx("/[lang]/[slug]"),
            "/docs/[...slug] should be before /[lang]/[slug]"
        );
    }

    #[test]
    fn route_sort_catchall_after_plain_dynamic_at_equal_static_depth() {
        use std::collections::BTreeMap;
        use tempfile::TempDir;

        // Verify that a plain dynamic segment sorts before a catchall at the
        // same static depth — i.e. /docs/[id] before /docs/[...slug].
        // This is the invariant documented in route_sort_key ("Catchall (rest)
        // segments always sort after plain dynamic ones").
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let pages = root.join("pages");
        for d in ["pages", "pages/docs"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        let stub = "export default function P() { return null; }\n";
        for f in [
            "pages/docs/[id].tsx",
            "pages/docs/[...slug].tsx",
            "pages/manual/about.tsx",
            "pages/manual/[id].tsx",
            "pages/manual/[[...slug]].tsx",
            "pages/api/[version]/[page].tsx",
            "pages/api/[...rest].tsx",
            "pages/kb/[version]/[page].tsx",
            "pages/kb/[[...rest]].tsx",
        ] {
            let abs = root.join(f);
            fs::create_dir_all(abs.parent().unwrap()).unwrap();
            fs::write(abs, stub).unwrap();
        }
        let mut routes = Vec::new();
        let shadow_pages_dest = root.join("shadow").join("pages");
        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(&root, &exclude);
        materialise_shadow(
            &pages,
            &shadow_pages_dest,
            &mut routes,
            &ctx,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();

        let order: BTreeMap<&str, usize> = routes
            .iter()
            .enumerate()
            .map(|(i, r)| (r.route.as_str(), i))
            .collect();

        let idx = |r: &str| *order.get(r).unwrap_or_else(|| panic!("missing route {r}"));

        // Plain dynamic must come before catchall at equal static depth.
        assert!(
            idx("/docs/[id]") < idx("/docs/[...slug]"),
            "/docs/[id] should be registered before /docs/[...slug]"
        );

        // The optional catchall sorts with the catchall bucket — after a
        // static sibling and after a plain dynamic at equal static depth.
        assert!(
            idx("/manual/about") < idx("/manual/[[...slug]]"),
            "/manual/about should be registered before /manual/[[...slug]]"
        );
        assert!(
            idx("/manual/[id]") < idx("/manual/[[...slug]]"),
            "/manual/[id] should be registered before /manual/[[...slug]]"
        );

        // A catchall must register AFTER a deeper dynamic descendant at the
        // same prefix — Hono dispatches in registration order, so the old
        // aggregate-count key let `/api/[...rest]` steal `/api/v1/intro`
        // from `/api/[version]/[page]` (probed on Hono 4.12.x). Pin both
        // the required and the optional form.
        assert!(
            idx("/api/[version]/[page]") < idx("/api/[...rest]"),
            "/api/[version]/[page] should be registered before /api/[...rest]"
        );
        assert!(
            idx("/kb/[version]/[page]") < idx("/kb/[[...rest]]"),
            "/kb/[version]/[page] should be registered before /kb/[[...rest]]"
        );
    }

    #[test]
    fn esbuild_loader_args_neutralise_css_imports() {
        // S5 contract — `.css` imports inside JS modules must be
        // neutralised at the loader level so the Worker bundle does not
        // carry user CSS bytes alongside the externally-shipped
        // `dist/assets/styles-<hash>.css` produced by
        // `ProductionAssetPipeline`. The wrong fix here (the rejected
        // `--external:*.css` alternative) would leave runtime `import`
        // statements that workerd cannot resolve, crashing the Worker
        // at module load. This test locks in `loader=empty` as the
        // chosen mechanism.
        assert!(
            ESBUILD_LOADER_ARGS.contains(&"--loader:.css=empty"),
            "Worker bundle must use `--loader:.css=empty` to drop user CSS \
             imports at compile time; got: {:?}",
            ESBUILD_LOADER_ARGS,
        );

        // Defensive: nothing in the loader list should ever be
        // `--external:*.css`. The rejected alternative is documented in
        // the constant's doc comment; this guard turns the rejection
        // into a compile-time invariant.
        assert!(
            !ESBUILD_LOADER_ARGS
                .iter()
                .any(|a| a.contains("external:") && a.contains(".css")),
            "Worker bundle must NOT mark .css as external — esbuild can \
             leave runtime `import` statements workerd cannot resolve. \
             Use `--loader:.css=empty` instead. Got: {:?}",
            ESBUILD_LOADER_ARGS,
        );

        // Existing .mdx contract preserved — same list, no regression.
        assert!(
            ESBUILD_LOADER_ARGS.contains(&"--loader:.mdx=jsx"),
            "Worker bundle must keep `--loader:.mdx=jsx` so MDX modules \
             continue to be parsed as JSX; got: {:?}",
            ESBUILD_LOADER_ARGS,
        );

        assert!(
            ESBUILD_LOADER_ARGS.contains(&"--loader:.wasm=copy"),
            "Worker bundle must use `--loader:.wasm=copy` so deployable Wasm \
             imports stay relative to the emitted bundle; got: {:?}",
            ESBUILD_LOADER_ARGS,
        );
        assert!(
            !ESBUILD_LOADER_ARGS
                .iter()
                .any(|arg| arg.contains("external:") && arg.contains(".wasm")),
            "Worker bundle must NOT mark .wasm external because that leaves a \
             shadow-relative runtime import. Use `--loader:.wasm=copy`; got: {:?}",
            ESBUILD_LOADER_ARGS,
        );
    }

    #[test]
    fn configured_loader_args_append_after_reserved_loaders_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let mut input = make_minimal_input(&tmp);
        input.extra_loader_args = vec![
            "--loader:.bin=binary".to_string(),
            "--loader:.txt=text".to_string(),
        ];

        let args: Vec<&str> = esbuild_loader_args(&input).collect();
        assert_eq!(&args[..ESBUILD_LOADER_ARGS.len()], ESBUILD_LOADER_ARGS);
        assert_eq!(
            &args[ESBUILD_LOADER_ARGS.len()..],
            ["--loader:.bin=binary", "--loader:.txt=text"]
        );
    }

    #[test]
    fn wasm_asset_manifest_is_sorted_deduped_and_bundle_relative() {
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path().join("shadow");
        let outdir = tmp.path().join("dist");
        fs::create_dir_all(&shadow).unwrap();
        fs::create_dir_all(&outdir).unwrap();
        fs::write(outdir.join("a-111.wasm"), b"a").unwrap();
        fs::write(outdir.join("z-222.wasm"), b"z").unwrap();
        let bundle = outdir.join("bundle.mjs");
        fs::write(&bundle, "import wasm from \"./a-111.wasm\";\n").unwrap();

        let outputs = BTreeMap::from([
            ("../dist/z-222.wasm".to_string(), serde_json::json!({})),
            ("../dist/a-111.wasm".to_string(), serde_json::json!({})),
            (
                outdir.join("a-111.wasm").to_string_lossy().into_owned(),
                serde_json::json!({}),
            ),
        ]);
        let metafile = serde_json::json!({ "outputs": outputs });
        let metafile_bytes = serde_json::to_vec(&metafile).unwrap();
        let assets = emitted_wasm_assets_from_metafile(
            &shadow.join(".zfb-metafile.json"),
            Some(&metafile_bytes),
            &shadow,
            &outdir,
            &bundle,
        )
        .unwrap();

        assert_eq!(
            assets,
            vec![PathBuf::from("a-111.wasm"), PathBuf::from("z-222.wasm")]
        );
    }

    #[test]
    fn wasm_asset_manifest_fails_closed_for_missing_or_malformed_metafile() {
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path().join("shadow");
        let outdir = tmp.path().join("dist");
        fs::create_dir_all(&shadow).unwrap();
        fs::create_dir_all(&outdir).unwrap();
        let bundle = outdir.join("bundle.mjs");
        fs::write(&bundle, "import wasm from \"./x-123.wasm\";\n").unwrap();
        let metafile_path = shadow.join(".zfb-metafile.json");

        let missing =
            emitted_wasm_assets_from_metafile(&metafile_path, None, &shadow, &outdir, &bundle)
                .expect_err("a Wasm-importing bundle needs a metafile");
        assert!(missing.to_string().contains("wasm asset manifest"));

        let malformed = emitted_wasm_assets_from_metafile(
            &metafile_path,
            Some(b"not json"),
            &shadow,
            &outdir,
            &bundle,
        )
        .expect_err("a malformed metafile must fail a Wasm-importing bundle");
        assert!(malformed.to_string().contains("wasm asset manifest"));
    }

    #[test]
    fn wasm_asset_manifest_rejects_missing_and_out_of_bundle_outputs() {
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path().join("shadow");
        let outdir = tmp.path().join("dist");
        fs::create_dir_all(&shadow).unwrap();
        fs::create_dir_all(&outdir).unwrap();
        let bundle = outdir.join("bundle.mjs");
        fs::write(&bundle, "import wasm from \"./x-123.wasm\";\n").unwrap();
        let metafile_path = shadow.join(".zfb-metafile.json");

        let missing = serde_json::json!({ "outputs": { "../dist/x-123.wasm": {} } });
        let missing_bytes = serde_json::to_vec(&missing).unwrap();
        let missing = emitted_wasm_assets_from_metafile(
            &metafile_path,
            Some(&missing_bytes),
            &shadow,
            &outdir,
            &bundle,
        )
        .expect_err("a manifest entry must name an existing asset");
        assert!(missing.to_string().contains("does not exist"));

        let outside = tmp.path().join("outside.wasm");
        fs::write(&outside, b"outside").unwrap();
        let outside_outputs = BTreeMap::from([(
            outside.to_string_lossy().into_owned(),
            serde_json::json!({}),
        )]);
        let outside_meta = serde_json::json!({ "outputs": outside_outputs });
        let outside_bytes = serde_json::to_vec(&outside_meta).unwrap();
        let outside = emitted_wasm_assets_from_metafile(
            &metafile_path,
            Some(&outside_bytes),
            &shadow,
            &outdir,
            &bundle,
        )
        .expect_err("a manifest entry must stay under the bundle outdir");
        assert!(outside
            .to_string()
            .contains("escapes bundle output directory"));
    }

    #[test]
    fn malformed_metafile_stays_nonfatal_for_wasm_free_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path().join("shadow");
        let outdir = tmp.path().join("dist");
        fs::create_dir_all(&shadow).unwrap();
        fs::create_dir_all(&outdir).unwrap();
        let bundle = outdir.join("bundle.mjs");
        fs::write(&bundle, "export default {};\n").unwrap();

        let assets = emitted_wasm_assets_from_metafile(
            &shadow.join(".zfb-metafile.json"),
            Some(b"not json"),
            &shadow,
            &outdir,
            &bundle,
        )
        .expect("Wasm-free bundles retain best-effort metafile behavior");
        assert!(assets.is_empty());
    }

    #[test]
    fn wasm_asset_manifest_ignores_wasm_text_outside_static_esm_imports() {
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path().join("shadow");
        let outdir = tmp.path().join("dist");
        fs::create_dir_all(&shadow).unwrap();
        fs::create_dir_all(&outdir).unwrap();
        let bundle = outdir.join("bundle.mjs");
        fs::write(
            &bundle,
            r#"
                // import wasm from "./comment-only.wasm";
                const message = "./string-only.wasm";
                const template = `import wasm from "./template-only.wasm";`;
            "#,
        )
        .unwrap();

        let assets = emitted_wasm_assets_from_metafile(
            &shadow.join(".zfb-metafile.json"),
            Some(b"not json"),
            &shadow,
            &outdir,
            &bundle,
        )
        .expect("non-import Wasm text must not require a metafile");
        assert!(assets.is_empty());
    }

    #[test]
    fn bundle_mode_define_args_pin_dev_and_production_values() {
        assert_eq!(
            bundle_mode_define_args(BundleMode::Development),
            [
                "--define:import.meta.env.PROD=false".to_string(),
                "--define:import.meta.env.DEV=true".to_string(),
                "--define:process.env.NODE_ENV=\"development\"".to_string(),
            ]
        );
        assert_eq!(
            bundle_mode_define_args(BundleMode::Production),
            [
                "--define:import.meta.env.PROD=true".to_string(),
                "--define:import.meta.env.DEV=false".to_string(),
                "--define:process.env.NODE_ENV=\"production\"".to_string(),
            ]
        );
    }

    #[test]
    fn operator_define_args_are_sorted_raw_and_separate_from_public_env() {
        let define_vars = BTreeMap::from([
            ("__ZETA__".to_string(), "{ enabled: true }".to_string()),
            ("__ALPHA__".to_string(), "\"raw string\"".to_string()),
        ]);
        assert_eq!(
            operator_define_args(&define_vars),
            vec![
                "--define:__ALPHA__=\"raw string\"".to_string(),
                "--define:__ZETA__={ enabled: true }".to_string(),
            ]
        );
    }

    #[test]
    fn public_env_define_args_defer_to_exact_operator_expression() {
        let public_env_vars = HashMap::from([
            ("PUBLIC_ZETA".to_string(), "zeta".to_string()),
            ("PUBLIC_COLLISION".to_string(), "generated".to_string()),
            ("SECRET_VALUE".to_string(), "never".to_string()),
        ]);
        let operator_define_vars = BTreeMap::from([(
            "process.env.PUBLIC_COLLISION".to_string(),
            "\"explicit\"".to_string(),
        )]);

        assert_eq!(
            public_env_define_args(&public_env_vars, &operator_define_vars),
            vec![
                "--define:import.meta.env.PUBLIC_COLLISION=\"generated\"".to_string(),
                "--define:process.env.PUBLIC_ZETA=\"zeta\"".to_string(),
                "--define:import.meta.env.PUBLIC_ZETA=\"zeta\"".to_string(),
            ]
        );
    }

    // -----------------------------------------------------------------------
    // Plugin alias / virtual-module wiring tests (#268)
    // -----------------------------------------------------------------------

    /// Regression test: a plugin alias `@/foo` → absolute path is consumable
    /// from a **shared SSR-only module** (i.e. imported by a page but not an
    /// island). This locks in the source-issue requirement: plugin aliases
    /// registered via `setup()` must resolve in pages / layouts / shared
    /// SSR-only code, not just in island bundles.
    ///
    /// Uses `mock_subprocess_output` so no real esbuild binary is required.
    /// The test verifies that:
    ///  1. `BundlerInput::plugin_alias_entries` is accepted without error.
    ///  2. `BundlerInput::plugin_virtual_modules` is accepted without error.
    ///  3. The bundler does NOT reject or drop the fields silently
    ///     (the mock-mode path exercises the struct-construction code path;
    ///     the real esbuild emission is covered by the end-to-end bundler
    ///     integration tests in `crates/zfb-build/tests/bundler_integration.rs`
    ///     which run when `ZFB_ESBUILD_BIN` is available).
    #[test]
    fn plugin_alias_and_virtual_module_fields_are_accepted_in_bundler_input() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        // Minimal project layout.
        fs::create_dir_all(root.join("pages")).unwrap();
        fs::create_dir_all(root.join("content")).unwrap();
        fs::create_dir_all(root.join("components")).unwrap();
        fs::create_dir_all(root.join("layouts")).unwrap();
        // Shared SSR-only module that would consume a plugin alias — the page
        // imports this module so the alias is in the non-island (main bundler)
        // dependency graph.
        fs::create_dir_all(root.join("components")).unwrap();
        fs::write(
            root.join("components/shared.tsx"),
            // In real usage this would `import Foo from '@/foo'`. We don't
            // need real resolution in mock mode — what we're testing is that
            // the fields wire through without compilation or runtime errors.
            "export const shared = 'hello';\n",
        )
        .unwrap();
        fs::write(
            root.join("pages/index.tsx"),
            "import { shared } from '../components/shared';\n\
             export default function Home() { return null; }\n",
        )
        .unwrap();

        let input = BundlerInput {
            // Plugin alias: `@/foo` → some absolute path (no real file needed
            // because mock_subprocess_output bypasses esbuild).
            plugin_alias_entries: vec![(
                "@/foo".to_string(),
                root.join("src/foo.tsx").to_string_lossy().into_owned(),
            )],
            // Virtual module: `virtual:meta` with inline source.
            plugin_virtual_modules: vec![(
                "virtual:meta".to_string(),
                "export const version = '1.0.0';\n".to_string(),
            )],
            ..make_minimal_input(&tmp)
        };

        // With mock_subprocess_output the bundler writes the mock string to
        // dist/bundle.mjs without invoking esbuild, so the test succeeds
        // without a real binary and without needing `@/foo` to resolve.
        let out = bundle(input).expect(
            "bundler must accept plugin_alias_entries and plugin_virtual_modules \
             without error (#268)",
        );
        assert!(
            out.bundle_path.exists(),
            "bundle path must exist even in mock mode"
        );
    }

    /// Verify that `BundlerInput::plugin_alias_entries` and
    /// `plugin_virtual_modules` default to empty vecs in `for_project()` so
    /// existing call sites that don't supply plugin data keep working
    /// byte-for-byte identical to the pre-#268 build.
    #[test]
    fn for_project_defaults_plugin_fields_to_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let input = BundlerInput::for_project(
            root.clone(),
            zfb_render::adapters::Framework::Preact,
            BundleMode::Production,
            root.join("dist"),
            None,
        );
        assert!(
            input.plugin_alias_entries.is_empty(),
            "plugin_alias_entries must default to empty"
        );
        assert!(
            input.plugin_virtual_modules.is_empty(),
            "plugin_virtual_modules must default to empty"
        );
    }

    // ---- .md / .html derive_route (Sub 406) --------------------------------

    #[test]
    fn derive_route_accepts_md_and_html() {
        assert_eq!(
            derive_route(Path::new("about.md")).as_deref(),
            Some("/about"),
            "about.md must derive /about"
        );
        assert_eq!(
            derive_route(Path::new("index.html")).as_deref(),
            Some("/"),
            "index.html must derive /"
        );
    }

    // ---- route collision tests for new extensions (Sub 406) ----------------

    /// Helper: run materialise_shadow on a pages dir and return the error
    /// message when a collision is detected. Panics if no error is returned.
    fn assert_route_collision(pages_dir: &Path, root: &Path) -> String {
        let shadow_pages_dest = root.join("shadow_coll").join("pages");
        let mut routes = Vec::new();
        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(root, &exclude);
        let err = materialise_shadow(
            pages_dir,
            &shadow_pages_dest,
            &mut routes,
            &ctx,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .expect_err("expected a route collision error");
        err.to_string()
    }

    #[test]
    fn collision_tsx_and_md_same_stem() {
        // pages/about.tsx + pages/about.md → both map to /about → error.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let pages = root.join("pages");
        fs::create_dir_all(&pages).unwrap();
        let stub = "export default function P() { return null; }\n";
        fs::write(pages.join("about.tsx"), stub).unwrap();
        fs::write(pages.join("about.md"), "# about\n").unwrap();

        let msg = assert_route_collision(&pages, root);
        assert!(
            msg.contains("route collision"),
            "expected route collision message; got: {msg}"
        );
    }

    #[test]
    fn collision_tsx_and_html_same_stem() {
        // pages/contact.tsx + pages/contact.html → both map to /contact → error.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let pages = root.join("pages");
        fs::create_dir_all(&pages).unwrap();
        let stub = "export default function P() { return null; }\n";
        fs::write(pages.join("contact.tsx"), stub).unwrap();
        fs::write(pages.join("contact.html"), "<p>contact</p>\n").unwrap();

        let msg = assert_route_collision(&pages, root);
        assert!(
            msg.contains("route collision"),
            "expected route collision message; got: {msg}"
        );
    }

    #[test]
    fn collision_md_and_html_same_stem() {
        // pages/page.md + pages/page.html → both map to /page → error.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let pages = root.join("pages");
        fs::create_dir_all(&pages).unwrap();
        fs::write(pages.join("page.md"), "# page\n").unwrap();
        fs::write(pages.join("page.html"), "<p>page</p>\n").unwrap();

        let msg = assert_route_collision(&pages, root);
        assert!(
            msg.contains("route collision"),
            "expected route collision message; got: {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // render_md_page_shell unit tests (#408)
    // -----------------------------------------------------------------------

    #[test]
    fn render_md_page_shell_uses_title_from_frontmatter() {
        let fm = serde_json::json!({"title": "About Us"});
        let shell = render_md_page_shell(&fm, "about", "./_zfb_md_body_about.jsx");
        // Title const must be the frontmatter value, not the slug.
        assert!(
            shell.contains("const __title = \"About Us\";"),
            "expected title from frontmatter; got:\n{shell}"
        );
        // Import must reference the body file with relative path.
        assert!(
            shell.contains("from \"./_zfb_md_body_about.jsx\""),
            "expected body import; got:\n{shell}"
        );
        // Full document structure.
        assert!(
            shell.contains("<html lang={__lang}>"),
            "html element with lang; got:\n{shell}"
        );
        assert!(
            shell.contains("<meta charSet=\"utf-8\" />"),
            "charset meta; got:\n{shell}"
        );
        assert!(
            shell.contains("<title>{__title}</title>"),
            "title element; got:\n{shell}"
        );
        assert!(
            shell.contains("<MdBody />"),
            "body component; got:\n{shell}"
        );
    }

    #[test]
    fn render_md_page_shell_falls_back_to_slug_when_no_title() {
        let fm = serde_json::json!({});
        let shell = render_md_page_shell(&fm, "about", "./_zfb_md_body_about.jsx");
        assert!(
            shell.contains("const __title = \"about\";"),
            "expected slug fallback; got:\n{shell}"
        );
    }

    #[test]
    fn render_md_page_shell_falls_back_to_index_for_root() {
        // pages/index.md → slug_fallback is "index"
        let fm = serde_json::json!({});
        let shell = render_md_page_shell(&fm, "index", "./_zfb_md_body_index.jsx");
        assert!(
            shell.contains("const __title = \"index\";"),
            "root page slug fallback should be \"index\"; got:\n{shell}"
        );
    }

    #[test]
    fn render_md_page_shell_uses_lang_from_frontmatter() {
        let fm = serde_json::json!({"title": "Page", "lang": "ja"});
        let shell = render_md_page_shell(&fm, "page", "./_zfb_md_body_page.jsx");
        assert!(
            shell.contains("const __lang = \"ja\";"),
            "expected lang from frontmatter; got:\n{shell}"
        );
    }

    #[test]
    fn render_md_page_shell_defaults_lang_to_en() {
        let fm = serde_json::json!({"title": "Page"});
        let shell = render_md_page_shell(&fm, "page", "./_zfb_md_body_page.jsx");
        assert!(
            shell.contains("const __lang = \"en\";"),
            "expected default lang \"en\"; got:\n{shell}"
        );
    }

    #[test]
    fn render_md_page_shell_ignores_non_string_title() {
        // Non-string title falls back to slug, not to garbage.
        let fm = serde_json::json!({"title": 42});
        let shell = render_md_page_shell(&fm, "slug-fallback", "./_zfb_md_body_x.jsx");
        assert!(
            shell.contains("const __title = \"slug-fallback\";"),
            "non-string title must fall back to slug; got:\n{shell}"
        );
    }

    #[test]
    fn render_md_page_shell_title_is_json_string_literal() {
        // The title value is emitted as a JSON string literal assigned to
        // a const and then referenced via a JSX expression `{__title}`.
        // This is safe for any string content — the JSX renderer handles
        // escaping at render time. The test verifies the const assignment
        // shape (not raw HTML injection).
        let fm = serde_json::json!({"title": "A & <B>"});
        let shell = render_md_page_shell(&fm, "page", "./_zfb_md_body_page.jsx");
        // The title must be assigned to a const (JSON string literal form).
        // json_str produces a valid JSON string; the const is referenced
        // via {__title} in JSX so the renderer handles escaping at render time.
        assert!(
            shell.contains("const __title = "),
            "title must be assigned to a const; got:\n{shell}"
        );
        // The value must appear inside the const assignment as a string literal.
        // We don't assert the exact escaping since json_str may or may not
        // escape & / < (both are valid in JSON).
        assert!(
            shell.contains("<title>{__title}</title>"),
            "title must be referenced via JSX expression; got:\n{shell}"
        );
    }

    // -----------------------------------------------------------------------
    // materialise_shadow with .md pages (#408)
    // -----------------------------------------------------------------------

    #[test]
    fn materialise_shadow_compiles_md_pages_and_emits_shell() {
        // A .md file in the pages/ root must be compiled and wrapped in
        // the HTML shell; a `_zfb_md_body_<stem>.jsx` sibling holds the
        // compiled body, and the original .md shadow path holds the shell.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("pages");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            src.join("about.md"),
            "---\ntitle: About\nlang: fr\n---\n\n# Hello\n\nA paragraph.\n",
        )
        .unwrap();

        let dest = tmp.path().join("shadow").join("pages");
        let mut routes = Vec::new();
        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(tmp.path(), &exclude);
        materialise_shadow(
            &src,
            &dest,
            &mut routes,
            &ctx,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();

        // Route is detected for /about.
        assert_eq!(routes.len(), 1, "expected one route; got {routes:?}");
        assert_eq!(routes[0].route, "/about");

        // Shadow has both the shell and the body file.
        let shell_path = dest.join("about.md");
        let body_path = dest.join("_zfb_md_body_about.jsx");
        assert!(shell_path.is_file(), "shell must exist at about.md");
        assert!(
            body_path.is_file(),
            "body must exist at _zfb_md_body_about.jsx"
        );

        // Shell is a TSX module wrapping the body.
        let shell = fs::read_to_string(&shell_path).unwrap();
        assert!(
            shell.contains("const __title = \"About\";"),
            "shell must pick up title from frontmatter; got:\n{shell}"
        );
        assert!(
            shell.contains("const __lang = \"fr\";"),
            "shell must pick up lang from frontmatter; got:\n{shell}"
        );
        assert!(
            shell.contains("from \"./_zfb_md_body_about.jsx\""),
            "shell must import the body file with relative path; got:\n{shell}"
        );
        assert!(
            shell.contains("<MdBody />"),
            "shell must render <MdBody />; got:\n{shell}"
        );

        // Body contains compiled MDX output.
        let body = fs::read_to_string(&body_path).unwrap();
        assert!(
            body.contains("_createMdxContent"),
            "body must be compiled JSX; got:\n{body}"
        );

        // derive_route must skip the _-prefixed body file (no route for it).
        assert!(
            !routes
                .iter()
                .any(|r| r.source_path.to_string_lossy().contains("_zfb_md_body")),
            "body file must not produce a route; routes: {routes:?}"
        );
    }

    #[test]
    fn materialise_shadow_md_page_slug_fallback_for_index() {
        // pages/index.md → route "/" → slug fallback "index" → title "index".
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("pages");
        fs::create_dir_all(&src).unwrap();
        // No frontmatter title.
        fs::write(src.join("index.md"), "# Welcome\n").unwrap();

        let dest = tmp.path().join("shadow").join("pages");
        let mut routes = Vec::new();
        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(tmp.path(), &exclude);
        materialise_shadow(
            &src,
            &dest,
            &mut routes,
            &ctx,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();

        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route, "/");

        let shell = fs::read_to_string(dest.join("index.md")).unwrap();
        assert!(
            shell.contains("const __title = \"index\";"),
            "slug fallback should be 'index'; got:\n{shell}"
        );
    }

    #[test]
    fn materialise_shadow_md_in_content_dir_is_not_wrapped_in_shell() {
        // .md files in a non-pages dir (content/) must be compiled as
        // collection entries (no shell wrapper) — the pages shell only
        // applies when is_pages_dir == true.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("content");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("post.md"), "---\ntitle: Post\n---\n\n# Post\n").unwrap();

        let dest = tmp.path().join("shadow").join("content");
        let mut routes = Vec::new();
        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(tmp.path(), &exclude);
        materialise_shadow(
            &src,
            &dest,
            &mut routes,
            &ctx,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();

        // In non-pages dir: post.md copied verbatim (not compiled into shell).
        let shadow_file = dest.join("post.md");
        assert!(shadow_file.is_file(), "post.md must exist in shadow");
        let content = fs::read_to_string(&shadow_file).unwrap();
        // Verbatim copy: still has the frontmatter YAML, not a shell module.
        assert!(
            !content.contains("const __title ="),
            "content dir .md must NOT be wrapped in shell; got:\n{content}"
        );
        // No _-prefixed body file should exist.
        assert!(
            !dest.join("_zfb_md_body_post.jsx").exists(),
            "no body file expected for content dir .md"
        );
        // No route collected for content dir.
        assert!(
            routes.is_empty(),
            "content dir must not produce routes; got {routes:?}"
        );
    }

    #[test]
    fn materialise_shadow_md_page_relative_link_produces_anchor_in_body() {
        // A relative markdown link `[link](./other)` in a pages .md file
        // must compile into an <a> element in the body JSX. We assert on
        // tag structure (an <a> is emitted, link text is present, href is
        // present) rather than guessing the exact resolved URL.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("pages");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            src.join("about.md"),
            "---\ntitle: About\n---\n\nSee [other page](./other).\n",
        )
        .unwrap();

        let dest = tmp.path().join("shadow").join("pages");
        let mut routes = Vec::new();
        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(tmp.path(), &exclude);
        materialise_shadow(
            &src,
            &dest,
            &mut routes,
            &ctx,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();

        let body = fs::read_to_string(dest.join("_zfb_md_body_about.jsx")).unwrap();

        // The compiled body must contain an <a> element.
        assert!(
            body.contains("<_components.a"),
            "relative link must produce an <a> element; got:\n{body}"
        );
        // The link text must appear somewhere in the body output.
        assert!(
            body.contains("other page"),
            "link text must be present; got:\n{body}"
        );
        // An href attribute must be present (value is whatever the pipeline emits).
        assert!(
            body.contains("href="),
            "anchor must have an href attribute; got:\n{body}"
        );
    }

    // --- is_pruned_infra_dir / shadow walker prune tests (#432) ---------------

    /// Collect all file paths under `root` relative to `root`, sorted.
    fn collect_dest_files(root: &Path) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for entry in WalkDir::new(root).follow_links(false).sort_by_file_name() {
            let entry = entry.unwrap();
            // Accept both regular files and symlinks to files (non-transformed
            // shadow entries are now symlinks after the symlink_or_copy change).
            let ft = entry.file_type();
            if !ft.is_file() && !ft.is_symlink() {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.push(rel.to_owned());
        }
        out.sort();
        out
    }

    #[test]
    fn is_pruned_infra_dir_named_infra_dirs_not_materialised() {
        // Named infra dirs (node_modules, .git, .next, .turbo, .vercel)
        // must not appear in the shadow destination; a regular sibling file
        // must still be copied. Tests both materialise_shadow (non-pages
        // dest) and materialise_collection walkers.
        for infra_name in &["node_modules", ".git", ".next", ".turbo", ".vercel"] {
            let tmp = tempfile::tempdir().unwrap();
            let src = tmp.path().join("src");
            // Regular sibling file.
            fs::create_dir_all(&src).unwrap();
            fs::write(src.join("real.tsx"), "export default () => null;\n").unwrap();
            // Infra dir with nested file — must be pruned.
            let infra = src.join(infra_name).join("foo");
            fs::create_dir_all(&infra).unwrap();
            fs::write(infra.join("inner.js"), "// should not appear\n").unwrap();

            // --- materialise_shadow ---
            let dest_shadow = tmp.path().join("shadow");
            let exclude = no_bundle_exclude();
            let ctx = default_mat_ctx(tmp.path(), &exclude);
            materialise_shadow(
                &src,
                &dest_shadow,
                &mut Vec::new(),
                &ctx,
                &mut Vec::new(),
                &mut Vec::new(),
                &mut Vec::new(),
                &mut Vec::new(),
            )
            .unwrap();

            let shadow_files = collect_dest_files(&dest_shadow);
            assert!(
                shadow_files.contains(&"real.tsx".to_string()),
                "infra={infra_name}: real.tsx must be present in shadow; got {shadow_files:?}"
            );
            assert!(
                !shadow_files
                    .iter()
                    .any(|f| f.starts_with(infra_name) || f.contains(&format!("/{infra_name}/"))),
                "infra={infra_name}: no file under infra dir must appear in shadow; got {shadow_files:?}"
            );

            // --- materialise_collection ---
            let dest_coll = tmp.path().join("collection");
            materialise_collection(
                &src,
                &dest_coll,
                "test",
                &mut Vec::new(),
                &ctx,
                None,
                None,
                None,
                &mut Vec::new(),
                &mut Vec::new(),
                &mut Vec::new(),
                &mut Vec::new(),
            )
            .unwrap();

            let coll_files = collect_dest_files(&dest_coll);
            assert!(
                coll_files.contains(&"real.tsx".to_string()),
                "infra={infra_name}: real.tsx must be present in collection; got {coll_files:?}"
            );
            assert!(
                !coll_files
                    .iter()
                    .any(|f| f.starts_with(infra_name) || f.contains(&format!("/{infra_name}/"))),
                "infra={infra_name}: no file under infra dir must appear in collection; got {coll_files:?}"
            );
        }
    }

    #[test]
    fn is_pruned_infra_dir_dotdir_at_depth_not_materialised() {
        // A hidden directory nested under a regular subdirectory (.cache,
        // .wrangler, etc.) must not appear in the destination, but the
        // parent regular subdirectory and its non-dot sibling MUST.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        // Regular sibling under subdir.
        fs::create_dir_all(src.join("subdir")).unwrap();
        fs::write(src.join("subdir/real.tsx"), "export default () => null;\n").unwrap();
        // Hidden dir under subdir — must be pruned.
        fs::create_dir_all(src.join("subdir/.cache")).unwrap();
        fs::write(src.join("subdir/.cache/inner.json"), "{}\n").unwrap();

        let dest = tmp.path().join("shadow");
        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(tmp.path(), &exclude);
        materialise_shadow(
            &src,
            &dest,
            &mut Vec::new(),
            &ctx,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();

        let files = collect_dest_files(&dest);
        assert!(
            files.contains(&"subdir/real.tsx".to_string()),
            "subdir/real.tsx must be materialised; got {files:?}"
        );
        assert!(
            !files.iter().any(|f| f.contains(".cache")),
            "subdir/.cache/* must NOT be materialised; got {files:?}"
        );
    }

    #[test]
    fn is_pruned_infra_dir_depth0_root_not_pruned() {
        // Even when the walker root's name starts with '.', depth-0 is never
        // pruned — the helper's `entry.depth() > 0` guard ensures the caller's
        // chosen root is always walked.
        let tmp = tempfile::tempdir().unwrap();
        // Create a src whose name starts with '.'.
        let src = tmp.path().join(".dotroot");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("page.tsx"), "export default () => null;\n").unwrap();

        let dest = tmp.path().join("shadow");
        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(tmp.path(), &exclude);
        materialise_shadow(
            &src,
            &dest,
            &mut Vec::new(),
            &ctx,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();

        let files = collect_dest_files(&dest);
        assert!(
            files.contains(&"page.tsx".to_string()),
            "walker root named '.dotroot' must still be walked; got {files:?}"
        );
    }

    #[test]
    fn is_pruned_infra_dir_sort_and_filter_interaction() {
        // Verify that the filter_entry + sort_by_file_name combination yields
        // lexicographically ordered entries and that node_modules does NOT
        // appear while the sibling "nodes/" dir DOES.
        let tmp = tempfile::tempdir().unwrap();

        // Build a source tree:
        //   aaa.tsx          (plain file — should be first)
        //   node_modules/x   (pruned infra dir — must not appear)
        //   nodes/x.tsx      (regular dir starting with "nodes" — must appear)
        //   zzz.tsx          (plain file — should be last)
        fs::create_dir_all(tmp.path()).unwrap();
        fs::write(tmp.path().join("aaa.tsx"), "// a\n").unwrap();
        fs::create_dir_all(tmp.path().join("node_modules")).unwrap();
        fs::write(tmp.path().join("node_modules/x.js"), "// x\n").unwrap();
        fs::create_dir_all(tmp.path().join("nodes")).unwrap();
        fs::write(tmp.path().join("nodes/x.tsx"), "// nx\n").unwrap();
        fs::write(tmp.path().join("zzz.tsx"), "// z\n").unwrap();

        // Collect walked paths directly using the same pattern as the walkers.
        let walked: Vec<String> = WalkDir::new(tmp.path())
            .follow_links(false)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| !is_pruned_infra_dir(e))
            .filter_map(|e| {
                let e = e.unwrap();
                if !e.file_type().is_file() {
                    return None;
                }
                let rel = e
                    .path()
                    .strip_prefix(tmp.path())
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
                    .to_owned();
                Some(rel)
            })
            .collect();

        assert_eq!(
            walked,
            vec![
                "aaa.tsx".to_string(),
                "nodes/x.tsx".to_string(),
                "zzz.tsx".to_string(),
            ],
            "lexicographic order must be preserved and node_modules must be absent; got {walked:?}"
        );
    }

    // --- enumerate_extra_top_level_dirs tests (#433) --------------------------

    /// Helper: collect just the last path component names from the helper's
    /// return value for readable assertions.
    fn dir_names(paths: Vec<PathBuf>) -> Vec<String> {
        let mut names: Vec<String> = paths
            .into_iter()
            .map(|p| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn enumerate_extra_top_level_dirs_basic_gitignore() {
        // .gitignore containing "worktrees/" causes the worktrees/ dir to be
        // excluded; styles/ (not gitignored) is returned.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        fs::create_dir_all(root.join("styles")).unwrap();
        fs::write(root.join("styles/foo.css"), "/* css */\n").unwrap();
        fs::create_dir_all(root.join("worktrees/sub")).unwrap();
        fs::write(root.join("worktrees/sub/file.txt"), "data\n").unwrap();
        fs::write(root.join(".gitignore"), "worktrees/\n").unwrap();

        let result = enumerate_extra_top_level_dirs(root, &[]);
        let names = dir_names(result);
        assert!(
            names.contains(&"styles".to_string()),
            "styles must be present; got {names:?}"
        );
        assert!(
            !names.contains(&"worktrees".to_string()),
            "worktrees must be excluded by .gitignore; got {names:?}"
        );
    }

    #[test]
    fn enumerate_extra_top_level_dirs_negated_pattern_whole_dir_excluded() {
        // .gitignore: "worktrees/\n!worktrees/keep/"
        // Because max_depth=1, we see only the top-level "worktrees/" entry.
        // The ignore crate applies gitignore rules at the entry level — a
        // negation for a sub-path inside an already-excluded directory does NOT
        // re-include the parent at depth 1. The whole "worktrees/" dir is still
        // excluded. This is intentional and documented: the extra-dirs pass is
        // whole-dir-or-nothing at max_depth=1; consumers needing sub-path
        // granularity should not rely on this pass.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        fs::create_dir_all(root.join("styles")).unwrap();
        fs::write(root.join("styles/foo.css"), "/* css */\n").unwrap();
        fs::create_dir_all(root.join("worktrees/keep")).unwrap();
        fs::write(root.join("worktrees/keep/file.txt"), "data\n").unwrap();
        fs::write(root.join(".gitignore"), "worktrees/\n!worktrees/keep/\n").unwrap();

        let result = enumerate_extra_top_level_dirs(root, &[]);
        let names = dir_names(result);
        assert!(
            names.contains(&"styles".to_string()),
            "styles must be present; got {names:?}"
        );
        // Negation at sub-path depth does NOT re-include the whole worktrees/
        // dir at max_depth=1 — the parent is excluded first.
        assert!(
            !names.contains(&"worktrees".to_string()),
            "worktrees must still be excluded (negation at depth is not preserved); got {names:?}"
        );
    }

    #[test]
    fn enumerate_extra_top_level_dirs_no_git_dir_still_honors_gitignore() {
        // Verifies `require_git(false)`: .gitignore is respected even when
        // there is no .git/ directory, i.e., the consumer isn't a git repo.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // Deliberately no .git/ directory.
        fs::create_dir_all(root.join("styles")).unwrap();
        fs::write(root.join("styles/foo.css"), "/* css */\n").unwrap();
        fs::create_dir_all(root.join("worktrees/sub")).unwrap();
        fs::write(root.join("worktrees/sub/file.txt"), "data\n").unwrap();
        fs::write(root.join(".gitignore"), "worktrees/\n").unwrap();

        let result = enumerate_extra_top_level_dirs(root, &[]);
        let names = dir_names(result);
        assert!(
            names.contains(&"styles".to_string()),
            "styles must be present; got {names:?}"
        );
        assert!(
            !names.contains(&"worktrees".to_string()),
            "worktrees must be excluded by .gitignore even without .git/; got {names:?}"
        );
    }

    #[test]
    fn enumerate_extra_top_level_dirs_skip_list_intersection() {
        // Dirs in the known skip-list (node_modules, dist) are excluded even
        // when they are not in .gitignore.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        fs::create_dir_all(root.join("styles")).unwrap();
        fs::write(root.join("styles/foo.css"), "/* css */\n").unwrap();
        fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        fs::write(root.join("node_modules/pkg/index.js"), "// js\n").unwrap();
        fs::create_dir_all(root.join("dist")).unwrap();
        fs::write(root.join("dist/bundle.js"), "// bundle\n").unwrap();

        let known: &[&str] = &["node_modules", "dist"];
        let result = enumerate_extra_top_level_dirs(root, known);
        let names = dir_names(result);
        assert!(
            names.contains(&"styles".to_string()),
            "styles must be present; got {names:?}"
        );
        assert!(
            !names.contains(&"node_modules".to_string()),
            "node_modules must be excluded by skip-list; got {names:?}"
        );
        assert!(
            !names.contains(&"dist".to_string()),
            "dist must be excluded by skip-list; got {names:?}"
        );
    }

    #[test]
    fn enumerate_extra_top_level_dirs_hidden_dirs_excluded() {
        // Top-level hidden directories (.foo/) are excluded by the starts_with('.')
        // filter (and also by standard_filters — double-covered).
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        fs::create_dir_all(root.join("styles")).unwrap();
        fs::write(root.join("styles/foo.css"), "/* css */\n").unwrap();
        fs::create_dir_all(root.join(".foo")).unwrap();
        fs::write(root.join(".foo/config"), "data\n").unwrap();

        let result = enumerate_extra_top_level_dirs(root, &[]);
        let names = dir_names(result);
        assert!(
            names.contains(&"styles".to_string()),
            "styles must be present; got {names:?}"
        );
        assert!(
            !names.contains(&".foo".to_string()),
            ".foo hidden dir must be excluded; got {names:?}"
        );
    }

    // ── issue #1840 first-party dot-path staging allowlist tests ─────────

    /// The whole point of the allowlist: `.zudo-doc/routes-src` is enumerated
    /// EVEN WHEN gitignored (and hidden), while the generic extra-dirs walker
    /// keeps excluding the dot-dir — the allowlist is additive, not a filter
    /// loosening.
    #[test]
    fn first_party_staging_dirs_included_even_when_gitignored() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".zudo-doc/routes-src")).unwrap();
        fs::write(
            root.join(".zudo-doc/routes-src/generated.tsx"),
            "export default () => null;\n",
        )
        .unwrap();
        fs::write(root.join(".gitignore"), ".zudo-doc/\n").unwrap();

        let result = enumerate_first_party_staging_dirs(root);
        assert_eq!(
            result,
            vec![(
                root.join(".zudo-doc/routes-src"),
                PathBuf::from(".zudo-doc/routes-src")
            )],
            "gitignored allowlisted dir must still be enumerated"
        );

        // The generic walker still prunes the hidden dir — unchanged behavior.
        let extra = dir_names(enumerate_extra_top_level_dirs(root, &[]));
        assert!(
            !extra.contains(&".zudo-doc".to_string()),
            ".zudo-doc must stay excluded from the generic extra-dirs pass; got {extra:?}"
        );
    }

    /// Absent allowlist paths are not enumerated — `.zudo-doc` without a
    /// `routes-src` child yields nothing, as does an empty project.
    #[test]
    fn first_party_staging_dirs_absent_paths_not_enumerated() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(enumerate_first_party_staging_dirs(root).is_empty());
        fs::create_dir_all(root.join(".zudo-doc/cache")).unwrap();
        assert!(
            enumerate_first_party_staging_dirs(root).is_empty(),
            ".zudo-doc without routes-src must not be enumerated"
        );
    }

    /// A symlinked allowlist path (or a symlinked parent) is never enumerated
    /// — the allowlist stages real generated directories, not aliased trees.
    #[cfg(unix)]
    #[test]
    fn first_party_staging_dirs_symlinked_paths_not_enumerated() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let outside = tmp.path().join("outside-tree");
        fs::create_dir_all(outside.join("routes-src")).unwrap();

        // routes-src itself a symlink.
        fs::create_dir_all(root.join("a/.zudo-doc")).unwrap();
        std::os::unix::fs::symlink(
            outside.join("routes-src"),
            root.join("a/.zudo-doc/routes-src"),
        )
        .unwrap();
        assert!(enumerate_first_party_staging_dirs(&root.join("a")).is_empty());

        // The parent `.zudo-doc` a symlink.
        fs::create_dir_all(root.join("b")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("b/.zudo-doc")).unwrap();
        assert!(enumerate_first_party_staging_dirs(&root.join("b")).is_empty());
    }

    /// `.zfb/*.json` enumeration: top-level regular `*.json` files only —
    /// nested files, non-JSON files, and the gitignore are all irrelevant.
    #[test]
    fn first_party_staging_json_files_top_level_json_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".zfb/nested")).unwrap();
        fs::write(root.join(".zfb/doc-history-meta.json"), "{}\n").unwrap();
        fs::write(root.join(".zfb/cache.bin"), "binary\n").unwrap();
        fs::write(root.join(".zfb/nested/deep.json"), "{}\n").unwrap();
        fs::write(root.join(".gitignore"), ".zfb/\n").unwrap();

        let result = enumerate_first_party_staging_json_files(root);
        assert_eq!(
            result,
            vec![(
                root.join(".zfb/doc-history-meta.json"),
                PathBuf::from(".zfb/doc-history-meta.json")
            )],
            "only top-level .zfb/*.json must be enumerated (gitignore bypassed)"
        );
    }

    /// A symlinked `.zfb` dir (or a symlinked `*.json` inside a real one) is
    /// never enumerated — `fs::read_dir` would follow the dir symlink and
    /// stage meta files from an arbitrary external target, breaking the
    /// bounded staging surface.
    #[cfg(unix)]
    #[test]
    fn first_party_staging_json_files_symlinked_paths_not_enumerated() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let outside = tmp.path().join("outside-tree");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("leaked.json"), "{}\n").unwrap();

        // `.zfb` itself a symlink.
        fs::create_dir_all(root.join("a")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("a/.zfb")).unwrap();
        assert!(enumerate_first_party_staging_json_files(&root.join("a")).is_empty());

        // A symlinked json file inside a real `.zfb`.
        fs::create_dir_all(root.join("b/.zfb")).unwrap();
        std::os::unix::fs::symlink(outside.join("leaked.json"), root.join("b/.zfb/alias.json"))
            .unwrap();
        assert!(enumerate_first_party_staging_json_files(&root.join("b")).is_empty());
    }

    // ── issue #1692 claim-plan / wholesale-mirror helper tests ───────────

    #[test]
    fn alias_target_claim_path_strips_glob_suffix() {
        assert_eq!(
            alias_target_claim_path("/ws/lib/shared/*"),
            normalize_path_lexical(Path::new("/ws/lib/shared"))
        );
        assert_eq!(
            alias_target_claim_path("/ws/lib/shared/**"),
            normalize_path_lexical(Path::new("/ws/lib/shared"))
        );
        // A concrete target claims itself.
        assert_eq!(
            alias_target_claim_path("/ws/lib/shared/index.ts"),
            normalize_path_lexical(Path::new("/ws/lib/shared/index.ts"))
        );
    }

    #[test]
    fn resolve_mirror_root_uses_nearest_package_json_ancestor() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let project = ws.join("sub-packages/host");
        fs::create_dir_all(&project).unwrap();
        // Package-layout sibling with its own package.json.
        fs::create_dir_all(ws.join("packages/ui/deep")).unwrap();
        fs::write(ws.join("packages/ui/package.json"), "{}").unwrap();
        let claim = ws.join("packages/ui/deep/util.ts");
        fs::write(&claim, "x").unwrap();
        assert_eq!(
            resolve_mirror_root(&claim, &project, ws),
            Some(normalize_path_lexical(&ws.join("packages/ui"))),
            "a file under a package.json'd sibling resolves to that package dir"
        );

        // Bare-dir sibling (no package.json anywhere between the file and the
        // workspace root) → the claim's own directory.
        fs::create_dir_all(ws.join("lib/shared")).unwrap();
        let bare = ws.join("lib/shared/contract.ts");
        fs::write(&bare, "x").unwrap();
        assert_eq!(
            resolve_mirror_root(&bare, &project, ws),
            Some(normalize_path_lexical(&ws.join("lib/shared"))),
            "a bare-dir sibling resolves to the claim's own directory"
        );
    }

    #[test]
    fn resolve_mirror_root_rejects_non_sibling_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let project = ws.join("sub-packages/host");
        fs::create_dir_all(&project).unwrap();

        // No workspace: first_party_root == project_root → never a sibling.
        let inside = project.join("src/app.ts");
        fs::create_dir_all(inside.parent().unwrap()).unwrap();
        fs::write(&inside, "x").unwrap();
        assert_eq!(resolve_mirror_root(&inside, &project, &project), None);

        // A project-local path is not a sibling.
        assert_eq!(resolve_mirror_root(&inside, &project, ws), None);

        // A node_modules path is never mirrored.
        let vendored = ws.join("node_modules/pkg/x.ts");
        fs::create_dir_all(vendored.parent().unwrap()).unwrap();
        fs::write(&vendored, "x").unwrap();
        assert_eq!(resolve_mirror_root(&vendored, &project, ws), None);

        // A directly claimed hidden/infra directory cannot become the walk
        // root: ignore walkers preserve their own root even when it would be
        // pruned as a descendant.
        for root_name in [".unstaged", "dist"] {
            let skipped = ws.join(root_name).join("escape.ts");
            fs::create_dir_all(skipped.parent().unwrap()).unwrap();
            fs::write(&skipped, "x").unwrap();
            assert_eq!(resolve_mirror_root(&skipped, &project, ws), None);
        }

        // The first-party root itself is never mirrored wholesale.
        let root_file = ws.join("root.ts");
        fs::write(&root_file, "x").unwrap();
        assert_eq!(resolve_mirror_root(&root_file, &project, ws), None);

        // A region CONTAINING the project (an ancestor-of-project dir) is not a
        // disjoint sibling region.
        let container_claim = ws.join("sub-packages/thing.ts");
        fs::write(&container_claim, "x").unwrap();
        assert_eq!(resolve_mirror_root(&container_claim, &project, ws), None);
    }

    /// Epic #1995 depends on this invariant far from here, so pin it by
    /// name rather than leaving it as one assertion inside the general
    /// rejection sweep above.
    ///
    /// `orchestrator.rs`'s `content_under_css_mirror_root` gate is only
    /// meaningfully NARROW because no published mirror root can be an
    /// ancestor of `project_root`: such a root would make
    /// `is_under_css_mirror_root` true for every path in the project, and the
    /// gate would silently become the option (a) the epic REJECTED — every
    /// ordinary markdown edit rerunning the Tailwind scan — with no test
    /// failing anywhere. (The gate carries its own defensive re-check, pinned
    /// by `orchestrator::tests::degenerate_project_containing_mirror_root_does_not_rerun_css`;
    /// this test is the primary guard, at the source that publishes roots.)
    #[test]
    fn resolve_mirror_root_never_returns_an_ancestor_of_project_root() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let project = ws.join("sub-packages/host");
        fs::create_dir_all(&project).unwrap();

        // (1) A file claim sitting directly in the project's parent.
        let file_claim = ws.join("sub-packages/thing.ts");
        fs::write(&file_claim, "x").unwrap();
        assert_eq!(resolve_mirror_root(&file_claim, &project, ws), None);

        // (2) The project's parent DIRECTORY itself as the claim (an alias
        // target's directory component resolves to exactly this shape).
        let dir_claim = ws.join("sub-packages");
        assert_eq!(resolve_mirror_root(&dir_claim, &project, ws), None);

        // (3) The same, with a `package.json` present — the nearest-ancestor
        // search must never be able to RETURN a project-containing directory
        // just because it looks like a package.
        fs::write(dir_claim.join("package.json"), "{}").unwrap();
        assert_eq!(resolve_mirror_root(&file_claim, &project, ws), None);
        assert_eq!(resolve_mirror_root(&dir_claim, &project, ws), None);

        // (4) A deeper claim whose nearest `package.json` ancestor is the
        // project-containing directory from (3): the search must break out
        // rather than climb into it.
        let nested = ws.join("sub-packages/host-tools/util.ts");
        fs::create_dir_all(nested.parent().unwrap()).unwrap();
        fs::write(&nested, "x").unwrap();
        assert_eq!(
            resolve_mirror_root(&nested, &project, ws),
            Some(ws.join("sub-packages/host-tools")),
            "a genuine sibling must still resolve — this test must not pass by \
             rejecting everything"
        );
    }

    #[test]
    fn sibling_mirror_plan_empty_for_standalone_project() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let outside = tmp.path().join("lib/shared/x.ts");
        fs::create_dir_all(outside.parent().unwrap()).unwrap();
        fs::write(&outside, "x").unwrap();
        let files = BTreeSet::from([outside]);
        // first_party_root == project_root ⇒ standalone.
        let plan = SiblingMirrorPlan::compute(&project, &project, &files, &BTreeMap::new(), &[]);
        assert!(
            plan.is_empty(),
            "a standalone project claims no sibling region"
        );
    }

    #[test]
    fn sibling_mirror_plan_claims_from_discovered_files_and_wildcard_alias() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let project = ws.join("sub-packages/host");
        fs::create_dir_all(&project).unwrap();
        // (a) a discovered graph file in a bare-dir sibling.
        fs::create_dir_all(ws.join("lib/shared")).unwrap();
        let discovered = ws.join("lib/shared/contract.ts");
        fs::write(&discovered, "x").unwrap();
        // (b) a wildcard alias into a DIFFERENT sibling with no discovered file.
        fs::create_dir_all(ws.join("packages/ui")).unwrap();
        fs::write(ws.join("packages/ui/package.json"), "{}").unwrap();
        let tsconfig_paths = BTreeMap::from([(
            "@ui/*".to_string(),
            vec![ws.join("packages/ui/*").to_string_lossy().into_owned()],
        )]);

        let plan = SiblingMirrorPlan::compute(
            &project,
            ws,
            &BTreeSet::from([discovered]),
            &tsconfig_paths,
            &[],
        );
        let roots: Vec<PathBuf> = plan.mirror_roots().map(Path::to_path_buf).collect();
        assert!(
            roots.contains(&normalize_path_lexical(&ws.join("lib/shared"))),
            "the discovered-file claim contributes lib/shared; got {roots:?}"
        );
        assert!(
            roots.contains(&normalize_path_lexical(&ws.join("packages/ui"))),
            "the wildcard-alias claim contributes packages/ui; got {roots:?}"
        );
        // The audit predicate reports paths inside a claimed root.
        assert!(plan.claims_path(&ws.join("packages/ui/nested/x.ts")));
        assert!(!plan.claims_path(&project.join("src/app.ts")));
    }

    // -----------------------------------------------------------------
    // `import.meta.glob` matched-files fixed-point drain (#1695), driving
    // `stage_glob_matched_files_to_fixed_point` directly rather than the
    // full `bundle_with_session` orchestration — the queue's own shape
    // (unconditional staging, transitive drain via nested globs AND plain
    // imports, the `.mdx` guard, tick-boundary pruning) is what these tests
    // pin down.
    // -----------------------------------------------------------------

    #[test]
    fn glob_fixed_point_stages_match_outside_every_claim() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let project = ws.join("app");
        fs::create_dir_all(&project).unwrap();

        // A totally unrelated sibling region no claim source ever saw.
        fs::create_dir_all(ws.join("other-lib")).unwrap();
        let unclaimed_target = ws.join("other-lib/widget.ts");
        fs::write(&unclaimed_target, "export const widget = 'w';\n").unwrap();

        let out = tempfile::tempdir().unwrap();
        let work = out.path().join("work");
        let shadow = work.join("app");
        fs::create_dir_all(&shadow).unwrap();

        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(&project, &exclude);
        ctx.glob_matched_files.borrow_mut().insert(unclaimed_target);

        stage_glob_matched_files_to_fixed_point(&ctx, &project, ws, &shadow, &work, None).unwrap();

        assert!(
            work.join("other-lib/widget.ts").exists(),
            "a match outside every claimed mirror root must be staged at its workspace-relative slot"
        );
        assert_eq!(
            fs::read_to_string(work.join("other-lib/widget.ts")).unwrap(),
            "export const widget = 'w';\n"
        );
    }

    #[test]
    fn glob_fixed_point_expands_nested_macro_in_a_match_already_inside_a_claimed_region() {
        // codex-review regression test (P1 on the first cut of this queue):
        // `mirror_sibling_root`'s wholesale mirror is a RAW byte copy with
        // no macro expansion, so a match already inside a CLAIMED sibling
        // region that itself uses `import.meta.glob` must still have that
        // macro expanded when the queue stages it — skipping claimed-region
        // matches (the original design) left the literal, unexpanded macro
        // reaching esbuild.
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let project = ws.join("app");
        fs::create_dir_all(&project).unwrap();

        fs::create_dir_all(ws.join("lib/shared/nested")).unwrap();
        // `already-mirrored.ts` stands in for a file `mirror_sibling_root`
        // already wholesale-copied because it lives inside a claimed
        // region — but the copy is raw bytes; its OWN glob macro is still
        // unexpanded until THIS queue stages it.
        let claimed_target = ws.join("lib/shared/already-mirrored.ts");
        fs::write(
            &claimed_target,
            "export const mods = import.meta.glob('./nested/*.ts', { eager: true });\n",
        )
        .unwrap();
        fs::write(
            ws.join("lib/shared/nested/leaf.ts"),
            "export const leaf = 1;\n",
        )
        .unwrap();

        let out = tempfile::tempdir().unwrap();
        let work = out.path().join("work");
        let shadow = work.join("app");
        fs::create_dir_all(&shadow).unwrap();

        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(&project, &exclude);
        ctx.glob_matched_files.borrow_mut().insert(claimed_target);

        stage_glob_matched_files_to_fixed_point(&ctx, &project, ws, &shadow, &work, None).unwrap();

        let staged = work.join("lib/shared/already-mirrored.ts");
        assert!(staged.exists(), "the claimed-region match must be staged");
        let staged_source = fs::read_to_string(&staged).unwrap();
        assert!(
            !staged_source.contains("import.meta.glob("),
            "a claimed-region match's OWN glob macro must be expanded, not left raw: {staged_source}"
        );
        assert!(
            work.join("lib/shared/nested/leaf.ts").exists(),
            "the nested match discovered by the claimed-region file's own glob must be staged too"
        );
    }

    #[test]
    fn glob_fixed_point_drains_transitive_glob_chain_to_a_fixed_point() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let project = ws.join("app");
        fs::create_dir_all(&project).unwrap();

        // `hop1.ts` is the seed; it itself glob-imports `nested/*.ts`,
        // matching `hop2.ts` — a SECOND round the drain must discover on
        // its own, without `hop2.ts` ever being queued directly.
        fs::create_dir_all(ws.join("other-lib/nested")).unwrap();
        fs::write(
            ws.join("other-lib/hop1.ts"),
            "export const mods = import.meta.glob('./nested/*.ts', { eager: true });\n",
        )
        .unwrap();
        fs::write(
            ws.join("other-lib/nested/hop2.ts"),
            "export const hop2 = 'end';\n",
        )
        .unwrap();

        let out = tempfile::tempdir().unwrap();
        let work = out.path().join("work");
        let shadow = work.join("app");
        fs::create_dir_all(&shadow).unwrap();

        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(&project, &exclude);
        ctx.glob_matched_files
            .borrow_mut()
            .insert(ws.join("other-lib/hop1.ts"));

        stage_glob_matched_files_to_fixed_point(&ctx, &project, ws, &shadow, &work, None).unwrap();

        let hop1_staged = work.join("other-lib/hop1.ts");
        let hop2_staged = work.join("other-lib/nested/hop2.ts");
        assert!(hop1_staged.exists(), "the seeded first hop must be staged");
        assert!(
            hop2_staged.exists(),
            "hop1's OWN glob match (hop2) must be staged too — the drain must run a second round"
        );
        let hop1_source = fs::read_to_string(&hop1_staged).unwrap();
        assert!(
            !hop1_source.contains("import.meta.glob("),
            "hop1's own glob macro must be expanded when staged, not copied verbatim: {hop1_source}"
        );
        assert!(
            hop1_source.contains("./nested/hop2.ts"),
            "hop1's expansion must reference the matched hop2 file: {hop1_source}"
        );
    }

    #[test]
    fn glob_fixed_point_stages_plain_dependency_closure_of_a_matched_file() {
        // codex-review regression test (P2): `materialise_source_file` only
        // expands glob/`?raw`/worker macros — a matched file's ORDINARY
        // `import` dependency is invisible to it. The queue must run the
        // same discovery pass the exact-target staging loop uses so a
        // matched file's plain-import closure is staged too, or esbuild
        // fails to resolve it.
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let project = ws.join("app");
        fs::create_dir_all(&project).unwrap();
        // `discover_module_preprocessing_with_context` re-derives the
        // first-party boundary itself (`zfb_types::first_party_root_for`),
        // which only widens past `project_root` for a REAL
        // `pnpm-workspace.yaml` claiming it as a member — required here so
        // the discovery call accepts `other-lib` (a sibling of `app`) as
        // first-party rather than rejecting it as an escape.
        fs::write(ws.join("pnpm-workspace.yaml"), "packages:\n  - 'app'\n").unwrap();

        fs::create_dir_all(ws.join("other-lib")).unwrap();
        let matched = ws.join("other-lib/widget.ts");
        fs::write(
            &matched,
            "import { helper } from './helper.ts';\nexport const widget = helper();\n",
        )
        .unwrap();
        fs::write(
            ws.join("other-lib/helper.ts"),
            "export function helper() { return 1; }\n",
        )
        .unwrap();

        let out = tempfile::tempdir().unwrap();
        let work = out.path().join("work");
        let shadow = work.join("app");
        fs::create_dir_all(&shadow).unwrap();

        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(&project, &exclude);
        ctx.glob_matched_files.borrow_mut().insert(matched);

        stage_glob_matched_files_to_fixed_point(&ctx, &project, ws, &shadow, &work, None).unwrap();

        assert!(
            work.join("other-lib/widget.ts").exists(),
            "the matched file itself must be staged"
        );
        assert!(
            work.join("other-lib/helper.ts").exists(),
            "the matched file's PLAIN (non-glob) import must be staged too, or esbuild cannot resolve it"
        );
    }

    #[test]
    fn glob_fixed_point_skips_mdx_matches_to_avoid_clobbering_a_correct_compile() {
        // codex-review regression test (P2): an `.mdx` match needs the MDX
        // compile pipeline, not `materialise_source_file`'s plain JS/TS
        // transform. Staging it here would write raw/uncompiled Markdown
        // over whatever a real MDX-aware pass produced — worse than a no-op
        // — so the queue must skip it outright.
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let project = ws.join("app");
        fs::create_dir_all(&project).unwrap();

        fs::create_dir_all(ws.join("other-lib")).unwrap();
        let target = ws.join("other-lib/doc.mdx");
        fs::write(&target, "# Hello\n").unwrap();

        let out = tempfile::tempdir().unwrap();
        let work = out.path().join("work");
        let shadow = work.join("app");
        fs::create_dir_all(&shadow).unwrap();

        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(&project, &exclude);
        ctx.glob_matched_files.borrow_mut().insert(target);

        stage_glob_matched_files_to_fixed_point(&ctx, &project, ws, &shadow, &work, None).unwrap();

        assert!(
            !work.join("other-lib/doc.mdx").exists(),
            "an `.mdx` match must be skipped by this queue, not raw-copied over a real MDX compile"
        );
    }

    #[test]
    fn glob_fixed_point_stages_project_side_match_outside_ordinary_walk_coverage() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("app");
        // Stands in for a project subtree the ordinary `materialise_shadow` /
        // `enumerate_extra_top_level_dirs` walks never reach (e.g.
        // gitignored) — this isolated test drives the drain directly, with
        // no walk at all, so the target is ONLY reachable through the queue.
        fs::create_dir_all(project.join("generated")).unwrap();
        let target = project.join("generated/data.ts");
        fs::write(&target, "export const data = 1;\n").unwrap();

        let out = tempfile::tempdir().unwrap();
        let shadow = out.path().join("shadow");
        fs::create_dir_all(&shadow).unwrap();

        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(&project, &exclude);
        ctx.glob_matched_files.borrow_mut().insert(target);

        // No workspace ⇒ `shadow == work`, mirroring the byte-identical
        // no-op layout `bundle_with_session` uses outside a workspace.
        stage_glob_matched_files_to_fixed_point(&ctx, &project, &project, &shadow, &shadow, None)
            .unwrap();

        assert!(
            shadow.join("generated/data.ts").exists(),
            "a project-side glob match outside every ordinary walk's coverage must still be staged"
        );
    }

    #[test]
    fn glob_fixed_point_prunes_vanished_match_on_next_session_tick() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let project = ws.join("app");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(ws.join("other-lib")).unwrap();
        let target = ws.join("other-lib/gone-soon.ts");
        fs::write(&target, "export const x = 1;\n").unwrap();

        let exclude = no_bundle_exclude();
        let mut session = ShadowSession::new(&project).unwrap();
        let work = session.shadow_root().to_path_buf();
        let shadow = work.join("app");
        let staged_rel = Path::new("other-lib/gone-soon.ts");

        // Tick 1: the live glob matches `target` — every `MaterialiseCtx`
        // (including its `glob_matched_files` accumulator) is freshly built
        // per tick, exactly like `bundle_with_session` builds one per call.
        {
            let fingerprint = spec_fingerprint(&zfb_content::PipelineSpec::default());
            let writer =
                ShadowWriter::new(work.clone(), Some(&mut session), false, fingerprint).unwrap();
            let ctx = MaterialiseCtx {
                pipeline_spec: zfb_content::PipelineSpec::default(),
                copy_mode: false,
                bundle_exclude: &exclude,
                project_root: &project,
                writer: &writer,
                glob_matched_files: RefCell::new(BTreeSet::from([target.clone()])),
                raw_import_edges: RefCell::new(BTreeSet::new()),
                raw_import_aliases: RawImportAliasContext::empty(),
                module_worker_dependencies: RefCell::new(BTreeSet::new()),
                worker_build_context: ModuleWorkerBuildContext::default(),
                raw_preflight_complete: Cell::new(false),
                snapshot_specifiers: None,
            };
            stage_glob_matched_files_to_fixed_point(&ctx, &project, ws, &shadow, &work, None)
                .unwrap();
            writer.prune_stale().unwrap();
            writer.mark_clean();
        }
        assert!(
            work.join(staged_rel).exists(),
            "tick 1: the matched target must be staged into the work mirror"
        );

        // Tick 2: the live glob no longer matches `target` (deleted /
        // renamed / edited out of the pattern) — this tick's
        // `glob_matched_files` starts (and stays) empty.
        {
            let fingerprint = spec_fingerprint(&zfb_content::PipelineSpec::default());
            let writer =
                ShadowWriter::new(work.clone(), Some(&mut session), false, fingerprint).unwrap();
            let ctx = MaterialiseCtx {
                pipeline_spec: zfb_content::PipelineSpec::default(),
                copy_mode: false,
                bundle_exclude: &exclude,
                project_root: &project,
                writer: &writer,
                glob_matched_files: RefCell::new(BTreeSet::new()),
                raw_import_edges: RefCell::new(BTreeSet::new()),
                raw_import_aliases: RawImportAliasContext::empty(),
                module_worker_dependencies: RefCell::new(BTreeSet::new()),
                worker_build_context: ModuleWorkerBuildContext::default(),
                raw_preflight_complete: Cell::new(false),
                snapshot_specifiers: None,
            };
            stage_glob_matched_files_to_fixed_point(&ctx, &project, ws, &shadow, &work, None)
                .unwrap();
            writer.prune_stale().unwrap();
            writer.mark_clean();
        }
        assert!(
            !work.join(staged_rel).exists(),
            "tick 2: a vanished glob match must be pruned from the work mirror"
        );
    }

    #[test]
    fn has_wildcard_root_alias_detects_root_wildcard() {
        let tmp = tempfile::tempdir().unwrap();
        let project = normalize_path_lexical(tmp.path());
        let root_wildcard = BTreeMap::from([(
            "@/*".to_string(),
            vec![project.join("*").to_string_lossy().into_owned()],
        )]);
        assert!(has_wildcard_root_alias(&project, &root_wildcard, &[]));

        // A wildcard into a SUBDIR (not the root) does not arm the root pass.
        let subdir_wildcard = BTreeMap::from([(
            "@/*".to_string(),
            vec![project.join("src/*").to_string_lossy().into_owned()],
        )]);
        assert!(!has_wildcard_root_alias(&project, &subdir_wildcard, &[]));

        // A concrete root alias is staged file-by-file, not by the loose pass.
        let concrete = BTreeMap::from([(
            "@data".to_string(),
            vec![project.join("data.json").to_string_lossy().into_owned()],
        )]);
        assert!(!has_wildcard_root_alias(&project, &concrete, &[]));
    }

    #[test]
    fn enumerate_mirror_root_files_honors_gitignore_and_skip_list() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("keep.ts"), "x").unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub/nested.ts"), "x").unwrap();
        fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        fs::write(root.join("node_modules/pkg/v.ts"), "x").unwrap();
        fs::create_dir_all(root.join("dist")).unwrap();
        fs::write(root.join("dist/out.js"), "x").unwrap();
        fs::write(root.join(".gitignore"), "ignored.ts\n").unwrap();
        fs::write(root.join("ignored.ts"), "x").unwrap();

        let rels: Vec<PathBuf> = enumerate_mirror_root_files(root)
            .into_iter()
            .map(|p| p.strip_prefix(root).unwrap().to_path_buf())
            .collect();
        assert!(
            rels.contains(&PathBuf::from("keep.ts")),
            "root-level file kept; got {rels:?}"
        );
        assert!(
            rels.contains(&PathBuf::from("sub/nested.ts")),
            "nested file kept; got {rels:?}"
        );
        assert!(
            !rels.iter().any(|p| p.starts_with("node_modules")),
            "node_modules pruned; got {rels:?}"
        );
        assert!(
            !rels.iter().any(|p| p.starts_with("dist")),
            "dist pruned; got {rels:?}"
        );
        assert!(
            !rels.contains(&PathBuf::from("ignored.ts")),
            "gitignored file pruned; got {rels:?}"
        );
    }

    // ── symlink_or_copy tests ────────────────────────────────────────────

    /// Non-transformed files (CSS, PNG) placed by materialise_shadow must
    /// be symlinks on Unix, not byte-copies. On Windows (fallback path) we
    /// accept either a symlink or a regular file.
    #[test]
    fn symlink_or_copy_non_transformed_files_are_symlinks() {
        let src_tmp = tempfile::tempdir().unwrap();
        let dest_tmp = tempfile::tempdir().unwrap();

        let src = src_tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("style.css"), "body { margin: 0; }\n").unwrap();
        // Write minimal 1×1 PNG (binary content).
        fs::write(
            src.join("image.png"),
            b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x02\x00\x00\x00\x90wS\xde\x00\x00\x00\x0cIDATx\x9cc\xf8\x0f\x00\x00\x01\x01\x00\x05\x18\xd8N\x00\x00\x00\x00IEND\xaeB`\x82",
        )
        .unwrap();

        let dest = dest_tmp.path().join("shadow");
        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(src_tmp.path(), &exclude);
        materialise_shadow(
            &src,
            &dest,
            &mut Vec::new(),
            &ctx,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();

        for filename in &["style.css", "image.png"] {
            let dest_file = dest.join(filename);
            let meta = fs::symlink_metadata(&dest_file)
                .unwrap_or_else(|e| panic!("{filename} metadata error: {e}"));

            #[cfg(unix)]
            {
                assert!(
                    meta.file_type().is_symlink(),
                    "{filename} must be a symlink in the shadow tree on unix; got {meta:?}"
                );
                let target = fs::read_link(&dest_file)
                    .unwrap_or_else(|e| panic!("{filename} read_link error: {e}"));
                assert_eq!(
                    target,
                    src.join(filename),
                    "{filename} symlink must point at the source path"
                );
            }
            #[cfg(windows)]
            {
                // Windows: accept symlink (privileged context) OR regular file (fallback).
                assert!(
                    meta.file_type().is_symlink() || meta.file_type().is_file(),
                    "{filename} must be a symlink or regular file on windows; got {meta:?}"
                );
            }
        }
    }

    /// Compiled MDX destinations must be real files (not symlinks) and their
    /// content must differ from the source (compilation happened).
    #[test]
    fn symlink_or_copy_mdx_destination_is_real_file_not_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("docs");
        fs::create_dir_all(&src).unwrap();
        let mdx_src_content = "---\ntitle: Test\n---\n\n# Hello\n\nsome paragraph\n";
        fs::write(src.join("guide.mdx"), mdx_src_content).unwrap();

        let dest = tmp.path().join("shadow");
        let mut imports: Vec<ContentImport> = Vec::new();
        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(tmp.path(), &exclude);
        materialise_collection(
            &src,
            &dest,
            "docs",
            &mut imports,
            &ctx,
            None,
            None,
            None,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();

        let dest_mdx = dest.join("guide.mdx");
        let meta = fs::symlink_metadata(&dest_mdx).expect("guide.mdx must exist in shadow");

        // Must be a regular file, not a symlink.
        assert!(
            !meta.file_type().is_symlink(),
            "compiled MDX destination must NOT be a symlink; it should be a real file with compiled JSX"
        );
        assert!(
            meta.file_type().is_file(),
            "compiled MDX destination must be a regular file"
        );

        // Contents must differ from source (compilation happened).
        let dest_content = fs::read_to_string(&dest_mdx).unwrap();
        assert_ne!(
            dest_content, mdx_src_content,
            "compiled MDX destination content must differ from source (compilation must have run)"
        );
        // Sanity-check: compiled output ships a _createMdxContent wrapper.
        assert!(
            dest_content.contains("_createMdxContent"),
            "compiled MDX output must contain _createMdxContent; got:\n{dest_content}"
        );
    }

    /// Teardown-safety regression test: dropping the shadow TempDir (which
    /// contains symlinks) must NOT remove the original source files.
    ///
    /// This locks the core safety contract of #429 Option 1: a symlink's
    /// referent is never deleted when the symlink itself is removed.
    #[test]
    fn symlink_or_copy_teardown_does_not_remove_source_files() {
        // Source and destination are in SEPARATE tempdirs so that dropping
        // the dest tempdir cannot accidentally remove the src.
        let src_tmp = tempfile::tempdir().unwrap();
        let src = src_tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        let src_file = src.join("app.tsx");
        fs::write(&src_file, "export default () => null;\n").unwrap();

        {
            // Materialise into a separate tempdir and then drop it.
            let dest_tmp = tempfile::tempdir().unwrap();
            let exclude = no_bundle_exclude();
            let ctx = default_mat_ctx(src_tmp.path(), &exclude);
            materialise_shadow(
                &src,
                dest_tmp.path(),
                &mut Vec::new(),
                &ctx,
                &mut Vec::new(),
                &mut Vec::new(),
                &mut Vec::new(),
                &mut Vec::new(),
            )
            .unwrap();
            // dest_tmp dropped here — symlinks in it are removed, not their targets.
        }

        // Original source file must still exist and be readable.
        assert!(
            src_file.exists(),
            "source file must still exist after shadow TempDir is dropped"
        );
        let contents = fs::read_to_string(&src_file)
            .expect("source file must still be readable after shadow TempDir is dropped");
        assert_eq!(
            contents, "export default () => null;\n",
            "source file contents must be unchanged after shadow TempDir teardown"
        );
    }

    /// Empty `bundle.exclude` matcher — never matches, so `materialise_shadow`
    /// test calls behave exactly as they did before the knob existed.
    fn no_bundle_exclude() -> BundleExcludeMatcher {
        BundleExcludeMatcher::new(&[]).expect("empty bundle.exclude compiles")
    }

    /// Convenience constructor for test call sites: all pipeline options at
    /// their default / disabled state, no `bundle.exclude`, and the supplied
    /// `project_root`.  The `exclude` is kept alive by the caller.
    fn default_mat_ctx<'a>(
        project_root: &'a Path,
        exclude: &'a BundleExcludeMatcher,
    ) -> MaterialiseCtx<'a, 'static> {
        MaterialiseCtx {
            pipeline_spec: zfb_content::PipelineSpec::default(),
            copy_mode: false,
            bundle_exclude: exclude,
            project_root,
            writer: leaked_passthrough_writer(),
            glob_matched_files: RefCell::new(BTreeSet::new()),
            raw_import_edges: RefCell::new(BTreeSet::new()),
            raw_import_aliases: RawImportAliasContext::empty(),
            module_worker_dependencies: RefCell::new(BTreeSet::new()),
            worker_build_context: ModuleWorkerBuildContext::default(),
            raw_preflight_complete: Cell::new(false),
            // Passthrough/sessionless never skips, so the snapshot gate is
            // irrelevant here.
            snapshot_specifiers: None,
        }
    }

    /// Sessionless `ShadowWriter` for test call sites — passthrough mode
    /// performs exactly the pre-#993 fs operations. Leaked (`Box::leak`)
    /// so `default_mat_ctx` can hand out a `'static` borrow without
    /// changing its many call sites; the struct is a few words, leaked
    /// once per test call.
    fn leaked_passthrough_writer() -> &'static ShadowWriter<'static> {
        Box::leak(Box::new(
            ShadowWriter::new(
                PathBuf::from("/nonexistent-passthrough-shadow"),
                None,
                false,
                None,
            )
            .expect("passthrough writer construction is infallible"),
        ))
    }

    // ── bundle.exclude matcher tests (#664 / #672) ───────────────────────

    #[test]
    fn bundle_exclude_empty_matcher_never_matches() {
        // Zero-regression contract: an empty pattern list yields a matcher
        // that never matches, so the per-file skip never fires and the build
        // is byte-identical to one without the knob.
        let m = BundleExcludeMatcher::new(&[]).unwrap();
        let root = Path::new("/proj");
        assert!(!m.is_excluded(Path::new("/proj/components/Foo.stories.tsx"), root));
        assert!(!m.is_excluded(Path::new("/proj/pages/index.tsx"), root));
    }

    #[test]
    fn bundle_exclude_matches_project_relative_glob() {
        let root = Path::new("/proj");
        // `*` stops at `/` (literal_separator), so `components/*.stories.tsx`
        // matches a top-level story but NOT a nested one.
        let m = BundleExcludeMatcher::new(&["components/*.stories.tsx".to_string()]).unwrap();
        assert!(m.is_excluded(Path::new("/proj/components/Button.stories.tsx"), root));
        assert!(!m.is_excluded(Path::new("/proj/components/sub/Deep.stories.tsx"), root));
        assert!(!m.is_excluded(Path::new("/proj/components/Button.tsx"), root));
        // A path outside the project root cannot be project-relative → never
        // excluded.
        assert!(!m.is_excluded(Path::new("/elsewhere/components/X.stories.tsx"), root));

        // `**` recurses across `/`.
        let deep = BundleExcludeMatcher::new(&["components/**/*.stories.tsx".to_string()]).unwrap();
        assert!(deep.is_excluded(Path::new("/proj/components/Button.stories.tsx"), root));
        assert!(deep.is_excluded(Path::new("/proj/components/sub/Deep.stories.tsx"), root));
    }

    // ── issue #1668 part 2: two-tier bundle.exclude anchoring ────────────────

    /// A claimed pnpm-workspace member: returns `(TempDir, workspace_root,
    /// project_root)`. The `TempDir` guard must stay alive for the fixture's
    /// `pnpm-workspace.yaml` to keep existing while `with_workspace_root` reads
    /// it.
    fn workspace_exclude_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let ws = tempfile::tempdir().unwrap();
        let ws_root = ws.path().to_path_buf();
        fs::write(
            ws_root.join("pnpm-workspace.yaml"),
            "packages:\n  - '.'\n  - 'sub-packages/*'\n",
        )
        .unwrap();
        let project = ws_root.join("sub-packages/host");
        fs::create_dir_all(&project).unwrap();
        (ws, ws_root, project)
    }

    #[test]
    fn bundle_exclude_project_relative_pattern_never_leaks_to_siblings() {
        let (_ws, ws_root, project) = workspace_exclude_fixture();
        // A PROJECT-relative pattern.
        let m = BundleExcludeMatcher::new(&["src/**".to_string()])
            .unwrap()
            .with_workspace_root(&project);
        // Tier 1: a project file under the project's own `src/` is excluded.
        assert!(m.is_excluded(&project.join("src/secret.ts"), &project));
        // Tier 2: a sibling whose WORKSPACE-relative path is `lib/shared/foo.ts`
        // is anchored workspace-relative, so the project-relative `src/**`
        // never reaches it.
        assert!(!m.is_excluded(&ws_root.join("lib/shared/foo.ts"), &project));
        // A sibling package's OWN `src/` is `sub-packages/other/src/...`
        // workspace-relative — also outside the host's `src/**`.
        assert!(!m.is_excluded(&ws_root.join("sub-packages/other/src/x.ts"), &project));
    }

    #[test]
    fn bundle_exclude_workspace_relative_pattern_matches_siblings() {
        let (_ws, ws_root, project) = workspace_exclude_fixture();
        let sibling = ws_root.join("lib/shared/secret.ts");
        // A workspace-relative sibling pattern.
        let m = BundleExcludeMatcher::new(&["lib/shared/*.ts".to_string()])
            .unwrap()
            .with_workspace_root(&project);
        // Tier 2: the sibling matches, anchored at the workspace root.
        assert!(
            m.is_excluded(&sibling, &project),
            "a workspace sibling must match its workspace-relative pattern"
        );
        // The tier is what enables it: the SAME pattern without the workspace
        // root leaves the sibling included (the pre-#1668 single-tier behavior,
        // where a path outside project_root was never excluded).
        let single_tier = BundleExcludeMatcher::new(&["lib/shared/*.ts".to_string()]).unwrap();
        assert!(
            !single_tier.is_excluded(&sibling, &project),
            "without the workspace tier a sibling is never excluded"
        );
    }

    #[test]
    fn bundle_exclude_sibling_tier_is_inert_without_a_workspace() {
        // A standalone project (no pnpm-workspace.yaml above it): the sibling
        // tier records no widened root, so matching is byte-identical to the
        // single-tier pre-#1668 matcher.
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("standalone");
        fs::create_dir_all(&project).unwrap();
        let m = BundleExcludeMatcher::new(&["lib/shared/*.ts".to_string()])
            .unwrap()
            .with_workspace_root(&project);
        assert!(
            m.workspace_root.is_none(),
            "a standalone project must not arm the sibling tier"
        );
        // Tier 1 still matches a project file.
        assert!(m.is_excluded(&project.join("lib/shared/secret.ts"), &project));
        // A path outside the project is never excluded (no sibling tier).
        assert!(!m.is_excluded(&tmp.path().join("adjacent/lib/shared/secret.ts"), &project));
    }

    // REMOVED (#1557): `bundle_exclude_prefix_overlap_only_keeps_provably_disjoint_wildcards`
    // pinned the `may_overlap_wildcard_target` prefix-overlap heuristic, which
    // THE SWITCH deletes — under exclusions every wildcard is suppressed
    // uniformly (no provably-disjoint carve-out), so there is nothing left to
    // test. The uniform shadow-only contract is pinned by
    // `exclusion_active_tsconfig_rebase_is_uniformly_shadow_only` above.

    #[test]
    fn bundle_exclude_invalid_pattern_is_an_error() {
        // An unclosed character class is an invalid glob — surface a clear
        // build error rather than silently ignoring the user's config.
        let err = BundleExcludeMatcher::new(&["components/[unclosed".to_string()]).unwrap_err();
        assert!(
            err.to_string().contains("invalid bundle.exclude pattern"),
            "error should name the bad pattern: {err}"
        );
    }

    #[test]
    fn materialise_shadow_bundle_exclude_skips_matched_file() {
        // Integration of the per-file skip inside materialise_shadow: an
        // excluded source file must NOT appear in the shadow tree, while a
        // sibling that does not match is materialised normally.
        let root = tempfile::tempdir().expect("tempdir");
        let src = root.path().join("components");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("Keep.tsx"), "export const keep = 1;").unwrap();
        fs::write(src.join("Drop.stories.tsx"), "export const drop = 1;").unwrap();

        let dest = root.path().join("shadow").join("components");
        let matcher = BundleExcludeMatcher::new(&["components/*.stories.tsx".to_string()]).unwrap();
        let ctx = default_mat_ctx(root.path(), &matcher);
        materialise_shadow(
            &src,
            &dest,
            &mut Vec::new(),
            &ctx,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();

        assert!(dest.join("Keep.tsx").exists(), "non-matching file kept");
        assert!(
            !dest.join("Drop.stories.tsx").exists(),
            "excluded *.stories.tsx must not be materialised into the shadow"
        );
    }

    #[test]
    fn materialise_shadow_empty_exclude_keeps_all_files() {
        // Zero-regression: with an empty bundle.exclude, every file is
        // materialised exactly as before the knob existed.
        let root = tempfile::tempdir().expect("tempdir");
        let src = root.path().join("components");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("Keep.tsx"), "export const keep = 1;").unwrap();
        fs::write(src.join("Story.stories.tsx"), "export const s = 1;").unwrap();

        let dest = root.path().join("shadow").join("components");
        let exclude = no_bundle_exclude();
        let ctx = default_mat_ctx(root.path(), &exclude);
        materialise_shadow(
            &src,
            &dest,
            &mut Vec::new(),
            &ctx,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();

        assert!(
            dest.join("Keep.tsx").exists(),
            "Keep present with empty exclude"
        );
        assert!(
            dest.join("Story.stories.tsx").exists(),
            "with empty bundle.exclude nothing is skipped (byte-identical to today)"
        );
    }

    /// `OnBrokenLinks::Error` causes `bundle()` to return `Err` when a
    /// content file contains an unresolvable markdown link.
    ///
    /// Uses `mock_subprocess_output` so no esbuild binary is required —
    /// broken-link detection runs during the MDX pipeline (before esbuild)
    /// and the `bail!` at the fatal-findings gate fires before the mock
    /// write is reached.
    ///
    /// Level: 1 (unit — pure logic in the bundler pipeline).
    #[test]
    fn on_broken_links_error_returns_err_without_esbuild() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        // Minimal project layout.
        fs::create_dir_all(root.join("pages")).unwrap();
        fs::create_dir_all(root.join("content/docs")).unwrap();
        fs::create_dir_all(root.join("components")).unwrap();
        fs::create_dir_all(root.join("layouts")).unwrap();

        fs::write(
            root.join("pages/index.tsx"),
            "export default function Home() { return null; }\n",
        )
        .unwrap();

        // The MDX file with a broken cross-doc link.
        // `./ghost.mdx` does not exist in content/docs/ — this is the
        // broken link that must trigger the error.
        fs::write(
            root.join("content/docs/article.mdx"),
            "---\ntitle: Article\n---\n\n[ghost link](./ghost.mdx)\n",
        )
        .unwrap();

        let input = BundlerInput {
            content_collections: vec![ContentCollectionSpec::new(
                "docs",
                PathBuf::from("content/docs"),
            )],
            resolve_markdown_links: Some(ResolveMarkdownLinksSpec {
                routes: vec![ResolveMarkdownLinksRoute {
                    docs_dir: PathBuf::from("content/docs"),
                    route_prefix: "/docs/".to_string(),
                }],
                on_broken_links: OnBrokenLinks::Error,
            }),
            ..make_minimal_input(&tmp)
        };

        let err = bundle(input)
            .expect_err("bundle must fail with OnBrokenLinks::Error when broken link exists");
        let msg = format!("{err:#}");

        // The error must name the broken link URL.
        assert!(
            msg.contains("ghost.mdx"),
            "error message must name the broken link; got: {msg}"
        );
        // The error must describe the problem domain.
        assert!(
            msg.contains("broken") || msg.contains("link"),
            "error message must describe the problem; got: {msg}"
        );
    }
}
