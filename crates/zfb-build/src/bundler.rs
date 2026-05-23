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
//!    `@takazudo/zfb-runtime`, and re-exports a `routes` map of
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
//!   `createPageRouter` from `@takazudo/zfb-runtime`. This is the entry
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
//! Variables in [`BundlerInput::define_vars`] that are **not** prefixed
//! with `PUBLIC_` are silently dropped — they never reach the bundle.
//! `import.meta.env.PROD` and `import.meta.env.DEV` are always emitted
//! (driven by [`BundlerInput::mode`]). See [`BundleMode`] and the
//! `server_secrets_are_not_bundled` test for the contract.
//!
//! ## Esbuild binary resolution
//!
//! Same precedence as `zfb_islands::EsbuildSubprocessConfig`:
//!
//! 1. [`BundlerInput::esbuild_binary`] (explicit override).
//! 2. `ZFB_ESBUILD_BIN` environment variable.
//! 3. `crates/zfb/binaries/esbuild/esbuild` (release-tarball slot — see
//!    that directory's README).
//!
//! If the resolved path does not exist, [`bundle`] returns a clear error
//! instructing the operator to either set the env var or stage the
//! binary in the slot. The release-engineering epic that downloads the
//! binary into the slot has not landed yet (parent: issue #5).

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;
use zfb_content::compile_mdx_to_jsx_module_cached;
use zfb_content::frontmatter as zfb_frontmatter;
use zfb_content::plugins::util::source_map::{
    CollectionRoute, DocsSourceMapOptions, build_docs_source_map,
};
use zfb_render::adapters::{make_adapter, Framework};

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
    /// Directory of route source files. Every `.tsx`/`.ts`/`.jsx`/`.js`/
    /// `.mdx` file under this directory becomes a route in the bundle.
    pub pages_dir: PathBuf,
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
    /// Build-time `--define` substitutions. The bundler **filters** this
    /// map: only keys starting with `PUBLIC_` are forwarded to esbuild
    /// (as `--define:process.env.<KEY>='<JSON-encoded value>'`). All
    /// other keys are silently dropped — server secrets MUST NOT appear
    /// in the bundle. See [`server_secrets_are_not_bundled`] in tests.
    pub define_vars: HashMap<String, String>,
    /// `compilerOptions.paths`-style alias map (TS path aliases). The
    /// bundler writes this verbatim into a synthetic `tsconfig.json`
    /// inside the shadow tree; esbuild then resolves user imports
    /// (`@/components/foo`) through it via `--tsconfig=`.
    ///
    /// Caller is responsible for resolving the project's `extends`
    /// chain (e.g. `tsconfig.base.json`) before passing the merged map
    /// here. Path targets MUST be expressed relative to the
    /// **project root** (the bundler rebases them onto the shadow tree
    /// internally).
    pub tsconfig_paths: BTreeMap<String, Vec<String>>,
    /// Bare specifiers to leave unresolved in the bundle. Use for
    /// `preact`, `react`, `react-dom/server`, etc. — packages the
    /// runtime SSR adapter (T2) provides at embedded V8 host load time. An
    /// empty vec means "bundle everything from node_modules".
    pub external: Vec<String>,
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

    /// When `true`, esbuild is invoked with `--preserve-symlinks` so
    /// it stays at the symlink location during package resolution. Set
    /// by tests that point [`Self::node_modules_dir`] at a synthetic
    /// fixture tree where the package source physically lives outside
    /// the project root.
    ///
    /// Production builds leave this `false`. pnpm-style projects
    /// store package contents under
    /// `node_modules/.pnpm/<spec>/node_modules/<pkg>/...`; following
    /// the symlink lets esbuild walk up from the *real* path and
    /// discover the per-package nested `node_modules` (where pnpm
    /// hoists transitive deps like `hono`). Preserving symlinks would
    /// keep esbuild anchored at `node_modules/<pkg>` in the project
    /// root, where transitive deps are NOT hoisted, and resolution
    /// fails.
    pub node_modules_preserve_symlinks: bool,

    /// When `true`, append [`zfb_content::pipeline::Pipeline::add_strip_md_ext`]
    /// to the hoisted MDX pre-compile pipeline at every call site so
    /// internal `[link](other.md)` style hrefs are rewritten to
    /// `other/` (with a trailing slash, matching the JS engine's
    /// `rehypeStripMdExtension`). Mirrors [`zfb::config::Config::strip_md_ext`]
    /// in `crates/zfb/src/config.rs` — the CLI threads the resolved
    /// config flag here. Default: `false` (opt-in feature).
    ///
    /// The dev loader at `crates/zfb-render/src/loader.rs` honours the
    /// same flag via [`zfb_render::loader::ModuleLoader::with_strip_md_ext`]
    /// so `zfb dev` and `zfb build` produce the same href shape.
    pub strip_md_ext: bool,

    /// Optional syntect highlight theme name.  When `Some`, the hoisted
    /// MDX pre-compile pipeline uses
    /// [`zfb_content::pipeline::Pipeline::with_defaults_and_theme`] so
    /// every fenced code block is highlighted with the named built-in
    /// syntect theme instead of the default `base16-ocean.dark`.
    ///
    /// Theme names are syntect's built-in set (`"InspiredGitHub"`,
    /// `"Solarized (light)"`, etc.). Custom themes are loaded via
    /// `code_highlight_themes_dir`. Mirrors
    /// `zfb::config::Config::code_highlight.theme`. Default: `None`.
    pub code_highlight_theme: Option<String>,

    /// Optional absolute path to a directory of `.tmTheme` files.
    ///
    /// When `Some`, the hoisted MDX pre-compile pipeline loads every
    /// `.tmTheme` file in the directory so custom themes (e.g. Dracula)
    /// become addressable by name in `code_highlight_theme`.
    ///
    /// Mirrors `zfb::config::Config::code_highlight.themes_dir` (resolved
    /// to an absolute path by the command layer before being stored here).
    /// Default: `None` — bundled themes only.
    ///
    /// MUST be kept in sync with
    /// `zfb_content::SnapshotPipelineConfig::code_highlight_themes_dir`
    /// so the snapshot ↔ bundler `content_hash` stays byte-identical.
    pub code_highlight_themes_dir: Option<PathBuf>,

    /// Optional markdown link resolver. When `Some`, the bundler builds a
    /// source map from [`ResolveMarkdownLinksSpec::docs_dir`], appends
    /// [`zfb_content::plugins::ResolveLinksPlugin`] to the MDX pipeline,
    /// and handles broken links per [`ResolveMarkdownLinksSpec::on_broken_links`].
    ///
    /// Mirrors `zfb::config::Config::resolve_markdown_links`. Default: `None`
    /// (pass-through — links are not rewritten).
    pub resolve_markdown_links: Option<ResolveMarkdownLinksSpec>,

    /// Resolved per-construct GFM flags. Threaded into the hoisted MDX
    /// pre-compile pipeline at every `materialise_*` call site so the
    /// JSX `content_hash` baked into each compiled module agrees with
    /// what the snapshot walker (`zfb_content::build_snapshot_with_config`)
    /// produces. Divergence here is the snapshot ↔ bundler hash
    /// land mine documented at
    /// `crates/zfb-content/src/content_bridge.rs:118-153`.
    ///
    /// Mirrors `zfb::config::resolve_gfm_constructs(config.markdown)`.
    /// `Default` is [`ResolvedGfmConstructs::CONSERVATIVE`] so call
    /// sites that don't construct this struct manually (the test
    /// builders in `crates/zfb-build/tests/` etc.) keep the same
    /// effective parser behaviour.
    pub gfm_constructs: zfb_content::ResolvedGfmConstructs,

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

    /// Optional TOC config. When `Some`, a [`TocPlugin`] is appended to
    /// the hast phase immediately after `HeadingLinksPlugin` so it reads
    /// final deduplicated `id` attributes. `None` (the default) leaves
    /// the build byte-for-byte identical to the pre-TOC build.
    ///
    /// Mirrors `zfb::config::Config::markdown.toc`. Threaded alongside
    /// `gfm_constructs` so the snapshot walker and bundler agree on the
    /// same pipeline shape and produce byte-identical JSX `content_hash`
    /// values.
    pub toc: Option<zfb_content::TocConfig>,
    /// External-link rewriter config. When `Some`, the bundler's MDX
    /// pre-compile pipeline appends [`ExternalLinksPlugin`] so external
    /// `<a>` elements are annotated with `target` / `rel`. `None` (the
    /// default) keeps prior behaviour unchanged.
    ///
    /// Mirrors `markdown.externalLinks` in `zfb.config.ts`. MUST match
    /// what [`zfb_content::SnapshotPipelineConfig::external_links`] is
    /// set to — divergence shifts the JSX `content_hash` and breaks the
    /// snapshot ↔ bundler bridge lookup (the land mine at
    /// `crates/zfb-content/src/content_bridge.rs:118-153`).
    pub external_links: Option<(zfb_content::ExternalLinksConfig, Option<String>)>,
    /// Whether to include [`CjkFriendlyPlugin`] in the mdast phase.
    /// Mirrors `zfb::config::resolve_cjk_friendly(config.markdown)`.
    /// Default: `true` (plugin on). Set to `false` only when the user
    /// wrote `markdown: { cjkFriendly: false }` in `zfb.config.ts`.
    ///
    /// Must match the `SnapshotPipelineConfig::cjk_friendly` value used
    /// by the snapshot walker — otherwise the `content_hash` baked into
    /// every compiled MDX module diverges and every
    /// `<Content />` lookup falls back to
    /// `<pre data-zfb-content-fallback>`.
    ///
    /// [`CjkFriendlyPlugin`]: zfb_content::plugins::CjkFriendlyPlugin
    pub cjk_friendly: bool,

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
}

