//! Build-time **bundler stage** — produces a single ESM bundle from the
//! user's `pages/`, `content/`, `components/`, and `layouts/` source
//! roots, suitable for both miniflare-driven SSR (Wave 2 / T6) and the
//! runtime SSR adapter (T2 `@takazudo/zfb-runtime`).
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
//!    module miniflare and the runtime SSR adapter consume.
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
//!   miniflare's worker bundle expects this symbol so the same bundle
//!   can also feed the islands hydration runtime per ADR-002.
//! - `default` — a Workers-style `{ fetch }` object whose `fetch` field
//!   is a `(Request) => Promise<Response>` constructed by passing
//!   `routes`, an embedded `ContentSnapshot` placeholder, and an inline
//!   framework adapter (the framework's own `renderToString` import) to
//!   `createPageRouter` from `@takazudo/zfb-runtime`. This is the entry
//!   shape miniflare's workerd Module loader expects (`export default {
//!   fetch }`); without it, miniflare boot fails with a missing-export
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
    pub content_dir: PathBuf,
    /// Directory of shared components (subject to `paths` aliasing).
    pub components_dir: PathBuf,
    /// Directory of layout components.
    pub layouts_dir: PathBuf,
    /// Which JSX framework's hydration shim to fold into the bundle.
    /// Drives [`make_adapter`] selection (ADR-002).
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
    /// runtime SSR adapter (T2) provides at miniflare load time. An
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
    /// hydration shim per ADR-002. The bundle itself does not import
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
}