impl BundlerInput {
    /// Construct a `BundlerInput` with the shared project-wide defaults,
    /// overriding only the fields that differ per command.
    ///
    /// Shared defaults:
    /// - Standard relative directory names (`pages`, `content`, `components`,
    ///   `layouts`).
    /// - Empty `define_vars`, `tsconfig_paths`, `external`.
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
            content_dir: PathBuf::from("content"),
            content_collections: Vec::new(),
            components_dir: PathBuf::from("components"),
            layouts_dir: PathBuf::from("layouts"),
            framework,
            define_vars: Default::default(),
            tsconfig_paths: Default::default(),
            external: Vec::new(),
            outdir,
            mode,
            minify: false,
            esbuild_binary: None,
            mock_subprocess_output: None,
            content_snapshot_json,
            node_modules_dir: None,
            node_modules_preserve_symlinks: false,
            strip_md_ext: false,
            code_highlight_theme: None,
            code_highlight_themes_dir: None,
            resolve_markdown_links: None,
            gfm_constructs: zfb_content::ResolvedGfmConstructs::default(),
            site: None,
            prefetch_disabled: false,
            toc: None,
            external_links: None,
            cjk_friendly: true,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            worker_only_routes: None,
            bundle_basename: None,
            css_module_class_maps: HashMap::new(),
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
    /// Key used in the bundle's `routes` object literal. Equal to
    /// `route` — kept as a separate field so future schemes (e.g. a
    /// derived symbol identifier) can change without breaking the
    /// route-string contract.
    pub entry_key: String,
    /// When `true`, this route was produced from a `.html` source file.
    /// It is recorded in the manifest for plugin consumers but is NOT
    /// compiled into the JS bundle — it bypasses esbuild and V8 render
    /// entirely. The build pipeline copies the source body verbatim.
    #[serde(default)]
    pub static_html: bool,
}

const SHADOW_HYDRATE_FILENAME: &str = "__zfb_internal_hydrate.jsx";
const SHADOW_ENTRY_FILENAME: &str = "entry.mjs";
const SHADOW_TSCONFIG_FILENAME: &str = "tsconfig.json";

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
///   *scoped class-name map*, not empty. The bundler rewrites each
///   such file in the shadow tree to a JS module
///   (`export default { "orig": "scoped", … }`) using the maps in
///   [`BundlerInput::css_module_class_maps`]; this loader tells
///   esbuild to parse that rewritten file as JS. esbuild matches the
///   **longest** file extension, so `.module.css` wins over `.css`
///   here — plain `.css` still routes to `=empty`. The scoped CSS
///   itself is shipped externally via `styles-<hash>.css` exactly
///   like Tailwind output. When a `.module.css` file has no map entry
///   (e.g. the CSS pipeline was not run), the shadow file keeps its
///   raw CSS bytes and this loader makes esbuild fail with a clear
///   parse error rather than silently dropping the import — which is
///   the correct fail-fast signal for a misconfigured build.
pub const ESBUILD_LOADER_ARGS: &[&str] =
    &["--loader:.mdx=jsx", "--loader:.md=jsx", "--loader:.css=empty", "--loader:.module.css=js"];

/// Default release-tarball slot for the esbuild binary. Mirrors
/// `zfb_islands::EsbuildSubprocessConfig::default`'s default — kept in
/// sync deliberately, both crates resolve the same slot.
const DEFAULT_ESBUILD_SLOT: &str = "crates/zfb/binaries/esbuild/esbuild";

/// Bundle the user's source tree into a single ESM file.
///
/// See the module-level documentation for the full pipeline.
pub fn bundle(input: BundlerInput) -> Result<BundlerOutput> {
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

    // 1b. Build the resolve-links source map when the feature is enabled.
    //
    // The map is built once here (before the shadow walk) from every
    // configured route and shared across all materialise calls via a
    // reference. An empty map is used when the feature is disabled so
    // the materialise helpers can always receive a reference without
    // conditional logic at each call site.
    //
    // Multi-route shape (sub #234) lets locale mirrors map to distinct
    // route prefixes — required for any project with EN+JA mirrors, or
    // any other multi-collection layout, so each mirror dir resolves
    // under its own route prefix (`/docs/` vs `/ja/docs/`).
    let resolve_source_map: HashMap<std::path::PathBuf, String> =
        if let Some(spec) = &input.resolve_markdown_links {
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
        } else {
            HashMap::new()
        };
    let resolve_links_enabled = input.resolve_markdown_links.is_some();

    // Accumulated broken links across all materialise calls.
    // After the walk completes, handled according to `on_broken_links`.
    let mut all_broken_links: Vec<(String, String)> = Vec::new(); // (file_path, url)

    // 2. Materialise the shadow tree.
    let work = tempfile::Builder::new()
        .prefix("zfb-bundler-")
        .tempdir()
        .context("bundler: failed to allocate shadow tempdir")?;
    let shadow = work.path();

    let shadow_pages = shadow.join("pages");
    let shadow_content = shadow.join("content");
    let shadow_components = shadow.join("components");
    let shadow_layouts = shadow.join("layouts");

    let mut routes: Vec<RouteEntry> = Vec::new();
    {
        let mut broken = Vec::new();
        materialise_shadow(
            &pages_dir,
            &shadow_pages,
            &mut routes,
            &input.project_root,
            input.strip_md_ext,
            input.code_highlight_theme.as_deref(),
            input.code_highlight_themes_dir.as_deref(),
            if resolve_links_enabled { Some(&resolve_source_map) } else { None },
            input.gfm_constructs,
            input.toc.clone(),
            input.external_links.as_ref(),
            input.cjk_friendly,
            &mut broken,
        )
        .with_context(|| format!("bundler: failed materialising pages from {}", pages_dir.display()))?;
        all_broken_links.extend(broken);
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
            materialise_collection(
                &col_root,
                &dest,
                &col.name,
                &mut content_imports,
                input.strip_md_ext,
                input.code_highlight_theme.as_deref(),
                input.code_highlight_themes_dir.as_deref(),
                if resolve_links_enabled { Some(&resolve_source_map) } else { None },
                input.gfm_constructs,
                input.toc.clone(),
                input.external_links.as_ref(),
                input.cjk_friendly,
                col.include.as_deref(),
                col.exclude.as_deref(),
                col.id_strip_suffix.as_deref(),
                &mut broken,
            )
            .with_context(|| {
                format!(
                    "bundler: failed materialising collection `{}` from {}",
                    col.name,
                    col_root.display()
                )
            })?;
            all_broken_links.extend(broken);
        }
        // Deterministic ordering — keys are `(collection, rel_path)`
        // so the emitted import indices match the underlying file
        // tree on every build, regardless of WalkDir's per-OS order.
        content_imports.sort_by(|a, b| a.shadow_rel_path.cmp(&b.shadow_rel_path));
    } else {
        let mut broken = Vec::new();
        materialise_shadow(
            &content_dir,
            &shadow_content,
            &mut Vec::new(),
            &input.project_root,
            input.strip_md_ext,
            input.code_highlight_theme.as_deref(),
            input.code_highlight_themes_dir.as_deref(),
            if resolve_links_enabled { Some(&resolve_source_map) } else { None },
            input.gfm_constructs,
            input.toc.clone(),
            input.external_links.as_ref(),
            input.cjk_friendly,
            &mut broken,
        )
        .with_context(|| {
            format!(
                "bundler: failed materialising content from {}",
                content_dir.display()
            )
        })?;
        all_broken_links.extend(broken);
    }

    {
        let mut broken = Vec::new();
        materialise_shadow(
            &components_dir,
            &shadow_components,
            &mut Vec::new(),
            &input.project_root,
            input.strip_md_ext,
            input.code_highlight_theme.as_deref(),
            input.code_highlight_themes_dir.as_deref(),
            if resolve_links_enabled { Some(&resolve_source_map) } else { None },
            input.gfm_constructs,
            input.toc.clone(),
            input.external_links.as_ref(),
            input.cjk_friendly,
            &mut broken,
        )
        .with_context(|| {
            format!(
                "bundler: failed materialising components from {}",
                components_dir.display()
            )
        })?;
        all_broken_links.extend(broken);
    }
    {
        let mut broken = Vec::new();
        materialise_shadow(
            &layouts_dir,
            &shadow_layouts,
            &mut Vec::new(),
            &input.project_root,
            input.strip_md_ext,
            input.code_highlight_theme.as_deref(),
            input.code_highlight_themes_dir.as_deref(),
            if resolve_links_enabled { Some(&resolve_source_map) } else { None },
            input.gfm_constructs,
            input.toc.clone(),
            input.external_links.as_ref(),
            input.cjk_friendly,
            &mut broken,
        )
        .with_context(|| {
            format!(
                "bundler: failed materialising layouts from {}",
                layouts_dir.display()
            )
        })?;
        all_broken_links.extend(broken);
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
    // hidden dirs, zfb output dirs) are skipped.
    {
        let known: &[&str] = &["pages", "content", "components", "layouts", "node_modules",
                                "dist", ".git", "target", ".turbo", ".next", ".vercel"];
        if let Ok(rd) = fs::read_dir(&input.project_root) {
            for entry in rd.flatten() {
                let ft = match entry.file_type() {
                    Ok(ft) => ft,
                    Err(_) => continue,
                };
                if !ft.is_dir() {
                    continue;
                }
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                // Skip hidden dirs and known/infrastructure dirs.
                if name_str.starts_with('.') {
                    continue;
                }
                if known.iter().any(|k| name_str == *k) {
                    continue;
                }
                let src_dir = input.project_root.join(&name);
                let dst_dir = shadow.join(&name);
                let mut broken = Vec::new();
                materialise_shadow(
                    &src_dir,
                    &dst_dir,
                    &mut Vec::new(),
                    &input.project_root,
                    input.strip_md_ext,
                    input.code_highlight_theme.as_deref(),
                    input.code_highlight_themes_dir.as_deref(),
                    if resolve_links_enabled { Some(&resolve_source_map) } else { None },
                    input.gfm_constructs,
                    input.toc.clone(),
                    input.external_links.as_ref(),
                    input.cjk_friendly,
                    &mut broken,
                )
                .with_context(|| {
                    format!(
                        "bundler: failed materialising extra dir {} into shadow",
                        src_dir.display()
                    )
                })?;
                all_broken_links.extend(broken);
            }
        }
    }

    // 2b. Optional node_modules symlink into the shadow tree.
    //     When `BundlerInput::node_modules_dir` is set, create a
    //     symlink `<shadow>/node_modules → <path>` so esbuild can
    //     resolve packages from there instead of walking up into an
    //     empty tempdir ancestry.
    if let Some(ref nm_dir) = input.node_modules_dir {
        let shadow_nm = shadow.join("node_modules");
        #[cfg(unix)]
        std::os::unix::fs::symlink(nm_dir, &shadow_nm).with_context(|| {
            format!(
                "bundler: failed to symlink node_modules {} → {}",
                nm_dir.display(),
                shadow_nm.display()
            )
        })?;
        #[cfg(not(unix))]
        {
            // On Windows, attempt a directory junction.
            fs::create_dir_all(&shadow_nm).with_context(|| {
                format!("bundler: failed to create node_modules dir in shadow tree")
            })?;
        }
    }

    // 2c. Handle broken links collected across all materialise calls.
    //
    // All calls ran to completion first so the full set of broken links is
    // reported in one pass (consistent with the `onBrokenLinks: 'error'`
    // contract in the issue spec). Warnings are emitted to stderr so they
    // are visible to both the CLI user and CI log scanners.
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
                bail!("{}", msg);
            }
        }
    }

    // 2d. CSS Modules — rewrite every `.module.css` file in the shadow
    //     tree to a JS module that re-exports its scoped class-name
    //     map as the default export. Paired with the
    //     `--loader:.module.css=js` esbuild flag, this makes a user's
    //     `import styles from "./x.module.css"; styles.foo` resolve to
    //     the scoped class string at bundle time. See the module-level
    //     `ESBUILD_LOADER_ARGS` doc and `BundlerInput::css_module_class_maps`.
    rewrite_css_modules_in_shadow(shadow, &input.project_root, &input.css_module_class_maps)
        .context("bundler: failed rewriting CSS Modules in shadow tree")?;

    // 3. Hydration shim.
    let shim_path = shadow.join(SHADOW_HYDRATE_FILENAME);
    fs::write(&shim_path, adapter.hydrate_shim_source()).with_context(|| {
        format!("bundler: failed writing hydration shim to {}", shim_path.display())
    })?;

    // 4. Synthetic tsconfig.json honouring the user's `paths`.
    write_synthetic_tsconfig(
        shadow,
        &input.tsconfig_paths,
        adapter.jsx_import_source(),
    )
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
        adapter.render_to_string_module(),
        entry_snapshot_for_write,
        entry_content_imports_for_write,
        input.site.as_deref(),
        input.prefetch_disabled,
    )
    .context("bundler: failed writing entry.mjs")?;

    // 6. Resolve and run esbuild (or the mock).
    fs::create_dir_all(&outdir)
        .with_context(|| format!("bundler: failed to create outdir {}", outdir.display()))?;
    // Bundle filename — `bundle_basename` lets callers run two bundle()
    // passes in the same outdir (full SSG vs runtime-only) without clobber.
    let bundle_filename: &str = input
        .bundle_basename
        .as_deref()
        .unwrap_or("bundle.mjs");
    let bundle_path = outdir.join(bundle_filename);
    let sourcemap_path = outdir.join(format!("{bundle_filename}.map"));

    if let Some(mock) = input.mock_subprocess_output.as_ref() {
        fs::write(&bundle_path, mock).with_context(|| {
            format!("bundler: failed to write mock bundle to {}", bundle_path.display())
        })?;
    } else {
        run_esbuild(
            &input,
            shadow,
            &bundle_path,
        )?;
    }

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

    Ok(BundlerOutput {
        bundle_path,
        sourcemap_path,
        manifest,
    })
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
fn materialise_shadow(
    src: &Path,
    dest: &Path,
    routes: &mut Vec<RouteEntry>,
    project_root: &Path,
    strip_md_ext: bool,
    code_highlight_theme: Option<&str>,
    code_highlight_themes_dir: Option<&Path>,
    resolve_source_map: Option<&HashMap<std::path::PathBuf, String>>,
    gfm_constructs: zfb_content::ResolvedGfmConstructs,
    toc: Option<zfb_content::TocConfig>,
    external_links: Option<&(zfb_content::ExternalLinksConfig, Option<String>)>,
    cjk_friendly: bool,
    broken_links_out: &mut Vec<(String, String)>,
) -> Result<()> {
    if !src.exists() {
        // A missing source dir is non-fatal — not every project has e.g.
        // `layouts/`. Just skip; entry.mjs will simply not import from
        // it. This matches the "rebuild more, not less" defensive bias
        // of `zfb-build`'s policy module.
        return Ok(());
    }

    fs::create_dir_all(dest).with_context(|| format!("create dir {}", dest.display()))?;
    // Routes are only collected when the caller passed a `routes` vec
    // they actually intend to fill — by convention, only the call for
    // the pages root does this. We detect "is this the pages call?" by
    // checking the destination directory name (`pages`), not the source
    // name, so a user with a non-conventional source layout (e.g.
    // `src/routes/`) still produces routes correctly because the
    // bundler always materialises the shadow root under `pages/`.
    let is_pages_dir = dest
        .file_name()
        .map(|s| s == "pages")
        .unwrap_or(false);

    // Hoist a single `Pipeline::with_defaults_and_theme()` outside the
    // walk loop so the seven default plugins (admonitions, CJK-friendly
    // emphasis, heading-links, code-title, image-enlarge, mermaid,
    // syntect) all fire on every MDX file the walker visits. The dev
    // loader at `crates/zfb-render/src/loader.rs` reuses one pipeline
    // for the same reason — constructing a `Highlighter` and seven boxed
    // visitors per file would be wasteful. Borrow is linear (`&mut`), so
    // a single hoisted pipeline serves every MDX file sequentially.
    // See zfb#127 / #128.
    //
    // The opt-in `StripMdExtensionPlugin` is appended here when the
    // user enabled `stripMdExt` in `zfb.config.ts` (zfb#127 / #129).
    // The plugin is intentionally NOT in `with_defaults()` because it
    // only makes sense for sites whose authors hand-write
    // `[label](other.md)` style references. The dev loader honours the
    // same flag so dev preview matches built dist.
    //
    // When `resolve_source_map` is `Some`, the `ResolveLinksPlugin` is
    // also wired into the mdast phase after `AdmonitionsPlugin` so
    // author-written `[label](./other.mdx)` links rewrite to the
    // rendered route URL. The `source_dir` is updated per-file below.
    let mut pipeline = if let Some(dir) = code_highlight_themes_dir {
        zfb_content::pipeline::Pipeline::with_defaults_and_theme_and_gfm_and_themes_dir(
            code_highlight_theme,
            gfm_constructs,
            dir,
            cjk_friendly,
        )
        .with_context(|| {
            format!(
                "codeHighlight.themesDir: failed to load themes from {}",
                dir.display()
            )
        })?
    } else {
        zfb_content::pipeline::Pipeline::with_defaults_and_theme_and_gfm_and_cjk(
            code_highlight_theme,
            gfm_constructs,
            cjk_friendly,
        )
    };
    if let Some(toc_cfg) = toc {
        pipeline.add_toc(toc_cfg);
    }
    if strip_md_ext {
        pipeline.add_strip_md_ext();
    }
    if let Some(map) = resolve_source_map {
        pipeline.add_resolve_links(map.clone());
    }
    if let Some((cfg, site)) = external_links {
        pipeline.add_external_links(cfg.clone(), site.as_deref());
    }

    // sort_by_file_name() gives lexicographic order within each directory
    // level, matching walk_collection's explicit files.sort() contract so
    // the two walks feed entries to their respective Pipeline instances in
    // the same order.  Without sorting the OS-supplied readdir order is
    // non-deterministic and HeadingLinksPlugin's slug counter can assign
    // "basic-usage-7" in one walk and "basic-usage" in the other,
    // producing different content_hash values and breaking the
    // mdx://<collection>/<slug>#<hash> bridge lookup (zfb#187).
    for entry in WalkDir::new(src).follow_links(false).sort_by_file_name() {
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
            fs::create_dir_all(&to)
                .with_context(|| format!("create dir {}", to.display()))?;
            continue;
        }
        if !entry.file_type().is_file() {
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
                let abs_src = from.to_path_buf();
                let project_rel = abs_src
                    .strip_prefix(project_root)
                    .map(|p| p.to_path_buf())
                    .unwrap_or(abs_src);
                routes.push(RouteEntry {
                    route: route.clone(),
                    source_path: project_rel,
                    entry_key: route,
                    static_html: true,
                });
            }
            continue;
        }

        if is_mdx {
            // Reset per-document state (e.g. HeadingLinksPlugin's slug
            // counter) before each new MDX file so cross-document state
            // cannot leak and alter content_hash (zfb#187).
            pipeline.reset_per_entry();
            // Update per-file source_dir for ResolveLinksPlugin so
            // relative links like `./other.mdx` resolve correctly.
            if resolve_source_map.is_some() {
                if let Some(parent) = from.parent() {
                    pipeline.set_resolve_links_source_dir(parent.to_path_buf());
                }
            }
            let raw = fs::read_to_string(from)
                .with_context(|| format!("read mdx {}", from.display()))?;
            let body = strip_yaml_frontmatter(&raw);
            let compiled = compile_mdx_to_jsx_module_cached(body, from, None, Some(&mut pipeline))
                .with_context(|| format!("compile mdx {}", from.display()))?;
            // Drain broken-link diagnostics and record them with the file path.
            for diag in pipeline.take_broken_links() {
                broken_links_out.push((from.display().to_string(), diag.url));
            }
            fs::write(&to, compiled.jsx_source.as_bytes())
                .with_context(|| format!("write compiled mdx to {}", to.display()))?;
        } else if is_md && is_pages_dir {
            // .md page: compile via the MDX pipeline then wrap in a minimal
            // HTML shell.  The compiled body is written to a `_`-prefixed
            // sibling so `derive_route` skips it; the shell module at the
            // original `.md` shadow path becomes the page module esbuild
            // bundles and the router serves.
            pipeline.reset_per_entry();
            if resolve_source_map.is_some() {
                if let Some(parent) = from.parent() {
                    pipeline.set_resolve_links_source_dir(parent.to_path_buf());
                }
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
            let compiled =
                compile_mdx_to_jsx_module_cached(&md_body, from, None, Some(&mut pipeline))
                    .with_context(|| format!("compile md page {}", from.display()))?;
            for diag in pipeline.take_broken_links() {
                broken_links_out.push((from.display().to_string(), diag.url));
            }
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
            let stem = from
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("page");
            let body_filename = format!("_zfb_md_body_{stem}.jsx");
            let body_shadow_path = to
                .parent()
                .map(|p| p.join(&body_filename))
                .unwrap_or_else(|| PathBuf::from(&body_filename));
            fs::write(&body_shadow_path, compiled.jsx_source.as_bytes())
                .with_context(|| {
                    format!("write md body to {}", body_shadow_path.display())
                })?;
            // Shell module at the original `.md` shadow path.
            // Prefix the body import with "./" so esbuild resolves it as a
            // relative path (bare names are interpreted as package specifiers).
            let body_import = format!("./{body_filename}");
            let shell = render_md_page_shell(&frontmatter_value, &slug_fallback, &body_import);
            fs::write(&to, shell.as_bytes())
                .with_context(|| format!("write md page shell to {}", to.display()))?;
        } else {
            fs::copy(from, &to)
                .with_context(|| format!("copy {} -> {}", from.display(), to.display()))?;
        }

        // Routes only collected from the pages root.
        if is_pages_dir {
            if let Some(route) = derive_route(rel) {
                let abs_src = from.to_path_buf();
                let project_rel = abs_src
                    .strip_prefix(project_root)
                    .map(|p| p.to_path_buf())
                    .unwrap_or(abs_src);
                routes.push(RouteEntry {
                    route: route.clone(),
                    source_path: project_rel,
                    entry_key: route,
                    static_html: false,
                });
            }
        }
    }

    // Sort routes so that Hono registers more-specific routes first.
    //
    // Hono dispatches requests in registration order. Without explicit
    // ordering a fully-dynamic route like `/[lang]/[slug]` (→ `/:lang/:slug`)
    // registered BEFORE `/blog/[slug]` would steal `/blog/hello` by matching
    // it as (lang=blog, slug=hello). We prevent this by sorting from most-
    // specific to least-specific using a composite key:
    //
    //   (−static_segments, +dynamic_segments, +catchall_segments)
    //
    // Interpretation:
    //   - More static segments → lower (higher priority) primary key.
    //   - Among same static count, fewer dynamic → lower secondary key.
    //   - Catchall (rest) segments always sort after plain dynamic ones.
    //   - Alphabetical order breaks remaining ties (stable and deterministic).
    //
    // Example ordering for the routing-rendering fixture:
    //   /              → (0, 0, 0) — static, most specific
    //   /about         → (−1, 0, 0)
    //   /blog          → (−1, 0, 0) — tie broken alphabetically
    //   /blog/page/[p] → (−2, 1, 0) — 2 static segs, 1 dynamic
    //   /blog/[slug]   → (−1, 1, 0) — 1 static seg, 1 dynamic
    //   /docs/[...s]   → (−1, 0, 1) — 1 static seg, 1 catchall
    //   /[lang]/[slug] → (0, 2, 0)  — 0 static segs, 2 dynamic (least specific)
    //
    // Using isize allows negative values for the static component, which is
    // what we want — we want "more static" to sort EARLIER (lower).
    fn route_sort_key(route: &str) -> (isize, isize, isize) {
        let mut static_count = 0isize;
        let mut dynamic_count = 0isize;
        let mut catchall_count = 0isize;
        for seg in route.split('/') {
            if seg.is_empty() {
                continue; // leading slash
            }
            if seg.starts_with("[...") && seg.ends_with(']') {
                catchall_count += 1;
            } else if seg.starts_with('[') && seg.ends_with(']') {
                dynamic_count += 1;
            } else {
                static_count += 1;
            }
        }
        // Negate static_count so higher static count → lower (earlier) key.
        (-static_count, dynamic_count, catchall_count)
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
) -> Result<()> {
    for entry in WalkDir::new(shadow).follow_links(false) {
        let entry = entry.with_context(|| format!("walking shadow {}", shadow.display()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
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
        fs::write(path, js.as_bytes()).with_context(|| {
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
    strip_md_ext: bool,
    code_highlight_theme: Option<&str>,
    code_highlight_themes_dir: Option<&Path>,
    resolve_source_map: Option<&HashMap<std::path::PathBuf, String>>,
    gfm_constructs: zfb_content::ResolvedGfmConstructs,
    toc: Option<zfb_content::TocConfig>,
    external_links: Option<&(zfb_content::ExternalLinksConfig, Option<String>)>,
    cjk_friendly: bool,
    include: Option<&[String]>,
    exclude: Option<&[String]>,
    id_strip_suffix: Option<&str>,
    broken_links_out: &mut Vec<(String, String)>,
) -> Result<()> {
    if !src.exists() {
        return Ok(());
    }
    fs::create_dir_all(dest)
        .with_context(|| format!("create dir {}", dest.display()))?;

    // Compile the include / exclude globs once per collection. The
    // shared `CollectionFilter` MUST match `CollectionConfig::*` on the
    // snapshot side byte-for-byte — both surfaces feed the same JSON
    // bridge, so any divergence in which files survive (or which slug
    // each gets) silently turns every consumer-side `getCollection` /
    // `<Content />` lookup into a `<pre data-zfb-content-fallback>`
    // block.
    let filter = zfb_content::collection::CollectionFilter::new(
        include,
        exclude,
        id_strip_suffix,
    )
    .with_context(|| {
        format!(
            "bundler: failed to compile collection filter for `{}`",
            collection_name
        )
    })?;
    let has_glob_filter = include.map(|p| !p.is_empty()).unwrap_or(false)
        || exclude.map(|p| !p.is_empty()).unwrap_or(false);
    // Re-derive the same suffix `CollectionFilter` would have stored
    // (empty / whitespace → None) so the bundler's specifier rewrite
    // and the walker's rewrite agree on whether to strip.
    let strip_suffix = id_strip_suffix
        .map(str::trim)
        .filter(|s| !s.is_empty());

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
    // also wired after `AdmonitionsPlugin` in the mdast phase. The
    // `source_dir` is updated per-file inside the walk loop.
    let mut pipeline = if let Some(dir) = code_highlight_themes_dir {
        zfb_content::pipeline::Pipeline::with_defaults_and_theme_and_gfm_and_themes_dir(
            code_highlight_theme,
            gfm_constructs,
            dir,
            cjk_friendly,
        )
        .with_context(|| {
            format!(
                "codeHighlight.themesDir: failed to load themes from {}",
                dir.display()
            )
        })?
    } else {
        zfb_content::pipeline::Pipeline::with_defaults_and_theme_and_gfm_and_cjk(
            code_highlight_theme,
            gfm_constructs,
            cjk_friendly,
        )
    };
    if let Some(toc_cfg) = toc {
        pipeline.add_toc(toc_cfg);
    }
    if strip_md_ext {
        pipeline.add_strip_md_ext();
    }
    if let Some(map) = resolve_source_map {
        pipeline.add_resolve_links(map.clone());
    }
    if let Some((cfg, site)) = external_links {
        pipeline.add_external_links(cfg.clone(), site.as_deref());
    }

    // sort_by_file_name() gives lexicographic order within each directory
    // level, matching walk_collection's explicit files.sort() contract so
    // the two walks feed entries to their respective Pipeline instances in
    // the same order (zfb#187).
    for entry in WalkDir::new(src).follow_links(false).sort_by_file_name() {
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
            fs::create_dir_all(&to)
                .with_context(|| format!("create dir {}", to.display()))?;
            continue;
        }
        if !entry.file_type().is_file() {
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
        if is_markdown {
            // Reset per-document state (e.g. HeadingLinksPlugin's slug
            // counter) before each new MDX file (zfb#187).
            pipeline.reset_per_entry();
            // Update per-file source_dir for ResolveLinksPlugin.
            if resolve_source_map.is_some() {
                if let Some(parent) = from.parent() {
                    pipeline.set_resolve_links_source_dir(parent.to_path_buf());
                }
            }
            let raw = fs::read_to_string(from)
                .with_context(|| format!("read mdx {}", from.display()))?;
            // Use `zfb_content::frontmatter::extract` rather than the
            // local `strip_yaml_frontmatter` helper so the body fed
            // into the compiler is **byte-for-byte identical** to the
            // body that `zfb_content::collection::walk_collection`
            // (and therefore `zfb_content::build_snapshot`) would
            // pass. The two helpers have subtly different leading-
            // newline handling — `strip_yaml_frontmatter` greedily
            // trims `\r`/`\n` after the closing `---`, dropping the
            // blank-line separator between frontmatter and body —
            // which yields a different compiled-JSX content_hash and
            // therefore a different `mdx://…#<hash>` specifier than
            // what the snapshot bakes. The bridge map and the
            // snapshot's `module_specifier` field MUST agree on the
            // hash byte-for-byte; otherwise every `bridge.get(spec)`
            // lookup misses and the page renders the raw-markdown
            // fallback.
            let body = match zfb_frontmatter::extract(from, &raw) {
                Ok(uf) => uf.body.unwrap_or_default(),
                Err(_) => {
                    // Frontmatter parse failures fall back to the
                    // local stripper — the snapshot's
                    // `walk_collection` would surface the same error
                    // up its CollectionError path, so missing this
                    // file in the bridge is a no-op (the snapshot
                    // entry is missing too, the page rendering hits
                    // the fallback regardless).
                    strip_yaml_frontmatter(&raw).to_string()
                }
            };
            // Pass the SOURCE path (not the shadow destination) so
            // `compile_mdx_to_jsx_module_cached`'s
            // `collection_and_slug` helper sees the same
            // `(parent_dir, file_stem)` tuple it sees during
            // `zfb_content::build_snapshot` — and therefore bakes the
            // same `mdx://...` specifier into both the snapshot and
            // the bridge map. Mismatch here would make every bridge
            // lookup miss and silently fall back to the
            // raw-markdown <pre> block.
            let compiled = compile_mdx_to_jsx_module_cached(&body, from, None, Some(&mut pipeline))
                .with_context(|| format!("compile mdx {}", from.display()))?;
            // Drain broken-link diagnostics and record them with the file path.
            for diag in pipeline.take_broken_links() {
                broken_links_out.push((from.display().to_string(), diag.url));
            }
            fs::write(&to, compiled.jsx_source.as_bytes())
                .with_context(|| format!("write compiled mdx to {}", to.display()))?;

            // Defensive skip — see [`jsx_likely_breaks_downstream_parser`].
            // The original trigger for this guard was `remark-math`
            // `$$...$$` blocks leaking into the JSX as bare expression
            // containers like `{\infty}` (zfb#93). The emitter now
            // recognises `Math` / `InlineMath` mdast nodes natively
            // (see `crates/zfb-content/src/mdx_jsx_emit.rs`), so the
            // intended math path no longer trips this heuristic. We
            // keep the skip in place as a defensive net for any
            // future leak — bare `{\foo}` expressions in the emitted
            // JSX are unparseable by esbuild and would abort the
            // whole bundle. When the heuristic does fire, omitting
            // the broken module from the bridge map falls the page
            // back to the `<pre data-zfb-content-fallback>` shape;
            // the shadow file is left on disk so downstream debugging
            // can see the emitter output.
            if jsx_likely_breaks_downstream_parser(&compiled.jsx_source) {
                eprintln!(
                    "zfb bundler: skipping MDX content bridge for {} — compiled JSX contains bare `{{\\letter}}` expressions that esbuild rejects. The page will render via the <pre data-zfb-content-fallback> shape.",
                    from.display(),
                );
                continue;
            }

            let rel_str = path_to_posix_string(rel);
            let shadow_rel_path = format!("content/{}/{}", collection_name, rel_str);
            // Apply `idStripSuffix` to the specifier's slug segment
            // so the bundler's bridge-map key matches the snapshot's
            // `EntrySnapshot::module_specifier` after stripping. The
            // shared helper lives in `zfb-content` so both surfaces
            // share one implementation — divergence here is what the
            // snapshot↔bridge byte-for-byte invariant exists to
            // prevent.
            let specifier = zfb_content::collection::maybe_strip_specifier_suffix(
                &compiled.specifier,
                strip_suffix,
            );
            imports.push(ContentImport {
                specifier,
                shadow_rel_path,
            });
        } else {
            fs::copy(from, &to)
                .with_context(|| format!("copy {} -> {}", from.display(), to.display()))?;
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
            if j + 1 < bytes.len()
                && bytes[j] == b'\\'
                && bytes[j + 1].is_ascii_alphabetic()
            {
                return true;
            }
        }

        i += 1;
    }
    false
}

/// Convert a relative `Path` to a forward-slash-separated string. We
/// emit Posix separators unconditionally so the resulting `import`
/// statements are valid on every platform — esbuild on Windows would
/// otherwise reject backslash-bearing module specifiers.
fn path_to_posix_string(p: &Path) -> String {
    p.components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str().map(str::to_owned),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Derive a URL route from a path **relative to** `pages_dir`.
///
/// Returns `None` for non-page files (e.g. an accidental `.txt` inside
/// `pages/`). Recognised page extensions: `.tsx`, `.ts`, `.jsx`, `.js`,
/// `.mdx`, `.md`, `.html`. Files starting with `_` are treated as private
/// (skipped) to match the conventional Next/Astro/Remix behaviour.
fn derive_route(rel: &Path) -> Option<String> {
    let ext = rel.extension().and_then(|s| s.to_str())?;
    if !matches!(ext, "tsx" | "ts" | "jsx" | "js" | "mdx" | "md" | "html") {
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

/// Generate the `entry.mjs` module that re-exports `routes`,
/// `hydrateIsland`, and a Workers-style `default { fetch }` wrapper
/// driven by `createPageRouter` from `@takazudo/zfb-runtime`. This is
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
    render_to_string_module: &str,
    content_snapshot_json: Option<&str>,
    content_imports: &[ContentImport],
    site: Option<&str>,
    prefetch_disabled: bool,
) -> Result<()> {
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
    src.push_str("import { createPageRouter } from \"@takazudo/zfb-runtime\";\n");
    writeln!(
        &mut src,
        "import {{ renderToString as __zfb_renderToString }} from {spec};",
        spec = json_str(render_to_string_module),
    )
    .unwrap();

    // Stable per-route import alias so mangled-letter routes still
    // produce a valid identifier.
    for (idx, route) in js_routes.iter().enumerate() {
        // Convert source_path back to its position under shadow/pages.
        // RouteEntry::source_path is project-relative; the shadow page
        // mirrors the path under shadow/pages, **but** with the `pages/`
        // prefix replaced. We only need the path *under* pages_dir.
        let rel_under_pages = route_path_under_pages(&route.source_path);
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
            .filter(|v| {
                v.is_object()
                    && v.get("collections").is_some_and(|c| c.is_object())
            })
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
    fs::write(&path, src.as_bytes())
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Heuristic to recover "path under pages/" from a project-relative
/// page path. We assume `source_path` starts with `pages/` (since the
/// pages-dir walk pushed RouteEntries with project-relative source
/// paths). If for some reason it doesn't, fall back to the file name.
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

fn json_str(s: &str) -> String {
    serde_json::Value::String(s.to_string()).to_string()
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
        if segment.starts_with("[...") && segment.ends_with(']') {
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

/// Resolve and run the esbuild subprocess.
fn run_esbuild(
    input: &BundlerInput,
    shadow: &Path,
    bundle_path: &Path,
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
    for arg in ESBUILD_LOADER_ARGS {
        cmd.arg(arg);
    }

    if input.mode.is_prod() && input.minify {
        cmd.arg("--minify");
    }

    cmd.arg(format!("--tsconfig={}", tsconfig.display()));
    cmd.arg(format!("--outfile={}", bundle_path.display()));

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
    // via `--tsconfig=<tsconfig.json>` above, NOT as `--alias` flags.
    //
    // Why not `--alias`: esbuild's `--alias:<from>=<to>` is
    // prefix-with-slash — registering `@/foo` would silently also
    // rewrite `@/foo/bar`, contradicting the documented exact-match
    // contract honored by the embedded V8 host
    // (`zfb-render::BundleModuleLoader::resolve_alias`). A
    // `compilerOptions.paths` entry without the wildcard suffix is a
    // literal exact match in the TypeScript / esbuild path-mapping
    // pipeline.
    //
    // `zfb_plugin_resolver::build_resolver_inputs` materializes each
    // virtual module to a `.zfb-virtual-*.mjs` temp file inside
    // `shadow` (so esbuild's upward `node_modules` walk still finds
    // the right packages) and returns POSIX-normalized
    // `(specifier, absolute-path)` pairs. The `NamedTempFile` handles
    // live inside `resolver_inputs._temp_files` and are dropped after
    // the subprocess returns.
    let resolver_inputs = zfb_plugin_resolver::build_resolver_inputs(
        &input.plugin_alias_entries,
        &input.plugin_virtual_modules,
        shadow,
    )
    .context("bundler: failed materializing plugin resolver inputs")?;

    // Rewrite the synthetic tsconfig with the merged path map. Step 4
    // above already wrote a tsconfig honouring `input.tsconfig_paths`
    // alone; merge plugin entries on top (user-wins on key collision —
    // see `merge_into_tsconfig_paths` doc) and rewrite. The mock-
    // subprocess path skips this whole function, so the no-plugin /
    // unit-test path is byte-identical to the previous behaviour.
    let mut merged_paths = input.tsconfig_paths.clone();
    zfb_plugin_resolver::merge_into_tsconfig_paths(
        &mut merged_paths,
        &resolver_inputs.paths_entries,
    );
    // Recreate the adapter to get `jsx_import_source` — cheap (adapters
    // are zero-state) and avoids threading another parameter
    // through `run_esbuild`. Stays in sync with step 4 above so a
    // future framework switch can't make the two writes diverge.
    let jsx_import_source = make_adapter(input.framework).jsx_import_source().to_string();
    write_synthetic_tsconfig(shadow, &merged_paths, &jsx_import_source)
        .context("bundler: failed rewriting synthetic tsconfig with plugin entries")?;

    // import.meta.env.{PROD,DEV} — always emitted, driven by mode.
    let prod = input.mode.is_prod();
    cmd.arg(format!("--define:import.meta.env.PROD={}", prod));
    cmd.arg(format!("--define:import.meta.env.DEV={}", !prod));

    // PUBLIC_-prefixed env vars only. Anything else is dropped server-
    // side and never reaches the bundle.
    for (k, v) in &input.define_vars {
        if !k.starts_with("PUBLIC_") {
            continue;
        }
        // Both `process.env.PUBLIC_X` and `import.meta.env.PUBLIC_X` are
        // common spellings; emit both so user code is not forced to pick.
        let json_v = json_str(v);
        cmd.arg(format!("--define:process.env.{}={}", k, json_v));
        cmd.arg(format!("--define:import.meta.env.{}={}", k, json_v));
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

    // When a custom `node_modules_dir` is injected (test fixture mode),
    // packages are symlinked into the shadow tree rather than physically
    // present. Without `--preserve-symlinks` esbuild resolves imports from
    // the **real** (symlink-target) directory, causing it to walk up into
    // the source tree and miss the custom node_modules. With
    // `--preserve-symlinks` resolution stays anchored at the symlink
    // location inside the shadow tree, so `hono`, `preact`, etc. are found
    // in the injected node_modules even when the package source lives in a
    // different tree (e.g. the worktree's packages/ directory).
    if input.node_modules_dir.is_some() && input.node_modules_preserve_symlinks {
        cmd.arg("--preserve-symlinks");
    }

    cmd.arg(OsString::from(entry));

    let output = cmd
        .output()
        .with_context(|| format!("failed to spawn {}", bin.display()))?;
    // Drop `resolver_inputs` now — the subprocess has finished and
    // esbuild no longer needs the virtual-module `.mjs` temp files.
    // Explicit drop makes the lifetime intent visible; the
    // `NamedTempFile`s inside `_temp_files` delete themselves via
    // their Drop impl.
    drop(resolver_inputs);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "bundler: esbuild exited with status {}: {}",
            output.status,
            stderr.trim()
        );
    }
    Ok(())
}

fn resolve_esbuild_binary(explicit: Option<&Path>) -> Result<PathBuf> {
    resolve_esbuild_binary_with_env(explicit, |name| std::env::var_os(name), None)
}

/// Same as [`resolve_esbuild_binary`] but the env lookup is delegated to
/// a getter closure and the default slot path is overridable. Tests use
/// these escape hatches to drive the `ZFB_ESBUILD_BIN` resolution path
/// without mutating the real process environment or chdir-ing
/// (`std::env::set_var` is `unsafe` under Rust 2024 because it races
/// other threads reading the env table, and our test suite is
/// multi-threaded; chdir has the same problem).
///
/// `slot_override` lets tests point the slot at a path inside a tempdir
/// instead of relying on the real `DEFAULT_ESBUILD_SLOT` happening to
/// be absent in CWD.
fn resolve_esbuild_binary_with_env<F>(
    explicit: Option<&Path>,
    env_getter: F,
    slot_override: Option<&Path>,
) -> Result<PathBuf>
where
    F: Fn(&str) -> Option<std::ffi::OsString>,
{
    if let Some(p) = explicit {
        if !p.exists() {
            bail!(
                "bundler: esbuild binary not found at explicit path {}",
                p.display()
            );
        }
        return Ok(p.to_path_buf());
    }
    if let Some(env) = env_getter("ZFB_ESBUILD_BIN") {
        let p = PathBuf::from(env);
        if !p.exists() {
            bail!(
                "bundler: esbuild binary not found at ZFB_ESBUILD_BIN={}",
                p.display()
            );
        }
        return Ok(p);
    }
    let slot = slot_override
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(DEFAULT_ESBUILD_SLOT));
    if !slot.exists() {
        return Err(anyhow!(
            "bundler: esbuild binary not found at default slot {}. \
             Either set ZFB_ESBUILD_BIN to a usable esbuild CLI, or stage \
             the binary at the slot path. The release-engineering epic \
             that downloads it has not landed yet (see \
             crates/zfb/binaries/esbuild/README.md).",
            slot.display(),
        ));
    }
    Ok(slot)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        maps.insert(
            project_root.join("components/card.module.css"),
            names,
        );

        rewrite_css_modules_in_shadow(shadow_root, project_root, &maps).unwrap();

        let mapped =
            fs::read_to_string(shadow_root.join("components/card.module.css")).unwrap();
        assert!(mapped.contains("sc0_card"), "mapped module: {mapped}");
        assert!(!mapped.contains("color: red"), "raw CSS must be gone");

        let orphan =
            fs::read_to_string(shadow_root.join("styles/orphan.module.css")).unwrap();
        assert_eq!(orphan, "export default {};\n", "unmapped module degrades to {{}}");

        let global = fs::read_to_string(shadow_root.join("styles/global.css")).unwrap();
        assert_eq!(global, ".g{}", "plain .css must be untouched");
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
            content_dir: PathBuf::from("content"),
            content_collections: Vec::new(),
            components_dir: PathBuf::from("components"),
            layouts_dir: PathBuf::from("layouts"),
            framework: Framework::Preact,
            define_vars: HashMap::new(),
            tsconfig_paths: BTreeMap::new(),
            external: vec![],
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
            strip_md_ext: false,
            code_highlight_theme: None,
            code_highlight_themes_dir: None,
            resolve_markdown_links: None,
            gfm_constructs: zfb_content::ResolvedGfmConstructs::default(),
            site: None,
            prefetch_disabled: false,
            toc: None,
            external_links: None,
            cjk_friendly: true,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            worker_only_routes: None,
            bundle_basename: None,
            css_module_class_maps: HashMap::new(),
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
        assert_eq!(derive_route(Path::new("post.mdx")).as_deref(), Some("/post"));
        // _private files are skipped.
        assert!(derive_route(Path::new("_dev.tsx")).is_none());
        // Unknown extensions are skipped.
        assert!(derive_route(Path::new("README.txt")).is_none());
        // .md and .html are now accepted page extensions.
        assert_eq!(
            derive_route(Path::new("about.md")).as_deref(),
            Some("/about")
        );
        assert_eq!(
            derive_route(Path::new("index.html")).as_deref(),
            Some("/")
        );
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
            },
            RouteEntry {
                route: "/about".to_string(),
                source_path: PathBuf::from("pages/about.tsx"),
                entry_key: "/about".to_string(),
                static_html: false,
            },
        ];
        write_entry_module(shadow, &routes, "preact-render-to-string", None, &[], None, false).unwrap();

        let body = fs::read_to_string(shadow.join(SHADOW_ENTRY_FILENAME)).unwrap();

        // Imports the runtime factory and the framework's renderToString.
        assert!(
            body.contains("from \"@takazudo/zfb-runtime\""),
            "entry.mjs must import createPageRouter from @takazudo/zfb-runtime; got:\n{body}"
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
        write_entry_module(shadow, &[], "preact-render-to-string", None, &imports, None, false).unwrap();

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
        write_entry_module(shadow, &[], "preact-render-to-string", None, &[], None, false).unwrap();

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
            "preact-render-to-string",
            None,
            &[],
            Some("https://example.com"),
            false,
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
        write_entry_module(shadow, &[], "preact-render-to-string", None, &[], None, false).unwrap();

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
            "preact-render-to-string",
            None,
            &[],
            None,
            true,
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
        write_entry_module(shadow, &[], "preact-render-to-string", None, &[], None, false).unwrap();

        let body = fs::read_to_string(shadow.join(SHADOW_ENTRY_FILENAME)).unwrap();
        assert!(
            !body.contains("globalThis.__zfb.prefetchDisabled"),
            "prefetch_disabled=false → no prefetch setter; got:\n{body}"
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
        materialise_collection(
            &src,
            &dest,
            "docs",
            &mut imports,
            false,
            None,
            None,
            None,
            zfb_content::ResolvedGfmConstructs::default(),
            None,
            None,
            true,
            None,
            None,
            None,
            &mut Vec::new(),
        )
        .unwrap();

        // Two MDX files → two ContentImport records, with stable
        // forward-slash shadow_rel_paths under `content/docs/...`.
        assert_eq!(imports.len(), 2, "expected 2 MDX imports, got {imports:?}");
        let rels: Vec<&str> = imports
            .iter()
            .map(|i| i.shadow_rel_path.as_str())
            .collect();
        assert!(
            rels.iter().any(|r| *r == "content/docs/intro.mdx"),
            "missing top-level intro entry; got: {rels:?}"
        );
        assert!(
            rels.iter()
                .any(|r| *r == "content/docs/getting-started/installation.mdx"),
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
        materialise_collection(
            &src,
            &dest,
            "posts",
            &mut imports,
            false,
            None,
            None,
            None,
            zfb_content::ResolvedGfmConstructs::default(),
            None,
            None,
            true,
            None,
            None,
            None,
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
            "preact-render-to-string",
            None,
            &imports,
            None,
            false,
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
        materialise_collection(
            &tmp.path().join("does-not-exist"),
            &dest,
            "ghost",
            &mut imports,
            false,
            None,
            None,
            None,
            zfb_content::ResolvedGfmConstructs::default(),
            None,
            None,
            true,
            None,
            None,
            None,
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
        write_entry_module(shadow, &[], "react-dom/server", None, &[], None, false).unwrap();

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
        write_entry_module(shadow, &[], "react-dom/server", snapshot, &[], None, false).unwrap();
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
        let mut filter: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
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

    #[test]
    fn server_secrets_are_not_bundled() {
        // Real esbuild test (gated). Verifies a SECRET_ env var never
        // appears in the output, while a PUBLIC_ var does.
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
                const secret = process.env.SECRET_KEY;
                export default function Home() {
                  return apiUrl + " " + secret;
                }
            "#,
        )
        .unwrap();

        let mut defs = HashMap::new();
        defs.insert("PUBLIC_API_URL".into(), "https://example.test".into());
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
        let nm_dir = if zfb_pkg_node_modules
            .join("@takazudo/zfb-runtime")
            .exists()
        {
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
            content_dir: PathBuf::from("content"),
            content_collections: Vec::new(),
            components_dir: PathBuf::from("components"),
            layouts_dir: PathBuf::from("layouts"),
            framework: Framework::Preact,
            define_vars: defs,
            tsconfig_paths: BTreeMap::new(),
            external: vec!["preact".into()],
            outdir: root.join("dist"),
            mode: BundleMode::Production,
            minify: false,
            esbuild_binary: Some(bin),
            mock_subprocess_output: None,
            content_snapshot_json: None,
            node_modules_dir: nm_dir,
            node_modules_preserve_symlinks: false,
            strip_md_ext: false,
            code_highlight_theme: None,
            code_highlight_themes_dir: None,
            resolve_markdown_links: None,
            gfm_constructs: zfb_content::ResolvedGfmConstructs::default(),
            site: None,
            prefetch_disabled: false,
            toc: None,
            external_links: None,
            cjk_friendly: true,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            worker_only_routes: None,
            bundle_basename: None,
            css_module_class_maps: HashMap::new(),
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
    }

    #[test]
    fn missing_esbuild_binary_returns_actionable_error() {
        // When neither an explicit override nor the env var nor the
        // default slot is present, the bundler must error with a
        // pointer to BOTH escape hatches (`ZFB_ESBUILD_BIN` and the
        // release-tarball slot). This keeps operators unstuck.
        //
        // Drive the env path via an injected getter and the slot path
        // via `slot_override` so the test does not mutate `std::env`
        // and does not chdir — both are `unsafe` / racy under a
        // multi-threaded test runner.
        let tmp = tempfile::tempdir().unwrap();
        let missing_slot = tmp.path().join("crates/zfb/binaries/esbuild/esbuild");

        let err = resolve_esbuild_binary_with_env(None, |_| None, Some(&missing_slot))
            .unwrap_err();
        let msg = format!("{err}");

        assert!(msg.contains("ZFB_ESBUILD_BIN"), "msg: {msg}");
        assert!(msg.contains("crates/zfb/binaries/esbuild"), "msg: {msg}");
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
        materialise_shadow(
            &pages,
            &shadow_pages_dest,
            &mut routes,
            &root,
            false,
            None,
            None,
            None,
            zfb_content::ResolvedGfmConstructs::default(),
            None,
            None,
            true,
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
        assert!(idx("/blog/page/[page]") < idx("/blog/[slug]"),
            "/blog/page/[page] should be before /blog/[slug]");
        // /blog/[slug] (1 static) before /[lang]/[slug] (0 static)
        assert!(idx("/blog/[slug]") < idx("/[lang]/[slug]"),
            "/blog/[slug] should be before /[lang]/[slug]");
        // /docs/[...slug] (1 static, catchall) before /[lang]/[slug] (0 static)
        assert!(idx("/docs/[...slug]") < idx("/[lang]/[slug]"),
            "/docs/[...slug] should be before /[lang]/[slug]");
    }

    /// Locate a real esbuild binary for gated integration tests. Order:
    ///
    /// 1. `ZFB_ESBUILD_BIN` env var
    /// 2. `crates/zfb/binaries/esbuild/esbuild` slot relative to the
    ///    workspace root (resolved relative to `CARGO_MANIFEST_DIR`).
    /// 3. `which esbuild` on `$PATH`.
    pub(super) fn locate_real_esbuild() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("ZFB_ESBUILD_BIN") {
            let p = PathBuf::from(p);
            if p.exists() {
                return Some(p);
            }
        }
        // CARGO_MANIFEST_DIR is `crates/zfb-build`.
        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        if let Some(workspace) = here.parent().and_then(|p| p.parent()) {
            let slot = workspace.join("crates/zfb/binaries/esbuild/esbuild");
            if slot.exists() {
                return Some(slot);
            }
        }
        // Fallback to PATH.
        if let Ok(out) = Command::new("which").arg("esbuild").output() {
            if out.status.success() {
                let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let p = PathBuf::from(p);
                if p.exists() {
                    return Some(p);
                }
            }
        }
        None
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
            !ESBUILD_LOADER_ARGS.iter().any(|a| a.contains("external:") && a.contains(".css")),
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
            plugin_alias_entries: vec![
                ("@/foo".to_string(), root.join("src/foo.tsx").to_string_lossy().into_owned()),
            ],
            // Virtual module: `virtual:meta` with inline source.
            plugin_virtual_modules: vec![
                ("virtual:meta".to_string(), "export const version = '1.0.0';\n".to_string()),
            ],
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
        let err = materialise_shadow(
            pages_dir,
            &shadow_pages_dest,
            &mut routes,
            root,
            false,
            None,
            None,
            None,
            zfb_content::ResolvedGfmConstructs::default(),
            None,
            None,
            true,
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
        assert!(shell.contains("<html lang={__lang}>"), "html element with lang; got:\n{shell}");
        assert!(shell.contains("<meta charSet=\"utf-8\" />"), "charset meta; got:\n{shell}");
        assert!(shell.contains("<title>{__title}</title>"), "title element; got:\n{shell}");
        assert!(shell.contains("<MdBody />"), "body component; got:\n{shell}");
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
        materialise_shadow(
            &src,
            &dest,
            &mut routes,
            tmp.path(),
            false,
            None,
            None,
            None,
            zfb_content::ResolvedGfmConstructs::default(),
            None,
            None,
            true,
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
        assert!(body_path.is_file(), "body must exist at _zfb_md_body_about.jsx");

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
        assert!(shell.contains("<MdBody />"), "shell must render <MdBody />; got:\n{shell}");

        // Body contains compiled MDX output.
        let body = fs::read_to_string(&body_path).unwrap();
        assert!(
            body.contains("_createMdxContent"),
            "body must be compiled JSX; got:\n{body}"
        );

        // derive_route must skip the _-prefixed body file (no route for it).
        assert!(
            !routes.iter().any(|r| r.source_path.to_string_lossy().contains("_zfb_md_body")),
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
        materialise_shadow(
            &src,
            &dest,
            &mut routes,
            tmp.path(),
            false,
            None,
            None,
            None,
            zfb_content::ResolvedGfmConstructs::default(),
            None,
            None,
            true,
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
        fs::write(
            src.join("post.md"),
            "---\ntitle: Post\n---\n\n# Post\n",
        )
        .unwrap();

        let dest = tmp.path().join("shadow").join("content");
        let mut routes = Vec::new();
        materialise_shadow(
            &src,
            &dest,
            &mut routes,
            tmp.path(),
            false,
            None,
            None,
            None,
            zfb_content::ResolvedGfmConstructs::default(),
            None,
            None,
            true,
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
        assert!(routes.is_empty(), "content dir must not produce routes; got {routes:?}");
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
        materialise_shadow(
            &src,
            &dest,
            &mut routes,
            tmp.path(),
            false,
            None,
            None,
            None,
            zfb_content::ResolvedGfmConstructs::default(),
            None,
            None,
            true,
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
}