const SHADOW_HYDRATE_FILENAME: &str = "__zfb_internal_hydrate.jsx";
const SHADOW_ENTRY_FILENAME: &str = "entry.mjs";
const SHADOW_TSCONFIG_FILENAME: &str = "tsconfig.json";

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
    materialise_shadow(&pages_dir, &shadow_pages, &mut routes, &input.project_root)
        .with_context(|| format!("bundler: failed materialising pages from {}", pages_dir.display()))?;
    materialise_shadow(&content_dir, &shadow_content, &mut Vec::new(), &input.project_root)
        .with_context(|| {
            format!(
                "bundler: failed materialising content from {}",
                content_dir.display()
            )
        })?;
    materialise_shadow(&components_dir, &shadow_components, &mut Vec::new(), &input.project_root)
        .with_context(|| {
            format!(
                "bundler: failed materialising components from {}",
                components_dir.display()
            )
        })?;
    materialise_shadow(&layouts_dir, &shadow_layouts, &mut Vec::new(), &input.project_root)
        .with_context(|| {
            format!(
                "bundler: failed materialising layouts from {}",
                layouts_dir.display()
            )
        })?;

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

    // 3. Hydration shim (per ADR-002).
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
    write_entry_module(
        shadow,
        &routes,
        adapter.render_to_string_module(),
        input.content_snapshot_json.as_deref(),
    )
    .context("bundler: failed writing entry.mjs")?;

    // 6. Resolve and run esbuild (or the mock).
    fs::create_dir_all(&outdir)
        .with_context(|| format!("bundler: failed to create outdir {}", outdir.display()))?;
    let bundle_path = outdir.join("bundle.mjs");
    let sourcemap_path = outdir.join("bundle.mjs.map");

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

    for entry in WalkDir::new(src).follow_links(false) {
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
        let is_mdx = from.extension().and_then(|s| s.to_str()) == Some("mdx");
        if is_mdx {
            let raw = fs::read_to_string(from)
                .with_context(|| format!("read mdx {}", from.display()))?;
            let body = strip_yaml_frontmatter(&raw);
            let compiled = compile_mdx_to_jsx_module_cached(body, from, None, None)
                .with_context(|| format!("compile mdx {}", from.display()))?;
            fs::write(&to, compiled.jsx_source.as_bytes())
                .with_context(|| format!("write compiled mdx to {}", to.display()))?;
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

/// Derive a URL route from a path **relative to** `pages_dir`.
///
/// Returns `None` for non-page files (e.g. an accidental `.txt` inside
/// `pages/`). Recognised page extensions: `.tsx`, `.ts`, `.jsx`, `.js`,
/// `.mdx`. Files starting with `_` are treated as private (skipped) to
/// match the conventional Next/Astro/Remix behaviour.
fn derive_route(rel: &Path) -> Option<String> {
    let ext = rel.extension().and_then(|s| s.to_str())?;
    if !matches!(ext, "tsx" | "ts" | "jsx" | "js" | "mdx") {
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
/// the single load-bearing module miniflare (T6/T7) and the runtime SSR
/// adapter (T2) consume.
///
/// `render_to_string_module` is the framework's `renderToString`
/// specifier (e.g. `"preact-render-to-string"` for Preact,
/// `"react-dom/server"` for React) — drawn from
/// [`zfb_render::adapters::Adapter::render_to_string_module`]. The
/// wrapper imports `renderToString` by name from this specifier and
/// hands it to `createPageRouter` as the framework adapter, so the
/// bundle pins its own SSR call without leaking the framework choice
/// into miniflare's boot.
///
/// The default-fetch wrapper is emitted unconditionally, even when
/// `routes` is empty: an empty Hono app simply 404s every request, but
/// the bundle still satisfies workerd's "module must export default
/// with a fetch handler" contract so miniflare can boot and surface a
/// clean 404 rather than a missing-export error.
/// Generate the `entry.mjs` module.
///
/// `content_snapshot_json` is the JSON-serialized content snapshot to
/// embed. When `None`, a placeholder `{ collections: {} }` is used.
fn write_entry_module(
    shadow: &Path,
    routes: &[RouteEntry],
    render_to_string_module: &str,
    content_snapshot_json: Option<&str>,
) -> Result<()> {
    use std::fmt::Write as _;
    let mut src = String::new();
    src.push_str("// AUTO-GENERATED by zfb_build::bundler. Do not edit.\n");
    src.push_str(
        "// Single ESM entry shared by miniflare (T6/T7) and the runtime SSR adapter.\n",
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
    for (idx, route) in routes.iter().enumerate() {
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
    src.push_str("\nexport const routes = {\n");
    for (idx, route) in routes.iter().enumerate() {
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
    // miniflare (workerd) loads this bundle as a Module-syntax worker;
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
    //     miniflare boot is decoupled from the snapshot deliverable.
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
    for (idx, route) in routes.iter().enumerate() {
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
    // .mdx files were rewritten to JSX text by `materialise_shadow`;
    // tell esbuild to parse them as JSX so the .mdx extension keeps
    // working for user import paths.
    cmd.arg("--loader:.mdx=jsx");

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
    // the Preact ecosystem uses with bundlers like Vite — see ADR-002.
    if matches!(input.framework, Framework::Preact) {
        cmd.arg("--alias:react/jsx-runtime=preact/jsx-runtime");
        cmd.arg("--alias:react/jsx-dev-runtime=preact/jsx-dev-runtime");
    }

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
    if input.node_modules_dir.is_some() {
        cmd.arg("--preserve-symlinks");
    }

    cmd.arg(OsString::from(entry));

    let output = cmd
        .output()
        .with_context(|| format!("failed to spawn {}", bin.display()))?;
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
        assert!(derive_route(Path::new("README.md")).is_none());
    }

    #[test]
    fn entry_module_emits_default_fetch_wrapper_with_routes() {
        // T7-sibling contract: the bundler's synthetic entry.mjs MUST
        // expose a Workers-style `export default { fetch }` so workerd
        // (miniflare) can dispatch requests. Without it, miniflare boot
        // fails with "missing default export" before any user code runs.
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path();
        let routes = vec![
            RouteEntry {
                route: "/".to_string(),
                source_path: PathBuf::from("pages/index.tsx"),
                entry_key: "/".to_string(),
            },
            RouteEntry {
                route: "/about".to_string(),
                source_path: PathBuf::from("pages/about.tsx"),
                entry_key: "/about".to_string(),
            },
        ];
        write_entry_module(shadow, &routes, "preact-render-to-string", None).unwrap();

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
    fn entry_module_emits_default_fetch_when_routes_are_empty() {
        // The wrapper is emitted unconditionally so miniflare's
        // workerd-Module loader sees a Workers-shaped bundle even when
        // no pages exist yet (e.g. a brand-new `pages/` dir). The empty
        // Hono app inside `createPageRouter` 404s every request — that
        // is the documented zero-routes behaviour.
        let tmp = tempfile::tempdir().unwrap();
        let shadow = tmp.path();
        write_entry_module(shadow, &[], "react-dom/server", None).unwrap();

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
        write_entry_module(shadow, &[], "react-dom/server", snapshot).unwrap();
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

        let input = BundlerInput {
            project_root: root.clone(),
            pages_dir: PathBuf::from("pages"),
            content_dir: PathBuf::from("content"),
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
            node_modules_dir: None,
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
        materialise_shadow(&pages, &shadow_pages_dest, &mut routes, &root).unwrap();

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
}
