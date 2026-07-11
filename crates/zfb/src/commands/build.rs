//! `zfb build` command — one-shot production build.
//!
//! Contract:
//!   pub async fn run(args: &crate::cli::BuildArgs) -> anyhow::Result<()>
//!
//! The production output directory resolves with CLI `--outdir` > config
//! `outDir` > default `dist` precedence, relative to the current working
//! directory when it is not absolute.
//!
//! ## Pipeline overview
//!
//! The build wires the underlying crates in this order:
//!
//! 1. [`zfb_router::Router::scan`] enumerates the route table.
//! 2. [`crate::render_pipeline::build_prerender_map`] reads each TSX
//!    page's `export const prerender = …` flag so SSR-only routes
//!    skip the build-time render.
//! 3. [`zfb_build::bundle`] produces the ESM worker bundle for
//!    every page module and content collection in scope.
//! 4. [`zfb_build::renderer::render_all`] (T6) boots the embedded V8
//!    host in-process, drives a dispatch per concrete URL, and writes
//!    the response body to `<outdir>/<url>/index.html`.
//!
//! ### Dynamic-route handling
//!
//! Routes whose template contains `[slug]`, `[...rest]`, etc. are
//! expanded by [`crate::render_pipeline::expand_dynamic_routes`] using
//! the static `paths()` literal extractor in
//! [`zfb_render::paths_extract`]. Pages whose `paths()` returns a
//! JSON-literal array are resolved into one
//! [`zfb_build::renderer::RouteUniverseEntry`] per concrete URL and
//! rendered alongside the static routes. Pages whose `paths()` is not
//! statically resolvable (e.g. it `await`s an `import` or queries a
//! content collection at runtime) are surfaced via
//! [`crate::output::warn`] with the per-page reason and skipped from
//! `<outdir>/`; a follow-up sub-task adds runtime evaluation for those.
//!
//! The other caller contracts (project-root sanity check and
//! `✓ N pages built in X.XXs` summary) are unchanged.

// V8-off (issue #371, sub-task 4.1a): on the `!feature = "embed_v8"`
// path the `pub async fn run` body and `DefaultRunner` are compiled
// out. The imports and helper functions they reference then look
// unused — silence the lints in that configuration so V8-off builds
// stay warning-clean.
#![cfg_attr(not(feature = "embed_v8"), allow(unused_imports, dead_code))]

use std::env;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use zfb_build::adapter::{
    ensure_no_ssr_without_adapter, run_adapter_bundle_with, AdapterBundleInput,
    AdapterBundleOutput, AdapterChoice, AdapterRunner, DefaultAdapterRunner, SsrRouteRef,
};
use zfb_build::bundler::{bundle, BundleMode, BundlerInput, BundlerOutput};
use zfb_build::head_inject::ProdHeadAssets;
use zfb_build::pipeline::{
    apply_prod_asset_pipeline, synthesize_page_id_from_output, AssetEmitterPayload, CompanionFile,
    ProdAssetEmitterInputs, ProdRenderedFile, RelDistPath,
};
use zfb_build::renderer::{render_all, Backend, RendererInput, RendererOutput};
use zfb_css::{
    css_relative_path, is_tailwind_import_line, AuthoredCssEngine, CssEngine, CssPipeline,
    CssPipelineConfig, TailwindSubprocessConfig, TailwindSubprocessEngine,
};
use zfb_islands::{
    build_production_client_scripts_with_workers, build_production_islands_asset,
    discover_client_scripts, scan_islands_with_meta, scan_reachable_modules_with_meta,
    BundleConfig, ClientScriptWorkerEntry, EsbuildSubprocessBundler, EsbuildSubprocessConfig,
    FrameworkKind, FsResolver,
};
use zfb_router::Router;

use zfb_render::paths::PathsCache;

use crate::cli::{BuildArgs, BuildMinifyHtml};
use crate::commands::resolve::{
    resolve_outdir, resolve_outdir_arg, validate_outdir_safety, wipe_outdir_contents,
};
use crate::config::{Config, OutputMode};
use crate::output;
use crate::render_pipeline::{
    build_prerender_map, build_route_universe, check_runtime_installed, embedded_binary,
    embedded_node_modules, eval_deferred_paths_via_worker, expand_dynamic_routes, is_ssr_route,
    DeferredDynamicRoute, DynamicResolvedEntry, RouteUniversePlan, WorkerDispatch,
};

/// Entry point for `zfb build`.
///
/// Available only when the `embed_v8` cargo feature is on (issue #371,
/// sub-task 4.1a). The V8-off counterpart further down in this file
/// surfaces a clear runtime error.
#[cfg(feature = "embed_v8")]
pub async fn run(args: &BuildArgs) -> Result<()> {
    let started = Instant::now();

    let project_root = env::current_dir().context("failed to read current working directory")?;

    // The conventional pages root. The `pages/` dir requirement is
    // relaxed below (#1193): a project with package-owned build routes
    // may ship a truly empty/absent `pages/`, so the hard requirement is
    // re-checked AFTER plugin setup, once we know whether any build
    // routes were registered.
    let pages_dir = project_root.join("pages");

    let config = crate::config::load_from_dir(&project_root)
        .await
        .context("failed to load project configuration")?;
    let minify_html = resolve_minify_html(args.minify_html(), &config);

    let selected_outdir = resolve_outdir_arg(args.outdir.clone(), &config.out_dir);
    let outdir = resolve_outdir(&project_root, &selected_outdir);

    // Sub 3 / #108 — plugin lifecycle. Spawn the host before any heavy
    // work so `preBuild` can prepare files the bundler will see (e.g.
    // claude-resources index emission). If no plugins are declared, we
    // skip the spawn entirely so a config-less project pays nothing.
    let plugin_host = crate::commands::plugins::maybe_spawn_host(&config).await?;

    // #255 / #260 / #261 / #268 — shared plugin setup phase:
    // setup → virtual-module prefetch → alias/virtual-module derivation.
    //
    // `SetupCommand::Build` is the per-command difference (dev uses
    // `SetupCommand::Dev`).  As of #1193, `injectRoute` is ACCEPTED
    // during a build — a registered route becomes a package-owned build
    // route the overlay materialiser prerenders (see below) — rather than
    // the pre-#1193 dev-only hard error.
    let plugin_setup = crate::commands::plugins::run_plugin_setup(
        &plugin_host,
        &project_root,
        &config,
        zfb_build::SetupCommand::Build,
    )
    .await?;

    // Destructure all outputs from the shared setup phase before any
    // moves so the borrow checker is happy.
    let crate::commands::plugins::PluginSetupResult {
        v8_plugin_hooks,
        plugin_alias_entries: main_bundler_alias_entries,
        plugin_virtual_modules: main_bundler_virtual_modules,
        setup_registries,
    } = plugin_setup;

    // #1193 — the package-owned build routes registered during setup. The
    // overlay that materialises them is built AFTER preBuild (below), so a
    // preBuild hook that generates `pages/` files is reflected in both the
    // user-wins precedence check and the merged scan.
    let injected_routes = setup_registries.injected_routes.as_slice();

    // Build the IslandsPluginConfig from the same data — cheap clones since
    // the alias/virtual-module vecs are shared with the main bundler path.
    let islands_plugin_config = IslandsPluginConfig {
        alias_entries: main_bundler_alias_entries.clone(),
        virtual_modules: main_bundler_virtual_modules.clone(),
    };

    // #805 — wipe outdir before preBuild so plugin-emitted files (emitted
    // after this point) always land in a clean directory.
    validate_outdir_safety(&project_root, &outdir).context("outdir safety check failed")?;
    wipe_outdir_contents(&outdir)
        .with_context(|| format!("failed to wipe outdir {}", outdir.display()))?;

    if let Some(host) = plugin_host.as_ref() {
        let ctx = zfb_build::BuildHookContext {
            project_root: project_root.clone(),
            out_dir: outdir.clone(),
            config: serde_json::to_value(&config)
                .context("plugin lifecycle: serialise config for preBuild ctx")?,
            // preBuild: routes absent (undefined in JS) — spec AC for #262.
            routes: None,
        };
        host.run_pre_build(&ctx)
            .await
            .map_err(zfb_build::annotate_with_plugin_error)
            .context("preBuild lifecycle hook")?;
    }

    // #1193 — package-owned routes. Resolve the build pages root: with no
    // build routes this is `project_root/pages` and the overlay machinery
    // is entirely bypassed (byte-identical parity); with build routes it
    // is a per-build temp overlay that copies the user's real `pages/`
    // plus the synthesized package modules (user-`pages/`-wins precedence
    // is enforced inside via a pre-scan shape-key drop). Done AFTER preBuild
    // so any `pages/` files a preBuild hook generated are copied in and seen
    // by the precedence check + merged scan. `_overlay_guard` is the RAII
    // handle for the temp dir — it must outlive the bundle + render + any
    // `paths()` V8 eval, so it stays in scope to end-of-run.
    let overlay =
        crate::commands::package_routes::resolve_build_pages_root(&pages_dir, injected_routes)
            .context("resolving build pages root for package-owned routes")?;
    let build_pages_root = overlay.build_pages_root.clone();
    let _overlay_guard = overlay.guard;

    // Surface which package routes were materialised (and at what overlay
    // path) so the build output is legible when a preset owns routes.
    for mr in &overlay.materialized {
        crate::output::info(format!(
            "package route `{}` → pages/{}",
            mr.pattern,
            mr.pages_rel.display()
        ));
    }

    // codex P1 (#1191 review) — the islands seed must NOT walk the overlay
    // for user pages (the overlay has no `components/`). Collect the REAL
    // package-route entrypoints to seed package-route island discovery; user
    // pages are seeded from the real `pages_dir` below. Empty on the
    // no-package-route parity path.
    let package_route_entrypoints: Vec<PathBuf> = overlay
        .materialized
        .iter()
        .map(|mr| mr.entrypoint.clone())
        .collect();

    // Project-root sanity check, relaxed for package-owned routes (#1193):
    // a project with build routes may ship an empty/absent `pages/` (the
    // overlay is built from package routes alone). When there are no build
    // routes, the conventional `pages/` dir is still required.
    if !build_pages_root.is_dir() {
        return Err(anyhow!(
            "no `pages/` directory found in {}; run `zfb build` from a project root \
             (or have a plugin contribute build routes via injectRoute)",
            project_root.display()
        ));
    }

    // #1193 — scan the build pages root (the overlay when package routes
    // are present, else `project_root/pages`), so the merged route table
    // includes package-owned routes.
    let router = Router::scan(&build_pages_root)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("scanning routes under {}", build_pages_root.display()))?;
    let routes = router.routes();

    let (pages_built, route_manifest) = tokio::task::block_in_place(|| {
        run_build(BuildArgsResolved {
            project_root: &project_root,
            build_pages_root: &build_pages_root,
            // codex P1 — user-page islands resolve against the REAL pages
            // dir, never the overlay; package-route islands seed from their
            // real entrypoints.
            user_pages_dir: &pages_dir,
            package_route_entrypoints: &package_route_entrypoints,
            outdir: &outdir,
            config: &config,
            routes,
            runner: &DefaultRunner {
                islands_plugin_config,
                v8_plugin_hooks,
                registered_client_entries: setup_registries.client_entries.clone(),
            },
            adapter_runner: &DefaultAdapterRunner,
            plugin_alias_entries: main_bundler_alias_entries,
            plugin_virtual_modules: main_bundler_virtual_modules,
            minify_html,
        })
    })?;

    // #347 — emit the on-disk route manifest before postBuild runs so
    // any consumer script wired into `pnpm build` (sitemap generator,
    // OGP indexer, search shard builder) can read the same data the
    // plugin API exposes without writing a plugin. Default-on; opt out
    // via `emitRoutesManifest: false` in `zfb.config.ts`. Disabling the
    // emit does NOT affect `ctx.routes` — postBuild plugins still see
    // the in-memory manifest below.
    if config.emit_routes_manifest.unwrap_or(true) {
        emit_routes_manifest_file(&outdir, &route_manifest)
            .context("failed to emit dist/__zfb/routes.json")?;
    }

    // postBuild fires AFTER the renderer has finished writing dist/
    // (and the adapter has wrapped any SSR output). Run it before the
    // success banner so a failure here surfaces as a build error
    // rather than a phantom "build succeeded but plugin crashed".
    if let Some(host) = plugin_host.as_ref() {
        let ctx = zfb_build::BuildHookContext {
            project_root: project_root.clone(),
            out_dir: outdir.clone(),
            config: serde_json::to_value(&config)
                .context("plugin lifecycle: serialise config for postBuild ctx")?,
            // postBuild: routes present with all emitted URLs (#262).
            routes: Some(route_manifest),
        };
        host.run_post_build(&ctx)
            .await
            .map_err(zfb_build::annotate_with_plugin_error)
            .context("postBuild lifecycle hook")?;
    }
    if let Some(host) = plugin_host {
        // Best-effort shutdown — we already extracted whatever errors
        // matter from the hook calls.
        let _ = host.shutdown().await;
    }

    let elapsed = started.elapsed().as_secs_f64();
    output::success(format!("{pages_built} pages built in {elapsed:.2}s"));

    Ok(())
}

/// V8-off stub for `zfb build` (issue #371, sub-task 4.1a).
///
/// The build pipeline needs the embedded V8 host to evaluate dynamic
/// `paths()`, render SSG pages, and to bridge SSR routes through the
/// runtime. Without `embed_v8` none of that wiring exists, so we
/// surface a clear error at the call site rather than partially
/// running a doomed pipeline.
#[cfg(not(feature = "embed_v8"))]
pub async fn run(_args: &BuildArgs) -> Result<()> {
    anyhow::bail!(
        "zfb was built without V8 support (`--no-default-features` / \
         `embed_v8 = off`); `zfb build` requires the embedded V8 host \
         to render SSG pages. Rebuild with default features \
         (`cargo build`) or with `--features embed_v8` to enable this command."
    )
}

// ---------------------------------------------------------------------------
// V8 mode resolution (sub-task 4.1b / issue #373)
// ---------------------------------------------------------------------------

/// Resolved V8-mode decision for the current build.
///
/// Derived from [`Config::output`] and the set of `prerender = false`
/// routes by [`resolve_v8_mode`]. See [`OutputMode`] for the field-level
/// docs.
///
/// **Today's load-bearing role** is the precondition error returned by
/// [`resolve_v8_mode`] when `output: "static"` collides with detected SSR
/// routes. The value itself is computed and surfaced (so tests can pin
/// the decision tree) but the SSG render path still boots the embedded
/// V8 host unconditionally on the V8-on `zfb` binary — there is no
/// V8-less SSG renderer in this workspace today. `embed_v8 = off` is
/// already a hard fail at `pub async fn run` because the V8-off binary
/// can't render SSG pages.
///
/// The flag exists as infrastructure for the future shipping path
/// (Tauri sidecar / standalone SSR server) where a V8-less Rust runtime
/// is conceivable. See `research/344-v8-feature-gate.md` for the
/// rationale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V8Mode {
    /// V8-on: build assumes a V8-bearing runtime is part of the deploy
    /// shape (today: SSR routes through the adapter; future: any
    /// V8-bearing standalone runtime).
    On,
    /// V8-off: build proceeds as if no V8 runtime ships with the
    /// deploy artifact. Today this is observational; the binary still
    /// needs V8 for SSG rendering on the build machine.
    Off,
}

/// Resolve the V8-mode decision from the user's `output` choice and the
/// detected SSR-route set.
///
/// Decision tree (mirrors the table on [`OutputMode`]):
///
/// | `output`   | `ssr_routes` non-empty | result        |
/// | ---------- | ---------------------- | ------------- |
/// | `"static"` | no                     | `V8Mode::Off` |
/// | `"static"` | yes                    | **error**     |
/// | `"hybrid"` | any                    | `V8Mode::On`  |
/// | `"auto"`   | no                     | `V8Mode::Off` |
/// | `"auto"`   | yes                    | `V8Mode::On`  |
///
/// The error path is the load-bearing user-visible behaviour today —
/// it fires before the bundle step so a contradicting config doesn't
/// silently flip a route's deploy shape. The other branches resolve to
/// a `V8Mode` value that future shipping paths will read; SSG rendering
/// on the build machine continues to use V8 unconditionally on a V8-on
/// binary.
pub(crate) fn resolve_v8_mode(
    output: OutputMode,
    ssr_routes: &[SsrRouteRef<'_>],
) -> Result<V8Mode> {
    match output {
        OutputMode::Static => {
            if ssr_routes.is_empty() {
                Ok(V8Mode::Off)
            } else {
                let first = ssr_routes.first().expect("checked non-empty above");
                let extra = if ssr_routes.len() > 1 {
                    format!(" (and {} more)", ssr_routes.len() - 1)
                } else {
                    String::new()
                };
                Err(anyhow!(
                    "config sets `output: \"static\"` but route {route} \
                     exports `prerender = false`{extra}, which requires \
                     a V8-bearing runtime. Either remove `output: \"static\"` \
                     from zfb.config.ts (defaults to detection-driven `auto`) \
                     or change the route to `prerender = true`. \
                     See https://github.com/Takazudo/zudo-front-builder/blob/main/docs/src/content/docs/architecture/build-engine.mdx \
                     for the gate decision table.",
                    route = first.route_key,
                    extra = extra,
                ))
            }
        }
        OutputMode::Hybrid => Ok(V8Mode::On),
        OutputMode::Auto => {
            if ssr_routes.is_empty() {
                Ok(V8Mode::Off)
            } else {
                Ok(V8Mode::On)
            }
        }
    }
}

/// Resolve the effective production HTML-minification switch.
///
/// CLI flags are tri-state so an omitted flag can defer to the
/// config/preset value. `Config::minify_html` itself is already
/// defaulted to `false` by serde, so this function returns the single
/// boolean the build orchestration should carry.
pub(crate) fn resolve_minify_html(cli: BuildMinifyHtml, config: &Config) -> bool {
    cli.as_option().unwrap_or(config.minify_html)
}

// ---------------------------------------------------------------------------
// Internals — testable orchestration
// ---------------------------------------------------------------------------

/// Resolved inputs to the orchestration. Kept as a struct so the
/// orchestration body and the tests share one signature; adding a field
/// later doesn't ripple into call sites.
struct BuildArgsResolved<'a, R: BuildRunner, A: AdapterRunner> {
    project_root: &'a Path,
    /// The pages root the bundler and router scan are pointed at (#1193).
    /// Equal to `project_root/pages` for a project with no package-owned
    /// routes; otherwise the per-build overlay temp dir. Threaded so
    /// `BundlerInput.pages_dir` uses the SAME root the router scan used (the
    /// overlay IS the single pages root — there is no multi-root merge).
    ///
    /// NOTE: the islands seed does NOT use this — see `user_pages_dir` /
    /// `package_route_entrypoints` below (codex P1).
    build_pages_root: &'a Path,
    /// The REAL `project_root/pages` dir (codex P1). The islands seed walks
    /// THIS for user pages, never `build_pages_root` — when a package route
    /// makes `build_pages_root` an overlay temp dir, the overlay has no
    /// `components/`, so seeding from it strands user-page islands reached
    /// via outside-`pages/` imports. Equal to `build_pages_root` only on the
    /// no-package-route parity path.
    user_pages_dir: &'a Path,
    /// Absolute entrypoints of materialized package routes (codex P1). The
    /// islands seed adds each as a DFS root so package-route islands are
    /// discovered via the route's REAL module (whose relative imports resolve
    /// against its real location). Empty when there are no package routes.
    package_route_entrypoints: &'a [PathBuf],
    outdir: &'a Path,
    config: &'a Config,
    routes: &'a [zfb_router::Route],
    runner: &'a R,
    /// Indirection over `pnpm exec <adapter-bin>` so unit tests can
    /// assert dispatch shape without spawning a real subprocess.
    adapter_runner: &'a A,
    /// Plugin-registered import aliases to thread into the main bundler's
    /// esbuild invocation. Each `(from, to)` pair becomes `--alias:<from>=<to>`.
    /// Sourced from `setup_registries.aliases`; empty vec when no plugins are
    /// active. Mirrors what `IslandsPluginConfig::alias_entries` provides for
    /// the islands path.
    plugin_alias_entries: Vec<(String, String)>,
    /// Plugin-registered virtual-module `(specifier, source)` pairs to thread
    /// into the main bundler's esbuild invocation. Sourced from
    /// `setup_registries.virtual_modules` (sources pre-fetched before
    /// `block_in_place`). Empty vec when no plugins are active.
    plugin_virtual_modules: Vec<(String, String)>,
    /// Effective production HTML minification decision after resolving
    /// CLI override > config/preset > default.
    minify_html: bool,
}

/// Indirection seam over the heavy bundler + renderer calls.
///
/// Production wires this to [`DefaultRunner`] which calls the real
/// [`zfb_build::bundle`] and [`zfb_build::renderer::render_all`]. Unit
/// tests plug in fakes that record the arguments and return canned
/// outputs without spawning any subprocesses.
trait BuildRunner {
    /// Run the bundler. The renderer needs the resulting `bundle_path`
    /// + `sourcemap_path`. We return them through
    ///   [`zfb_build::bundler::BundlerOutput`] so production retains the
    ///   full manifest for the renderer's diagnostics path.
    fn bundle(&self, input: BundlerInput) -> Result<BundlerOutput>;

    /// Evaluate deferred dynamic routes whose `paths()` couldn't be
    /// statically extracted. The production runner starts the embedded V8
    /// host against the bundle, queries `/__paths__/<route>` for each
    /// deferred route, and returns:
    ///
    /// - the expanded route entries (resolved) + still-deferred entries,
    /// - the [`Backend`] to pass to the subsequent `render_all` call
    ///   (either `EmbeddedV8` when no host was started here, or
    ///   `Existing { base_url }` to reuse the already-running process),
    /// - an opaque cleanup handle — **the caller MUST drop it after
    ///   calling `render_all`**. Dropping it shuts the host down.
    ///   When no host was started (fake runner, or no deferred
    ///   routes), the handle is a no-op.
    ///
    /// Fake runners return an empty expansion and a `Stub` backend (the
    /// fake `render_all` ignores the backend).
    fn eval_deferred_paths(
        &self,
        deferred: &[DeferredDynamicRoute],
        bundle_out: &BundlerOutput,
        cache: &mut PathsCache,
    ) -> Result<(
        crate::render_pipeline::DynamicExpansion,
        Backend,
        WorkerHandle,
    )>;

    /// Run the renderer. Errors surface verbatim — the CLI relies on
    /// the renderer's
    /// [`zfb_build::renderer::RendererError::RenderFailed`] including
    /// the source-mapped user location.
    fn render_all(&self, input: RendererInput) -> Result<RendererOutput>;

    /// Produce bytes-only payloads for the production asset emitters.
    /// Called once per build, BEFORE `render_all`, so the
    /// orchestration can decide whether the renderer should inject
    /// `<link rel=stylesheet>` / `<script type=module>` for the
    /// matching stable URL — emitting head tags pointing at an asset
    /// that is never written would leak an unhashed URL into the
    /// shipped HTML.
    ///
    /// Default implementations live on:
    ///
    /// - [`DefaultRunner`] — runs `CssPipeline::build_emitter` and
    ///   `build_production_islands_asset` eagerly so head injection
    ///   knows which stable URLs are backed by bytes. Returns `None`
    ///   for any slot the project does not exercise (e.g. Tailwind
    ///   disabled, no `"use client"` components).
    /// - `FakeRunner` (test-only) — returns whatever bytes the test
    ///   set up so the rewrite path can be exercised without running
    ///   Tailwind / esbuild subprocesses.
    ///
    /// Returns both the bytes-only emitter inputs (CSS / islands / client
    /// scripts) **and** the set of registered island marker names collected by
    /// the islands scanner.  The marker-name set is empty when no islands were
    /// found; callers that only need the pipeline bytes may simply ignore it.
    ///
    /// The islands DFS is seeded from TWO sources (codex P1, #1191 review):
    /// `user_pages_dir` (the REAL `project_root/pages`, so user-page islands
    /// reached via outside-`pages/` imports resolve against the real tree)
    /// and `package_route_entrypoints` (each materialized package route's real
    /// entrypoint, so package-route islands are discovered too). It is
    /// deliberately NOT seeded from the overlay `build_pages_root` — that
    /// overlay copies only `pages/` and would strand user-page islands.
    fn emit_prod_assets(
        &self,
        project_root: &Path,
        user_pages_dir: &Path,
        package_route_entrypoints: &[PathBuf],
        outdir: &Path,
        config: &Config,
    ) -> Result<(ProdAssetEmitterInputs, std::collections::BTreeSet<String>)>;
}

/// Opaque handle that keeps a background renderer state alive.
///
/// Dropping this handle shuts the embedded V8 host down (via
/// [`zfb_build::renderer::shutdown`]). When no host was started,
/// the inner `Option` is `None` and the drop is a no-op.
struct WorkerHandle(Option<zfb_build::renderer::RendererState>);
impl Drop for WorkerHandle {
    fn drop(&mut self) {
        if let Some(state) = self.0.take() {
            // Best-effort: ignore shutdown errors (we're already done
            // rendering; only cleanup remains).
            let _ = zfb_build::renderer::shutdown(state);
        }
    }
}

/// Pre-fetched alias and virtual-module data from the plugin `setup` hook
/// (#261). Populated in the async `run()` context (where
/// `PluginHost::invoke_virtual_loader` is awaitable) and consumed by the
/// synchronous `build_default_islands_payload` call inside `block_in_place`.
///
/// An empty instance (the default) means no plugin registrations were active;
/// the islands bundler then produces output byte-identical to a build without
/// any plugin hooks.
#[derive(Debug, Default, Clone)]
pub(crate) struct IslandsPluginConfig {
    /// Alias entries derived from `SetupRegistries::aliases`. Each `(from, to)`
    /// pair becomes `--alias:<from>=<to>` on the esbuild subprocess.
    pub(crate) alias_entries: Vec<(String, String)>,
    /// Virtual-module `(specifier, source)` pairs. The source text has been
    /// fetched from `PluginHost::invoke_virtual_loader` before this struct is
    /// constructed. Each entry causes a temp `.mjs` file to be written and an
    /// `--alias:<specifier>=<path>` flag to be added to esbuild.
    pub(crate) virtual_modules: Vec<(String, String)>,
}

/// Production runner — straight pass-throughs to the real bundler /
/// renderer APIs.
///
/// Compiled in only when the `embed_v8` cargo feature is on (issue
/// #371, sub-task 4.1a). The runner's `eval_deferred_paths` impl
/// constructs `Backend::EmbeddedV8`, which only exists on the V8-on
/// path.
#[cfg(feature = "embed_v8")]
struct DefaultRunner {
    /// Pre-fetched plugin setup registries to wire into the islands bundler.
    islands_plugin_config: IslandsPluginConfig,
    /// Plugin-registry hooks for the embedded V8 host (sub-issue #260).
    /// Built from the same `setup_registries` + virtual-source map as
    /// `islands_plugin_config` so islands esbuild and the V8 host agree on
    /// the registered aliases / virtual modules.
    v8_plugin_hooks: zfb_render::PluginRegistryHooks,
    /// Package-owned client-side side-effect entries registered via
    /// `addClientEntry` during the plugin `setup` hook (#1196). Merged
    /// into the discovered `*.client.*` set before bundling.
    registered_client_entries: zfb_build::ClientEntryList,
}
#[cfg(feature = "embed_v8")]
impl BuildRunner for DefaultRunner {
    fn bundle(&self, input: BundlerInput) -> Result<BundlerOutput> {
        bundle(input)
    }

    fn eval_deferred_paths(
        &self,
        deferred: &[DeferredDynamicRoute],
        bundle_out: &BundlerOutput,
        cache: &mut PathsCache,
    ) -> Result<(
        crate::render_pipeline::DynamicExpansion,
        Backend,
        WorkerHandle,
    )> {
        let factory =
            crate::v8_host_adapter::make_v8_host_factory_with_hooks(self.v8_plugin_hooks.clone());
        if deferred.is_empty() {
            // No deferred routes: skip host construction entirely. Return the
            // factory so `render_all` can still boot the host for SSG.
            return Ok((
                crate::render_pipeline::DynamicExpansion::default(),
                Backend::EmbeddedV8 {
                    host_factory: factory,
                },
                WorkerHandle(None),
            ));
        }
        // Start the embedded V8 host against the bundle to evaluate runtime
        // paths(). The host is dispatched in-process via
        // WorkerDispatch::EmbeddedV8 — no HTTP server, no base_url.
        let mut state = zfb_build::renderer::start(zfb_build::renderer::RendererStartInput {
            bundle_path: bundle_out.bundle_path.clone(),
            sourcemap_path: bundle_out.sourcemap_path.clone(),
            backend: Backend::EmbeddedV8 {
                host_factory: factory.clone(),
            },
            request_timeout: None,
        })
        .context("could not start embedded V8 host for runtime paths() evaluation")?;
        let expansion = {
            // Borrow the live host from the RendererState and dispatch via
            // WorkerDispatch::EmbeddedV8 — no TCP hop, no HTTP base_url.
            let host = state.embedded_v8_host_mut().ok_or_else(|| {
                anyhow::anyhow!(
                    "embedded V8 host unavailable after start; \
                     Backend::EmbeddedV8 state had no host"
                )
            })?;
            let mut dispatch = WorkerDispatch::EmbeddedV8 { host };
            eval_deferred_paths_via_worker(deferred, &mut dispatch, cache, None)
        };
        // Return Backend::EmbeddedV8 (with the same factory) for the
        // subsequent render_all call.  render_all will create its own fresh
        // V8 host from the factory; the one in `state` here can then be
        // dropped cleanly.  The WorkerHandle wraps the state so it is shut
        // down after render_all finishes — this is belt-and-braces: the
        // state is only kept alive through the WorkerHandle drop, which runs
        // after render_all.
        Ok((
            expansion,
            Backend::EmbeddedV8 {
                host_factory: factory,
            },
            WorkerHandle(Some(state)),
        ))
    }

    fn render_all(&self, input: RendererInput) -> Result<RendererOutput> {
        render_all(input).map_err(anyhow::Error::from)
    }

    fn emit_prod_assets(
        &self,
        project_root: &Path,
        user_pages_dir: &Path,
        package_route_entrypoints: &[PathBuf],
        outdir: &Path,
        config: &Config,
    ) -> Result<(ProdAssetEmitterInputs, std::collections::BTreeSet<String>)> {
        // Run `CssPipeline::build_emitter` and
        // `build_production_islands_asset` eagerly (before render) so
        // head injection knows which stable URLs are backed by
        // bytes. Either slot independently returns `None` when the
        // project doesn't exercise it (Tailwind disabled, no
        // `"use client"` components, etc.).
        let css =
            build_default_css_payload(project_root, outdir, config, package_route_entrypoints)
                .context("CSS emitter (DefaultRunner) failed")?;
        let (islands, registered_marker_names) = build_default_islands_payload_with_bundle_options(
            project_root,
            user_pages_dir,
            package_route_entrypoints,
            outdir,
            config.framework,
            config.bundle.as_ref(),
            zfb_islands::BundleMode::Production,
            &self.islands_plugin_config,
            IslandsGlobPolicy::HardError,
            None,
        )
        .context("islands emitter (DefaultRunner) failed")?;
        let client_scripts = build_default_client_scripts_payloads(
            project_root,
            outdir,
            config.framework,
            &self.registered_client_entries,
            config.bundle.as_ref(),
        )
        .context("client-script emitters (DefaultRunner) failed")?;
        Ok((
            ProdAssetEmitterInputs {
                css,
                islands,
                client_scripts,
            },
            registered_marker_names,
        ))
    }
}

/// Run the real `CssPipeline::build_emitter` for a project and return
/// its bytes packaged for [`ProductionAssetPipeline`].
///
/// When the user disabled Tailwind via `zfb.config.{ts,json}`
/// (`tailwind: { enabled: false }`), this skips the Tailwind layers
/// (import / `@source` scan / preflight / subprocess) but still
/// processes the authored global stylesheet and CSS Modules — see
/// [`build_authored_only_css_payload`]. `enabled: false` means "no
/// Tailwind", not "no CSS" (issue #824).
///
/// Returns `Ok(None)` when:
///
/// - no scannable source files were found under the conventional
///   project roots (`pages/`, `components/`, `layouts/`, `content/`)
///   AND no authored global stylesheet exists. In that case the
///   project carries no CSS authoring surface and emitting an empty
///   stylesheet would just leave a broken `<link>` tag in HTML.
///
/// On `Ok(Some(_))` the orchestrator hashes the bytes and writes
/// `dist/assets/styles-<hash>.css`. The `relative_path` /
/// `stable_url` come from the bytes-only emitter contract
/// (`zfb_css::css_relative_path` and `zfb_types::STABLE_CSS_URL`) so
/// the renderer's head injector and the prod pipeline's URL rewriter
/// agree on the same key without a separate string channel.
pub(crate) fn build_default_css_payload(
    project_root: &Path,
    outdir: &Path,
    config: &Config,
    // fix-A [5] (#1191): absolute paths of materialized package-route
    // entrypoints. Their parent dirs are appended to the Tailwind `@source`
    // content globs so utility classes used ONLY in a package-route page
    // (whose entrypoint lives in node_modules / outside the globbed project
    // dirs) are scanned and not silently pruned from the emitted stylesheet.
    // Empty on the no-package-route path (byte-identical parity).
    package_route_entrypoints: &[PathBuf],
) -> Result<Option<AssetEmitterPayload>> {
    // `tailwind: { enabled: false }` disables only the Tailwind layers,
    // not the authored-CSS pipeline. Route to the Tailwind-free path so
    // global CSS + CSS Modules still ship (issue #824). Falling back to
    // the Tailwind subprocess path here would re-add the preflight the
    // user opted out of and incur subprocess cost.
    let tailwind_enabled = config.tailwind.as_ref().map(|t| t.enabled).unwrap_or(true);
    if !tailwind_enabled {
        return build_authored_only_css_payload(project_root, outdir);
    }

    let sources = discover_css_source_files(project_root);
    if sources.is_empty() {
        // No scannable surface — Tailwind would emit only its
        // preflight + reset bytes, which still yields a non-empty
        // stylesheet. The current shape ships those preflight bytes
        // by design (project might author globals via `input_css`),
        // so we proceed even with empty `sources`.
    }

    // Tailwind config: working_dir at project root so `@source`
    // directives resolve user paths correctly. Default content globs
    // come from `zfb_css::DEFAULT_CONTENT_ROOTS`; we rebase them
    // onto the project root absolute path so the synthesised entry
    // CSS picks up sources regardless of where the user invoked
    // `zfb build`.
    let mut content_globs = zfb_css::engine::DEFAULT_CONTENT_ROOTS
        .iter()
        .map(|root| project_root.join(root).to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    // fix-A [5] (#1191): a package-route page's entrypoint lives OUTSIDE the
    // conventional project content roots (node_modules / a workspace package),
    // and the overlay re-export module carries no class strings — so Tailwind's
    // `@source` scan would never see the package page's utility classes and
    // would prune them from `styles-<hash>.css` (green build, unstyled page).
    // Add each entrypoint's parent directory as an extra `@source` root so its
    // classes (and those of files it imports from the same package dir) are
    // scanned. De-duped to keep the directive list stable.
    {
        let mut seen: std::collections::HashSet<String> = content_globs.iter().cloned().collect();
        for entry in package_route_entrypoints {
            if let Some(dir) = entry.parent() {
                let glob = dir.to_string_lossy().into_owned();
                if seen.insert(glob.clone()) {
                    content_globs.push(glob);
                }
            }
        }
    }

    let mut tw_cfg = TailwindSubprocessConfig::default()
        .with_working_dir(project_root.to_path_buf())
        .with_content_globs(content_globs);

    // Sub #212 — wire in the embedded-binary extraction tier so consumers
    // running `zfb build` from a project that doesn't ship the
    // `crates/zfb/binaries/` workspace dir still resolve a working tailwind
    // CLI. The TempDir handle rides on the config (and hence the engine)
    // so the extracted file outlives every `produce_utility_css` call.
    // Skip when `ZFB_TAILWIND_BIN` is set — `with_embedded_binary` is also
    // a no-op in that case, but skipping the extract avoids the disk + tar
    // work entirely.
    if std::env::var_os("ZFB_TAILWIND_BIN").is_none() {
        if let Ok((handle, path)) = embedded_binary("tailwindcss-v4") {
            tw_cfg = tw_cfg.with_embedded_binary(handle, path);
        }
    }

    // Honour an authored global stylesheet at the conventional
    // location. Tailwind v4's entry CSS prepends our `@source`
    // directives to whatever the user wrote there, so the user's
    // `@theme`, `@import` of vendor CSS, etc. continue to work.
    //
    // Probe two layouts in order, first match wins:
    //   1. `<root>/styles/global.css`     — zfb's original convention
    //   2. `<root>/src/styles/global.css` — Vite/Astro/Next-style src/ layout
    //
    // The two-path probe matters because real-world consumers
    // (e.g. zudo-doc, see zudolab/zudo-doc#1355 wave 13) keep their
    // authored `@theme` tokens under `src/styles/` to share the src
    // tree with components and TS sources. Without this fallback the
    // Tailwind run misses the host's `@theme` block entirely and
    // utility classes like `bg-zd-bg` plus host-defined custom
    // properties go unstyled.
    if let Some(path) = resolve_input_global_css(project_root) {
        tw_cfg = tw_cfg.with_input_css(path);
    }

    let engine = TailwindSubprocessEngine::new(tw_cfg);

    // Tailwind path always ships a payload — its preflight bytes are never
    // empty, so there is no whitespace-only guard here (unlike the
    // authored-only path).
    let payload = run_css_emitter(engine, project_root, outdir, sources)?;
    Ok(Some(payload))
}

/// Shared tail of the two CSS-emitter paths
/// ([`build_default_css_payload`] and [`build_authored_only_css_payload`]).
///
/// Given an already-configured [`CssEngine`] (Tailwind subprocess or
/// authored-verbatim), builds the [`CssPipelineConfig`], runs
/// [`CssPipeline::build_emitter`], and packages the result as an
/// [`AssetEmitterPayload`]. Both call sites feed the same `project_root`
/// and `outdir`, so the CSS Modules hash root is identical across them and
/// matches [`compute_css_module_class_maps`] (issue #825). The per-call-site
/// differences (the authored path's whitespace-only `None` guard vs the
/// Tailwind path's always-emit) stay at the call sites, not in this helper.
fn run_css_emitter<E: CssEngine>(
    engine: E,
    project_root: &Path,
    outdir: &Path,
    sources: Vec<PathBuf>,
) -> Result<AssetEmitterPayload> {
    let pipe_cfg = CssPipelineConfig {
        sources,
        // The on-disk class-map JSON writer is not used: the build-time
        // CSS Modules rewrite consumes the maps in-memory instead.
        // `compute_css_module_class_maps` runs `CssModulesProcessor`
        // directly (same config this emitter uses, so scoped names agree)
        // and feeds `BundlerInput::css_module_class_maps`, which the
        // bundler applies in the shadow tree. No JSON channel is needed,
        // so `class_map_dir` stays `None`.
        class_map_dir: None,
        // `output_root` is unused by `build_emitter` (it does not
        // write the hashed asset itself) but is read by the
        // class-map writer when `class_map_dir` is `Some`. Pin it to
        // the configured outdir for forward-compat.
        output_root: outdir.to_path_buf(),
        // Hash root shared with `compute_css_module_class_maps` via
        // `CssModulesConfig::for_project_root` (issue #825).
        modules_config: zfb_css::modules::CssModulesConfig::for_project_root(project_root),
        ..CssPipelineConfig::default()
    };

    let pipeline = CssPipeline::new(engine, pipe_cfg);
    let emitter_out = pipeline.build_emitter()?;

    Ok(AssetEmitterPayload {
        bytes: emitter_out.bytes,
        relative_path: css_relative_path(),
        stable_url: emitter_out.stable_url,
        companions: Vec::new(),
    })
}

/// CSS payload for the `tailwind: { enabled: false }` path: authored
/// global stylesheet + CSS Modules, with the Tailwind layers
/// (import / `@source` scan / preflight / subprocess) skipped entirely.
///
/// `enabled: false` opts out of Tailwind, not out of CSS (issue #824).
/// The authored global stylesheet currently rides *inside* the Tailwind
/// engine via its `input_css` slot, so simply skipping the engine would
/// drop the user's globals too. Instead we read the authored global CSS
/// independently (same probe order as the Tailwind path,
/// [`resolve_input_global_css`]) and feed it through an
/// [`AuthoredCssEngine`] — a no-subprocess engine that returns the
/// authored bytes verbatim as the "engine half" of the combined
/// stylesheet. CSS Modules processing, concatenation, hashing, and
/// asset emission are engine-agnostic and run unchanged via
/// [`CssPipeline::build_emitter`].
///
/// Returns `Ok(None)` when the project has neither an authored global
/// stylesheet nor any CSS Modules — the combined output would be
/// whitespace only, and emitting it would leave a broken `<link>` tag
/// in HTML. This mirrors the empty-stylesheet guard on the Tailwind
/// path.
fn build_authored_only_css_payload(
    project_root: &Path,
    outdir: &Path,
) -> Result<Option<AssetEmitterPayload>> {
    let authored_css = match resolve_input_global_css(project_root) {
        Some(path) => {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read global CSS at {}", path.display()))?;
            // Skip the Tailwind import layer: the default zfb template's
            // `styles/global.css` ships `@import "tailwindcss";` (or the
            // split `tailwindcss/preflight` / `utilities` forms). With no
            // Tailwind subprocess to resolve them, emitting those lines
            // verbatim would make the browser request a non-existent
            // stylesheet, so we drop them here (issue #824).
            strip_tailwind_imports(&raw)
        }
        None => String::new(),
    };

    let sources = discover_css_source_files(project_root);
    let engine = AuthoredCssEngine::new(authored_css);

    let payload = run_css_emitter(engine, project_root, outdir, sources)?;

    // Skip the link when there is nothing to ship. With Tailwind off and
    // no authored globals + no modules, `combine` yields only its `"\n"`
    // separator; emitting that would inject a `<link>` to an effectively
    // empty stylesheet. The Tailwind path never hits this because its
    // preflight bytes are always non-empty.
    if payload.bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }

    Ok(Some(payload))
}

/// Drop `@import "tailwindcss"` directives from authored global CSS for
/// the Tailwind-disabled path.
///
/// Removes whole physical lines whose trimmed form starts with the
/// Tailwind import, covering both the umbrella import and the v4 split
/// sub-imports (`tailwindcss/preflight`, `tailwindcss/utilities`, …), in
/// either quote style. The per-line test is the shared
/// [`zfb_css::is_tailwind_import_line`] predicate — the same one
/// `build_synthesised_entry_css`'s `user_has_import` detection uses — so
/// the enabled and disabled paths agree byte-for-byte on what counts as
/// "the Tailwind import".
///
/// Scope: this strips the `@import` layer only. Other Tailwind-v4-only
/// at-rules (`@theme`, `@apply`, `@source`, `@utility`) are left as-is —
/// a zero-Tailwind project (the `enabled: false` scenario, issue #824)
/// does not author them, and sanitising the full Tailwind syntax is out
/// of scope.
///
/// Known limitations (intentional — this is a line filter, not a CSS
/// parser, and unlike the Tailwind path it does **not** strip comments
/// before scanning):
///
/// - An `@import "tailwindcss";` line *inside* a multi-line
///   `/* … */` block comment is dropped even though it is already inert.
///   Harmless: the surrounding comment delimiters stay, the output is
///   still valid CSS, and the browser requests nothing. (A single-line
///   `/* @import "tailwindcss"; */` is untouched — its trimmed line starts
///   with `/*`, not `@import`.)
/// - A same-line trailing rule (`@import "tailwindcss"; .real{…}`) is lost
///   along with the import, because whole physical lines are dropped. The
///   default zfb template puts the import on its own line, so this does not
///   bite real projects.
fn strip_tailwind_imports(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    for line in css.split_inclusive('\n') {
        if is_tailwind_import_line(line) {
            continue;
        }
        out.push_str(line);
    }
    out
}

/// Locate the project's authored global Tailwind input CSS file.
///
/// Probes the two conventional layouts in order and returns the first
/// match. Returns `None` when neither file exists, in which case the
/// CSS pipeline emits Tailwind preflight + scanned utilities only
/// (no `@theme` tokens, no user `@import` of vendor CSS).
///
/// Probe order:
///
/// 1. `<root>/styles/global.css`     — zfb's original convention.
/// 2. `<root>/src/styles/global.css` — Vite/Astro/Next-style `src/`
///    layout used by real-world consumers (e.g. zudo-doc; see
///    zudolab/zudo-doc#1355 wave 13). The `src/styles` fallback
///    closes the upstream gap that previously dropped the host's
///    authored `@theme` block on the floor whenever the project
///    organised its sources under `src/`.
///
/// Order is deterministic. If both files exist the legacy
/// `<root>/styles/global.css` wins so existing projects on the
/// original convention see no behaviour change.
pub(crate) fn resolve_input_global_css(project_root: &Path) -> Option<PathBuf> {
    const CANDIDATES: &[&[&str]] = &[&["styles", "global.css"], &["src", "styles", "global.css"]];
    for parts in CANDIDATES {
        let mut candidate = project_root.to_path_buf();
        for seg in *parts {
            candidate.push(seg);
        }
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Walk the conventional CSS-content roots (`pages/`, `components/`,
/// `layouts/`, `content/`) and return every TSX/TS/JSX/JS/MDX/MD
/// source file beneath them. Used as the `sources` field for the
/// CSS pipeline so the CSS Modules import-scanner can resolve
/// `import "...module.css"` statements; Tailwind's own utility-class
/// scan is driven by the synthesised `@source` directives, not this
/// list, so missing a file here does not silently strip utilities.
///
/// Order is filesystem walk order — the CSS pipeline's discovery
/// step de-dupes against an internal HashSet, so determinism is the
/// pipeline's responsibility, not this helper's.
fn discover_css_source_files(project_root: &Path) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    let extensions = ["tsx", "ts", "jsx", "js", "mdx", "md"];
    for root in zfb_css::engine::DEFAULT_CONTENT_ROOTS {
        let dir = project_root.join(root);
        if !dir.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&dir)
            .into_iter()
            .filter_map(|r| r.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let ext = entry
                .path()
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.to_ascii_lowercase());
            if let Some(ext) = ext {
                if extensions.contains(&ext.as_str()) {
                    out.push(entry.into_path());
                }
            }
        }
    }
    out
}

/// Compute the CSS Modules class-name maps for a project: discover
/// `.module.css` imports under the conventional source roots, compile
/// each with `lightningcss`, and return the
/// `absolute .module.css path → (original class → scoped class)` map.
///
/// This is the producer half of the build-time CSS Modules JSX
/// rewrite. The map is fed into [`BundlerInput::css_module_class_maps`]
/// so the bundler can rewrite each `import styles from "./x.module.css"`
/// to the scoped class names (see `zfb-build`'s `bundler.rs`).
///
/// The scoped class names produced here MUST be byte-identical to the
/// ones in the emitted `styles-<hash>.css` — both sides run
/// `CssModulesProcessor` with `CssModulesConfig::default()` (the same
/// config `CssPipeline` uses inside `build_emitter`), so the scoped
/// names agree without a shared channel.
///
/// Returns an empty map when no `.module.css` files are reachable — the
/// build then behaves exactly as before.
///
/// CSS Modules are processed regardless of `tailwind.enabled`: the
/// authored-CSS pipeline (and hence the emitted `styles-<hash>.css`)
/// ships the scoped module CSS even when Tailwind is off (issue #824),
/// so the class-map rewrite must run in lockstep or the HTML `class`
/// attributes would reference classes that never appear in the
/// stylesheet.
pub(crate) fn compute_css_module_class_maps(
    project_root: &Path,
) -> Result<std::collections::HashMap<PathBuf, std::collections::HashMap<String, String>>> {
    use std::collections::HashMap;

    let sources = discover_css_source_files(project_root);
    if sources.is_empty() {
        return Ok(HashMap::new());
    }

    let scan =
        zfb_css::scan_css_module_imports(&sources).context("CSS Modules import scan failed")?;

    // Auto-discovered modules: keep only resolved paths that exist on
    // disk — mirrors `CssPipeline::collect_modules`. Bare specifiers
    // (`@org/pkg/x.module.css`) cannot be compiled by lightningcss and
    // are dropped here too.
    let module_files: Vec<PathBuf> = scan.modules.into_iter().filter(|m| m.exists()).collect();
    if module_files.is_empty() {
        return Ok(HashMap::new());
    }

    // Hash scoped names off the project-relative path via the shared
    // `for_project_root` constructor (issue #825) — the same config
    // `run_css_emitter` feeds its pipeline, so the scoped names baked into
    // the JSX rewrite match the ones in the emitted `styles-<hash>.css`.
    let processor = zfb_css::CssModulesProcessor::new(
        zfb_css::modules::CssModulesConfig::for_project_root(project_root),
    );
    let out = processor
        .process(&module_files)
        .context("CSS Modules compilation failed")?;
    Ok(out.class_maps)
}

/// How `build_default_islands_payload` reacts when the scanner reports
/// [`zfb_islands::ScanMeta::glob_reachable_from_islands`] non-empty (issue
/// #1387, stopgap for #1385 pt.1: `import.meta.glob` reachable from a
/// `"use client"` island ships to the browser unexpanded and throws at
/// hydration).
///
/// Both `zfb build` and `zfb dev` route through the same
/// `build_default_islands_payload` function, but the two surfaces need
/// different failure modes: a one-shot build should fail loudly and stop,
/// while a live dev server must keep running so the author can fix the
/// file and save again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IslandsGlobPolicy {
    /// `zfb build` — return a hard `Err` naming the offending file(s).
    HardError,
    /// `zfb dev` — warn (naming the offending file(s)) and skip this
    /// rebundle tick; the caller sees `Ok((None, ..))`, same shape as the
    /// "no islands found" short-circuit, and the dev server stays up.
    WarnAndSkip,
}

/// A materialised islands shadow tree (issue #1404 — the full #1385 pt.1
/// fix).
///
/// Built by [`materialise_islands_shadow`] when `import.meta.glob`, a terminal
/// `?raw` edge, or a module-worker edge is reachable from a `"use client"`
/// island. It mirrors the project's source tree under a throwaway `TempDir`
/// keyed by project-root-relative path:
/// files that call `import.meta.glob` are written as REAL, expanded copies
/// (the Vite macro expanded Rust-side via `zfb_build::glob_expand`, the same
/// trick the SSR bundler's `materialise_source_file` uses), and `node_modules`
/// is symlinked as a whole. Plain source files are symlinked when esbuild will
/// run with `--preserve-symlinks`; in the project-`node_modules` + tsconfig
/// `paths` shape they are copied instead, mirroring the SSR bundler's
/// copy-mode fallback so non-hoisted pnpm/workspace resolution is not pinned
/// under `<shadow>/node_modules/...`. The island `source_path`s are then
/// remapped into the shadow so esbuild resolves transitive imports through the
/// materialised tree, reaching the expanded glob copies instead of the raw
/// project files. Raw-mirrored JS-like glob target/subtree files are scanned
/// before materialisation so a nested `import.meta.glob` that would otherwise
/// ship unexpanded keeps the stopgap instead.
struct IslandsShadow {
    /// Kept alive so the tempdir (and every symlink / real file inside it)
    /// survives until esbuild has finished bundling. Dropping it deletes the
    /// whole tree, so the caller MUST hold this until after
    /// `build_production_islands_asset` returns.
    _tempdir: tempfile::TempDir,
    /// Map from a real island `source_path` (as it appears in the scanner's
    /// islands set) to its shadow copy. Islands whose source lives outside
    /// the mirrored tree (e.g. a workspace package under `node_modules`) are
    /// absent — left pointing at the real file, resolved through the shadow's
    /// `node_modules` symlink.
    remap: std::collections::HashMap<PathBuf, PathBuf>,
    /// Whether the islands esbuild invocation must receive
    /// `--preserve-symlinks` for this shadow. False only when source files were
    /// copied into the shadow instead of symlinked.
    preserve_symlinks: bool,
    /// Logical original terminal targets represented by generated modules.
    raw_targets: std::collections::BTreeSet<PathBuf>,
}

/// Outcome of attempting to build the islands shadow for a project whose
/// scan reported `import.meta.glob` reachable from an island.
enum IslandsShadowOutcome {
    /// A complete shadow was materialised; use it.
    Ready(IslandsShadow),
    /// One or more glob-using modules reachable from an island cannot be
    /// expanded into the shadow — an unsupported `import.meta.glob` form
    /// (lazy/default, non-literal pattern, `import()` mode, …), a parse
    /// error, a glob module located outside the mirrorable project tree
    /// (outside the project root, or under `node_modules`), or a nested glob
    /// in a raw-mirrored glob target/subtree companion. The caller keeps the
    /// #1387 stopgap (build-time error / dev warn-and-skip) for these,
    /// naming the offenders carried here — glob in a SUPPORTED island-
    /// reachable module still expands; only the unsupported/unsafe remainder
    /// falls back.
    KeepStopgap(Vec<String>),
}

struct IslandsShadowPaths<'a> {
    root: &'a Path,
    canonical_root: Option<PathBuf>,
}

impl<'a> IslandsShadowPaths<'a> {
    fn new(root: &'a Path) -> Self {
        Self {
            root,
            canonical_root: root.canonicalize().ok(),
        }
    }

    /// Return the project-root-relative path of `p` when `p` is a
    /// project-local source file the islands shadow mirrors individually —
    /// i.e. it lives under `root` and NOT under `root/node_modules` (which is
    /// symlinked whole). A path outside `root`, under `node_modules`, or equal
    /// to `root` returns `None`.
    ///
    /// The scanner records canonicalized module paths, while callers may pass
    /// a raw project root whose ancestor resolves differently (macOS `/var` ->
    /// `/private/var`, symlinked tempdirs, etc.). Try the cheap raw comparison
    /// first, then fall back to canonicalized path comparison.
    fn project_local_rel(&self, p: &Path) -> Option<PathBuf> {
        if let Ok(rel) = p.strip_prefix(self.root) {
            return Self::usable_rel(rel);
        }

        let canonical_root = self.canonical_root.as_ref()?;
        let canonical_p = p.canonicalize().ok()?;
        let rel = canonical_p.strip_prefix(canonical_root).ok()?;
        Self::usable_rel(rel)
    }

    fn path_key(&self, p: &Path) -> PathBuf {
        self.project_local_rel(p)
            .unwrap_or_else(|| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()))
    }

    fn logical_project_path(&self, p: &Path) -> Option<PathBuf> {
        self.project_local_rel(p).map(|rel| self.root.join(rel))
    }

    fn usable_rel(rel: &Path) -> Option<PathBuf> {
        match rel.components().next() {
            Some(c) if c.as_os_str() == "node_modules" => None,
            Some(_) => Some(rel.to_path_buf()),
            None => None,
        }
    }
}

fn is_islands_shadow_js_like_file(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
        return false;
    };
    ["js", "jsx", "ts", "tsx", "mjs", "cjs", "mts", "cts"]
        .iter()
        .any(|candidate| ext.eq_ignore_ascii_case(candidate))
}

fn bytes_contain_import_meta_glob(bytes: &[u8]) -> bool {
    const NEEDLE: &[u8] = b"import.meta.glob";
    bytes.windows(NEEDLE.len()).any(|window| window == NEEDLE)
}

fn raw_mirrored_import_meta_glob_offender(path: &Path, parse_error: Option<String>) -> String {
    let parse_note = parse_error
        .map(|e| format!(" (the file could not be parsed to rule out a real call: {e})"))
        .unwrap_or_default();
    format!(
        "{} — contains `import.meta.glob`{parse_note} but is only reachable as a \
         raw-mirrored glob target/subtree file in the islands shadow, so it would ship \
         unexpanded and throw at hydration. Hoist the glob into an island-reachable \
         module so zfb can expand it, or replace it with explicit static imports.",
        path.display()
    )
}

/// True when the walk must NOT descend into this directory while mirroring
/// the shadow: dependency/infra trees (`node_modules` is symlinked as a
/// whole instead) and top-level build outputs. Mirrors zfb-build's private
/// `is_pruned_infra_dir` prune list plus the top-level `dist`/`target` dirs
/// so a glob-in-island build never symlinks a large output tree.
///
/// The extra top-level `dist`/`target` prune is what makes this predicate
/// diverge from the expander's `is_pruned_infra_dir` match-walk, so the
/// expansion is fed [`matched_target_under_pruned_build_output`] as its
/// `is_excluded` predicate to drop the same files — the two MUST stay in
/// sync or the expander references a target the mirror never materialises.
fn is_islands_shadow_pruned_dir(entry: &walkdir::DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }
    let name = entry.file_name().to_string_lossy();
    if matches!(
        name.as_ref(),
        "node_modules" | ".git" | ".next" | ".turbo" | ".vercel"
    ) {
        return true;
    }
    // Top-level build outputs only (depth 1 under the walk root) — a nested
    // `dist/`/`target/` source dir is still mirrored (same caveat as
    // zfb-build's prune list).
    if entry.depth() == 1 && matches!(name.as_ref(), "dist" | "target") {
        return true;
    }
    // Any hidden dir below the root (`.zfb-build`, `.cache`, `.vite`, …).
    if entry.depth() > 0 && name.starts_with('.') {
        return true;
    }
    false
}

/// Companion to [`is_islands_shadow_pruned_dir`] for the glob EXPANSION
/// match-walk.
///
/// The shadow MIRROR walk (set (b)) prunes a top-level `dist`/`target`
/// directory via [`is_islands_shadow_pruned_dir`]. The expander's OWN
/// match-walk (`zfb_build::glob_expand`, which prunes with the SSR
/// bundler's `is_pruned_infra_dir`) deliberately does NOT prune those, so
/// a glob like `./**/*.tsx` next to a `dist/` would be matched + imported by
/// the expander but pruned from the mirror → "Could not resolve ./dist/…"
/// at esbuild time. Passing this as the expander's `is_excluded` predicate
/// drops exactly those build-output matches, keeping the matched set ⊆ the
/// mirrored set (and keeping compiled output out of an island bundle, which
/// is correct on its own).
///
/// It mirrors ONLY the depth-1 `dist`/`target` rule — the sole divergence
/// between the two prune predicates — and MUST be kept in sync with it.
fn matched_target_under_pruned_build_output(abs: &Path, file_dir: &Path) -> bool {
    let rel = match abs.strip_prefix(file_dir) {
        Ok(r) => r,
        Err(_) => return false,
    };
    let mut comps = rel.components();
    let first = match comps.next() {
        Some(c) => c.as_os_str().to_string_lossy().into_owned(),
        None => return false,
    };
    // Only a file UNDER a top-level `dist/`/`target/` dir (there must be a
    // further path component) — a file literally named `dist`/`target` at
    // depth 1 is not build output, matching the directory-only prune in
    // `is_islands_shadow_pruned_dir`.
    comps.next().is_some() && matches!(first.as_str(), "dist" | "target")
}

/// Create a symlink at `to` pointing at `from`, removing any pre-existing
/// entry first. Unix uses one call for files and dirs; Windows needs the
/// file/dir split and the privilege to create symlinks (Windows islands
/// shadows are UNTESTED and out of scope per the T3 cutover manifest —
/// junction fallback is a follow-up).
fn shadow_symlink(from: &Path, to: &Path) -> std::io::Result<()> {
    let _ = std::fs::remove_file(to);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(from, to)
    }
    #[cfg(windows)]
    {
        if from.is_dir() {
            std::os::windows::fs::symlink_dir(from, to)
        } else {
            std::os::windows::fs::symlink_file(from, to)
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        // No symlink support — fall back to a real file copy (dirs
        // unsupported on such platforms; none are targeted by zfb today).
        std::fs::copy(from, to).map(|_| ())
    }
}

fn shadow_copy_file(from: &Path, to: &Path) -> std::io::Result<()> {
    let _ = std::fs::remove_file(to);
    std::fs::copy(from, to).map(|_| ())
}

/// Materialise the islands preprocessing shadow tree for a project whose scan
/// reported `import.meta.glob`, `?raw`, or a module worker reachable from a
/// `"use client"` island (issues #1404, #1499, and #1500).
///
/// Precondition: at least one glob module, terminal raw edge, or module-worker
/// edge is present (the caller takes the no-shadow fast path otherwise).
///
/// The set of files mirrored is the union of (a) the island-reachable
/// closure `scan_meta.island_reachable_modules` — the shadow's completeness
/// contract — and (b) every file under each glob module's own directory
/// subtree. Set (b) captures the glob's matched TARGET files, which are
/// reachable only through the expanded macro (never as a scanner edge) and
/// are always anchored under the glob module's directory (`import.meta.glob`
/// rejects `../` patterns), so mirroring that subtree materialises them
/// without re-running the match. Every mirrored file is symlinked EXCEPT the
/// island-reachable glob modules, which are written as expanded real copies.
///
/// Before creating the tempdir, raw-mirrored JS-like files in that union are
/// scanned for real `import.meta.glob(...)` calls. If one is found, the shadow
/// keeps the stopgap because recursive expansion is intentionally out of
/// scope: the raw symlink would otherwise ship the Vite-only macro to the
/// browser and throw at hydration. This is conservative by design. An unused
/// JS-like sibling in a globbed subtree is flagged if it contains a real glob,
/// while a glob target that is also island-reachable is expanded as its own
/// glob module and is not flagged.
///
/// Returns [`IslandsShadowOutcome::KeepStopgap`] (not an `Err`) when a
/// glob module uses an unsupported form, lies outside the mirrorable tree, or
/// a raw-mirrored JS-like companion file contains a nested glob, so the caller
/// can apply the #1387 policy (hard error / dev warn-and-skip).
/// A genuine filesystem error propagates as `Err`.
fn materialise_islands_shadow(
    project_root: &Path,
    islands: &[zfb_islands::Island],
    scan_meta: &zfb_islands::ScanMeta,
) -> Result<IslandsShadowOutcome> {
    use std::collections::{BTreeSet, HashMap};

    let root = project_root;
    let paths = IslandsShadowPaths::new(root);

    // --- Pre-flight: every glob module must be shadow-expandable. ---------
    // A glob module outside the mirrorable tree (outside project_root, or
    // under node_modules) would be reached through the whole `node_modules`
    // symlink as its RAW source and ship the unexpanded call — keep the
    // stopgap for those. Then expand every mirrorable glob module up front so
    // all unsupported-form errors are reported together and the mirror walk
    // is skipped entirely when any is unsupported; cache the expansion for
    // the walk below.
    let mut offenders: Vec<String> = Vec::new();
    let mut expanded_glob_modules: BTreeSet<PathBuf> = BTreeSet::new();
    let mut expanded_by_key: HashMap<PathBuf, String> = HashMap::new();
    let mut matched_glob_targets: BTreeSet<PathBuf> = BTreeSet::new();
    let mut all_raw_edges: BTreeSet<zfb_islands::RawImportEdge> = scan_meta
        .raw_import_edges_from_islands
        .iter()
        .cloned()
        .collect();
    for g in &scan_meta.glob_reachable_from_islands {
        if paths.project_local_rel(g).is_none() {
            offenders.push(format!(
                "{} — reachable from a \"use client\" island but located outside the \
                 mirrorable project tree (outside the project root, or under \
                 node_modules), so its `import.meta.glob` cannot be expanded into the \
                 islands shadow yet",
                g.display()
            ));
            continue;
        }
        let source = std::fs::read_to_string(g)
            .with_context(|| format!("read glob module {}", g.display()))?;
        let file_dir = g.parent().unwrap_or(root);
        // Keep the expander's match-walk in lockstep with the shadow mirror
        // walk (set (b) below): drop any matched target under a top-level
        // `dist`/`target` so the expansion never references a file the mirror
        // (which prunes those via `is_islands_shadow_pruned_dir`) won't
        // materialise. See `matched_target_under_pruned_build_output`.
        let exclude_build_output =
            |abs: &Path| matched_target_under_pruned_build_output(abs, file_dir);
        match zfb_build::glob_expand::expand_import_meta_glob_with_matches(
            &source,
            file_dir,
            &exclude_build_output,
        ) {
            Ok(expansion) => {
                matched_glob_targets.extend(expansion.matched_files);
                expanded_glob_modules.insert(g.clone());
                expanded_by_key.insert(paths.path_key(g), expansion.expanded_source);
            }
            Err(e) => offenders.push(format!("{}: {e:#}", g.display())),
        }
    }
    // --- Collect the file set to mirror. ---------------------------------
    let mut to_mirror: BTreeSet<PathBuf> = BTreeSet::new();
    // (a) the island-reachable closure (project-local files only; the rest
    //     are covered by the node_modules symlink).
    for m in &scan_meta.island_reachable_modules {
        if paths.project_local_rel(m).is_some() {
            to_mirror.insert(m.clone());
        }
    }
    // (b) every file under each glob module's directory subtree (its matched
    //     targets live here).
    for g in &expanded_glob_modules {
        let dir = g.parent().unwrap_or(root);
        for entry in walkdir::WalkDir::new(dir)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !is_islands_shadow_pruned_dir(e))
        {
            let entry =
                entry.with_context(|| format!("walking glob module subtree {}", dir.display()))?;
            if entry.file_type().is_file() && paths.project_local_rel(entry.path()).is_some() {
                to_mirror.insert(entry.path().to_path_buf());
            }
        }
    }
    // (c) every project-local module reachable from each expanded glob target.
    //     These are real esbuild edges after macro expansion, but they are not
    //     present in the page/island scanner graph and may live outside the
    //     glob module's own directory subtree.
    if !matched_glob_targets.is_empty() {
        let target_roots: Vec<PathBuf> = matched_glob_targets.iter().cloned().collect();
        let resolver = FsResolver::new();
        match scan_reachable_modules_with_meta(&target_roots, &resolver) {
            Ok(meta) => {
                for m in meta.modules {
                    if paths.project_local_rel(&m).is_some() {
                        to_mirror.insert(m);
                    }
                }
                all_raw_edges.extend(meta.raw_import_edges);
            }
            Err(zfb_islands::ScanError::Parse { path, message }) => {
                let bytes = std::fs::read(&path).with_context(|| {
                    format!(
                        "read parse-error islands shadow glob target {}",
                        path.display()
                    )
                })?;
                if bytes_contain_import_meta_glob(&bytes)
                    && paths.project_local_rel(&path).is_some()
                {
                    // Let the raw-mirrored nested-glob preflight below produce
                    // the intended #1412 KeepStopgap message, including the
                    // parse-error detail, instead of turning this into an IO-ish
                    // shadow materialisation failure.
                    to_mirror.insert(path);
                } else {
                    return Err(zfb_islands::ScanError::Parse { path, message }.into());
                }
            }
            Err(e) => {
                return Err(anyhow!(
                    "scan imports reachable from islands shadow glob targets: {e}"
                ));
            }
        }
    }

    // --- Pre-flight + expansion: terminal `?raw` edges. -----------------
    // Raw importers are executable modules and therefore receive a real
    // rewritten shadow copy. Raw targets are mirrored/tracked as terminal
    // assets but never scanned as JS (including `foo.js?raw`).
    let mut generated_raw_by_key: HashMap<
        PathBuf,
        Vec<zfb_build::raw_import_expand::GeneratedRawModule>,
    > = HashMap::new();
    let mut raw_target_keys: BTreeSet<PathBuf> = BTreeSet::new();
    let raw_importers: BTreeSet<PathBuf> = all_raw_edges
        .iter()
        .map(|edge| edge.importer.clone())
        .collect();
    for importer in raw_importers {
        if paths.project_local_rel(&importer).is_none() {
            offenders.push(format!(
                "{} — contains `?raw` reachable from a \"use client\" island but is \
                 outside the mirrorable project tree",
                importer.display()
            ));
            continue;
        }
        let key = paths.path_key(&importer);
        let logical_importer = paths.logical_project_path(&importer).ok_or_else(|| {
            anyhow!(
                "raw importer {} has no logical path under {}",
                importer.display(),
                project_root.display()
            )
        })?;
        let source = match expanded_by_key.get(&key) {
            Some(expanded_glob) => expanded_glob.clone(),
            None => std::fs::read_to_string(&importer)
                .with_context(|| format!("read raw-import module {}", importer.display()))?,
        };
        match zfb_build::raw_import_expand::expand_raw_imports(
            &source,
            &logical_importer,
            project_root,
            &|_| false,
        ) {
            Ok(expansion) => {
                expanded_by_key.insert(key.clone(), expansion.expanded_source);
                generated_raw_by_key.insert(key, expansion.generated_modules);
            }
            Err(error) => offenders.push(format!("{}: {error:#}", importer.display())),
        }
    }
    for edge in &all_raw_edges {
        match paths.project_local_rel(&edge.target) {
            Some(_) => {
                raw_target_keys.insert(paths.path_key(&edge.target));
                to_mirror.insert(edge.target.clone());
            }
            None => offenders.push(format!(
                "{} — raw target imported from {} is outside the mirrorable project tree",
                edge.target.display(),
                edge.importer.display()
            )),
        }
    }

    // --- Pre-flight + expansion: module-worker URL edges. ---------------
    // Rewrite every first-party importer, including nested worker sources,
    // using the same zfb-build span pass as SSR. The returned dependency
    // closure is mirrored for the later worker-emission sibling, but no import
    // is injected into the islands entry: worker sources remain browser-only.
    let worker_importers: BTreeSet<PathBuf> = scan_meta
        .module_worker_edges_from_islands
        .iter()
        .map(|edge| edge.importer.clone())
        .collect();
    for importer in worker_importers {
        if paths.project_local_rel(&importer).is_none() {
            offenders.push(format!(
                "{} — contains a module worker reachable from a \"use client\" island but is outside the mirrorable first-party project tree",
                importer.display()
            ));
            continue;
        }
        let key = paths.path_key(&importer);
        let logical_importer = paths.logical_project_path(&importer).ok_or_else(|| {
            anyhow!(
                "module-worker importer {} has no logical path under {}",
                importer.display(),
                project_root.display()
            )
        })?;
        let source = match expanded_by_key.get(&key) {
            Some(expanded) => expanded.clone(),
            None => std::fs::read_to_string(&importer)
                .with_context(|| format!("read module-worker importer {}", importer.display()))?,
        };
        match zfb_build::rewrite_module_worker_urls(&source, &logical_importer, project_root) {
            Ok(rewrite) => {
                expanded_by_key.insert(key, rewrite.expanded_source);
                to_mirror.insert(importer.clone());
                for dependency in rewrite.dependencies {
                    match paths.project_local_rel(&dependency.dependency) {
                        Some(_) => {
                            to_mirror.insert(dependency.dependency);
                        }
                        None => offenders.push(format!(
                            "{} — module-worker dependency of {} is outside the mirrorable first-party project tree",
                            dependency.dependency.display(),
                            importer.display()
                        )),
                    }
                }
            }
            Err(error) => offenders.push(format!("{}: {error:#}", importer.display())),
        }
    }

    // --- Pre-flight: raw-mirrored JS-like companions must not contain a
    // nested glob. ---------------------------------------------------------
    let glob_reachable: BTreeSet<PathBuf> = scan_meta
        .glob_reachable_from_islands
        .iter()
        .map(|p| paths.path_key(p))
        .collect();
    for from in &to_mirror {
        let from_key = paths.path_key(from);
        if raw_target_keys.contains(&from_key) {
            // Terminal text is intentionally never parsed, even when the
            // filename is JS-like and contains `import.meta.glob` bytes.
            continue;
        }
        if expanded_by_key.contains_key(&from_key) || glob_reachable.contains(&from_key) {
            continue;
        }
        if !is_islands_shadow_js_like_file(from) {
            continue;
        }
        let bytes = std::fs::read(from)
            .with_context(|| format!("read raw-mirrored islands shadow file {}", from.display()))?;
        if !bytes_contain_import_meta_glob(&bytes) {
            continue;
        }
        let source = match String::from_utf8(bytes) {
            Ok(source) => source,
            Err(e) => {
                offenders.push(raw_mirrored_import_meta_glob_offender(
                    from,
                    Some(format!("file is not valid UTF-8: {e}")),
                ));
                continue;
            }
        };
        match zfb_build::glob_expand::source_contains_import_meta_glob(&source) {
            Ok(true) => offenders.push(raw_mirrored_import_meta_glob_offender(from, None)),
            Ok(false) => {}
            Err(e) => offenders.push(raw_mirrored_import_meta_glob_offender(
                from,
                Some(format!("{e:#}")),
            )),
        }
    }
    if !offenders.is_empty() {
        return Ok(IslandsShadowOutcome::KeepStopgap(offenders));
    }

    // --- Materialise. ----------------------------------------------------
    let tempdir = tempfile::Builder::new()
        .prefix("zfb-islands-shadow-")
        .tempdir()
        .context("failed to allocate islands shadow tempdir")?;
    let shadow_root = tempdir.path();
    let project_node_modules = detect_project_node_modules(root);
    let source_copy_mode = project_node_modules.is_some() && !read_tsconfig_paths(root).is_empty();
    let preserve_symlinks = !source_copy_mode;

    for from in &to_mirror {
        let rel = match paths.project_local_rel(from) {
            Some(r) => r,
            None => continue,
        };
        let to = shadow_root.join(rel);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create shadow dir {}", parent.display()))?;
        }
        let from_key = paths.path_key(from);
        if let Some(expanded) = expanded_by_key.get(&from_key) {
            // Real expanded copy — NOT a symlink, so esbuild reads the
            // expanded macro from the shadow in both preserve-symlinks and
            // copy-mode branches.
            std::fs::write(&to, expanded.as_bytes())
                .with_context(|| format!("write expanded glob module {}", to.display()))?;
            if let Some(modules) = generated_raw_by_key.get(&from_key) {
                let parent = to.parent().unwrap_or(shadow_root);
                for module in modules {
                    let generated = parent.join(&module.filename);
                    std::fs::write(&generated, module.source.as_bytes()).with_context(|| {
                        format!("write generated raw module {}", generated.display())
                    })?;
                }
            }
        } else if source_copy_mode {
            shadow_copy_file(from, &to).with_context(|| {
                format!("copy shadow file {} -> {}", from.display(), to.display())
            })?;
        } else {
            shadow_symlink(from, &to).with_context(|| {
                format!("symlink shadow file {} -> {}", from.display(), to.display())
            })?;
        }
    }

    // Symlink node_modules as a whole so shadow files' bare imports
    // (`preact`, `@takazudo/zfb/runtime`, …) resolve — esbuild walks up from
    // each shadow file to `<shadow>/node_modules`.
    if let Some(nm) = project_node_modules {
        let shadow_nm = shadow_root.join("node_modules");
        shadow_symlink(&nm, &shadow_nm).with_context(|| {
            format!(
                "symlink shadow node_modules {} -> {}",
                shadow_nm.display(),
                nm.display()
            )
        })?;
    }
    // Note: the user's `tsconfig.json` / `jsconfig.json` are not source files
    // in this mirror set. Islands esbuild still runs with `project_root` as its
    // working dir, and its synthetic-tsconfig path (when plugin aliases or
    // virtual modules require one) is seeded from that project config. The
    // shadow only controls whether project source files are symlinked or real
    // copies before esbuild follows their relative imports.

    // --- Remap island source_paths into the shadow. ----------------------
    let mut remap: HashMap<PathBuf, PathBuf> = HashMap::new();
    for island in islands {
        if let Some(rel) = paths.project_local_rel(&island.source_path) {
            remap.insert(island.source_path.clone(), shadow_root.join(rel));
        }
    }

    Ok(IslandsShadowOutcome::Ready(IslandsShadow {
        _tempdir: tempdir,
        remap,
        preserve_symlinks,
        raw_targets: all_raw_edges
            .into_iter()
            .filter_map(|edge| paths.logical_project_path(&edge.target))
            .collect(),
    }))
}

/// Run the real `build_production_islands_asset` against the
/// project's discovered island set and return its bytes packaged for
/// [`ProductionAssetPipeline`].
///
/// Returns `Ok(None)` when:
///
/// - the project has no `"use client"` components (the scanner
///   returns an empty set), OR
/// - the islands scanner returned a transient error (we surface a
///   warning so the build keeps going — a missing island bundle is
///   an authoring concern, not a hard failure of the build's CSS or
///   page paths), OR
/// - `islands_glob_policy` is [`IslandsGlobPolicy::WarnAndSkip`] and the
///   scanner found `import.meta.glob` reachable from an island (#1387) —
///   a warning is emitted and the rebundle is skipped for this tick.
///
/// Returns `Err` when `islands_glob_policy` is
/// [`IslandsGlobPolicy::HardError`] and the scanner found
/// `import.meta.glob` reachable from an island (#1387) — this is the
/// build-time stopgap for #1385 pt.1: the islands esbuild pipeline does
/// not expand `import.meta.glob` in any form yet, so shipping the literal
/// call to the browser would throw a `TypeError` at hydration instead of
/// failing the build with a clear message.
///
/// On `Ok(Some(_))` the orchestrator hashes the bytes and writes
/// `dist/assets/islands-<hash>.js`. The renderer's HTML references
/// the stable URL (`/assets/islands.js`) which the rewrite step
/// replaces with the hashed form; no stable `islands.js` is written
/// to disk in production (the bundler carries bytes in memory only).
///
/// The second return value is the set of **registered marker names** from
/// `islands_set` — the strings the SSR side will write into
/// `data-zfb-island` / `data-zfb-island-skip-ssr` attributes.  The build
/// pass uses this for the island-marker-check (#984 / #990).  It is empty
/// (not `None`) when no islands were found or when the scanner failed, so
/// the marker-check pass can still warn about rendered markers with zero
/// registered islands.
#[cfg(test)]
pub(crate) fn build_default_islands_payload(
    project_root: &Path,
    user_pages_dir: &Path,
    package_route_entrypoints: &[PathBuf],
    outdir: &Path,
    framework: crate::config::Framework,
    plugin_config: &IslandsPluginConfig,
    islands_glob_policy: IslandsGlobPolicy,
    raw_invalidation: Option<&zfb_build::RawImportInvalidation>,
) -> Result<(
    Option<AssetEmitterPayload>,
    std::collections::BTreeSet<String>,
)> {
    build_default_islands_payload_with_bundle_options(
        project_root,
        user_pages_dir,
        package_route_entrypoints,
        outdir,
        framework,
        None,
        zfb_islands::BundleMode::Production,
        plugin_config,
        islands_glob_policy,
        raw_invalidation,
    )
}

pub(crate) fn build_default_islands_payload_with_bundle_options(
    // The project root — used for esbuild's working dir (tsconfig
    // discovery, entry-tempfile placement) and the node_modules walk.
    project_root: &Path,
    // #1193 / #1191 review (codex P1) — the REAL `project_root/pages` dir
    // to seed USER-page island discovery from. This is deliberately the
    // user's real pages tree, NOT the build pages root: when a package
    // route is present the build pages root is an OVERLAY temp dir that
    // copies only `pages/` (plus generated package modules) and has no
    // `components/`. Seeding the scanner from the overlay strands every
    // user-page island reached via an outside-`pages/` import (e.g.
    // `../components/Counter` or a tsconfig alias) → the island silently
    // vanishes from production whenever ANY package route exists. Seeding
    // from the REAL pages dir resolves those imports against the real tree.
    // It is the DFS entry root only; esbuild still resolves against
    // `project_root`.
    user_pages_dir: &Path,
    // #1191 review (codex P1) — the absolute entrypoints of materialized
    // package routes. Package-route islands are discovered by seeding the
    // scanner with each route's REAL entrypoint (whose relative imports
    // resolve against its real location), NOT via the overlay copy. Empty
    // when there are no package routes. Threading the real entrypoints (vs
    // walking the overlay) keeps package-route island discovery working
    // while the user-page seed uses the real pages tree.
    package_route_entrypoints: &[PathBuf],
    outdir: &Path,
    framework: crate::config::Framework,
    bundle_config: Option<&crate::config::BundleConfig>,
    bundle_mode: zfb_islands::BundleMode,
    plugin_config: &IslandsPluginConfig,
    // Issue #1387 — see [`IslandsGlobPolicy`]: `zfb build` passes
    // `HardError`, `zfb dev`'s `rebundle_islands` passes `WarnAndSkip`.
    islands_glob_policy: IslandsGlobPolicy,
    // Dev-only live invalidation registry. Production passes None.
    raw_invalidation: Option<&zfb_build::RawImportInvalidation>,
) -> Result<(
    Option<AssetEmitterPayload>,
    std::collections::BTreeSet<String>,
)> {
    // Walk the conventional islands roots. The scanner DFS-walks
    // imports starting from each entry path, so seeding with the
    // pages dir is enough — anything reachable through a
    // page → component import chain gets found.
    let mut entries: Vec<std::path::PathBuf> = Vec::new();
    if user_pages_dir.is_dir() {
        for ext in ["tsx", "ts", "jsx", "js"] {
            for entry in walkdir::WalkDir::new(user_pages_dir)
                .into_iter()
                .filter_map(|r| r.ok())
            {
                if entry.file_type().is_file()
                    && entry.path().extension().and_then(|s| s.to_str()) == Some(ext)
                {
                    entries.push(entry.into_path());
                }
            }
        }
    }
    // Seed package-route islands from each route's REAL entrypoint (codex
    // P1). The entrypoint's own relative imports resolve against its real
    // location (same way the bundler/overlay handle package modules), so a
    // `"use client"` component a package page imports is discovered without
    // walking the overlay. Skip entries already covered by the user-pages
    // walk (defensive — a package entrypoint living under `pages/` would be
    // unusual but the DFS dedups by visited-set anyway).
    for entry in package_route_entrypoints {
        if entry.is_file() && !entries.contains(entry) {
            entries.push(entry.clone());
        }
    }
    if entries.is_empty() {
        if let Some(invalidation) = raw_invalidation {
            invalidation.replace_islands(Vec::new());
        }
        return Ok((None, std::collections::BTreeSet::new()));
    }

    // Treat each injected package-route entrypoint as honorary project
    // source for the bare-specifier descent gate (#1268 Symptom 2): a
    // route whose realpath is under `node_modules` must still be able to
    // descend into its `"use client"` island package (directly or through
    // its package-chrome closure), the same single hop a real `pages/`
    // entry gets. Entrypoints outside `node_modules` are ignored by the
    // resolver, so this is a no-op on the conventional-pages path.
    let resolver = FsResolver::new().with_injected_route_roots(package_route_entrypoints);
    let (islands_set, scan_meta) = match scan_islands_with_meta(&entries, &resolver) {
        Ok(result) => result,
        Err(
            error @ (zfb_islands::ScanError::ImportQuery { .. }
            | zfb_islands::ScanError::RawImport { .. }
            | zfb_islands::ScanError::ModuleWorker { .. }
            | zfb_islands::ScanError::SharedWorker { .. }),
        ) => {
            let message = format!("zfb islands: {error}");
            match islands_glob_policy {
                IslandsGlobPolicy::HardError => return Err(anyhow!(message)),
                IslandsGlobPolicy::WarnAndSkip => {
                    output::warn(format!(
                        "{message}. Skipping this islands rebundle; the dev server stays up."
                    ));
                    return Ok((None, std::collections::BTreeSet::new()));
                }
            }
        }
        Err(e) => {
            output::warn(format!(
                "islands scanner failed ({e}); skipping islands asset emission"
            ));
            return Ok((None, std::collections::BTreeSet::new()));
        }
    };
    if let Some(invalidation) = raw_invalidation {
        let paths = IslandsShadowPaths::new(project_root);
        invalidation.replace_islands(scan_meta.raw_import_edges_from_islands.iter().map(|edge| {
            paths
                .logical_project_path(&edge.target)
                .unwrap_or_else(|| edge.target.clone())
        }));
    }

    // Issue #1404 (full #1385 pt.1 fix): when `import.meta.glob` is reachable
    // from a `"use client"` island, materialise a minimal islands shadow —
    // the island graph mirrored under a TempDir with the Vite macro expanded
    // Rust-side (the islands esbuild pipeline cannot expand it itself), the
    // same trick the SSR bundler already uses — and remap the island
    // `source_path`s into it so esbuild bundles the expanded copies. Most
    // shadows run with `--preserve-symlinks`; the project-node_modules +
    // tsconfig-paths shape copies source files instead and omits the flag,
    // mirroring the SSR bundler's copy-mode fallback. Supported forms (eager
    // + string-literal)
    // now WORK; unsupported forms (lazy/default, non-literal, `import()`
    // mode) and glob modules outside the mirrorable tree keep the #1387
    // stopgap below. A graph with no glob/raw/worker preprocessing need takes
    // the fast path (this whole block is skipped): no shadow, `source_path`s
    // untouched, no `--preserve-symlinks`.
    //
    // `_islands_shadow` holds the shadow TempDir alive until AFTER
    // `build_production_islands_asset` runs esbuild below — dropping it early
    // would delete the tree out from under the subprocess. The remap is
    // applied ONLY to the bundle's island slice (built just before the
    // bundle call), NOT to `islands_set` itself, so the marker-name and
    // collision passes below keep seeing the real project paths.
    let mut _islands_shadow: Option<IslandsShadow> = None;
    let mut islands_preserve_symlinks = false;
    if !scan_meta.glob_reachable_from_islands.is_empty()
        || !scan_meta.raw_import_edges_from_islands.is_empty()
        || !scan_meta.module_worker_edges_from_islands.is_empty()
    {
        match materialise_islands_shadow(project_root, &islands_set, &scan_meta)? {
            IslandsShadowOutcome::Ready(shadow) => {
                islands_preserve_symlinks = shadow.preserve_symlinks;
                if let Some(invalidation) = raw_invalidation {
                    invalidation.replace_islands(shadow.raw_targets.iter().cloned());
                }
                _islands_shadow = Some(shadow);
            }
            IslandsShadowOutcome::KeepStopgap(offenders) => {
                // Unsupported `import.meta.glob` form(s), glob module(s)
                // outside the mirrorable tree, or nested globs in raw-
                // mirrored companions remain — retain the #1387 stopgap for
                // exactly these files (hard error for `zfb build`, warn-and-
                // skip for `zfb dev`). Shipping any of them would leave a
                // Vite-only macro in the browser bundle and throw at
                // hydration.
                let files = offenders.join("; ");
                let message = format!(
                    "`import.meta.glob(...)` cannot be safely shipped from one or more \
                     files reachable from a \"use client\" island. zfb can currently \
                     expand eager string-literal globs only when the glob call is in an \
                     island-reachable module that is written as a real shadow copy; \
                     unsupported forms and globs found only in raw-mirrored glob \
                     target/subtree files would ship to the browser unexpanded and throw \
                     at hydration: {files}. Follow the remediation above for each file, \
                     use the eager string-literal form where applicable, replace the glob \
                     with explicit static imports, or move the usage to a server-only \
                     (non-\"use client\") module. Tracked at \
                     https://github.com/Takazudo/zudo-front-builder/issues/1385 and \
                     https://github.com/Takazudo/zudo-front-builder/issues/1412."
                );
                match islands_glob_policy {
                    IslandsGlobPolicy::HardError => {
                        return Err(anyhow!("zfb islands: {message}"));
                    }
                    IslandsGlobPolicy::WarnAndSkip => {
                        output::warn(format!(
                            "zfb islands: {message} Skipping this islands rebundle; the dev \
                             server stays up — fix the file(s) above and save again."
                        ));
                        return Ok((None, std::collections::BTreeSet::new()));
                    }
                }
            }
        }
    }

    // Issue #289: a project may use `<ClientRouter />` without any
    // `"use client"` islands (a static page that only wants View
    // Transitions). When the scanner detected client-router usage, the
    // islands asset still has to be emitted so the runtime's side-effect
    // import ships — so the empty-islands short-circuit below only fires
    // when client-router is NOT in play.
    if islands_set.is_empty() && !scan_meta.uses_client_router {
        // Issue #822: only the loud warning + verify-hint when the scan
        // saw a *near-miss* — a module that looks like it meant to be a
        // `"use client"` island but didn't register one (a misplaced or
        // misspelled directive, or a valid directive with no exported
        // component). For a project that is island-free on purpose
        // (`near_miss_candidates == 0`), the verify-hint is permanent
        // noise, so we demote to a quiet info note with no hint.
        if scan_meta.near_miss_candidates == 0 {
            output::info(
                "no \"use client\" islands found; skipping islands bundle \
                 (no islands asset will be emitted)",
            );
        } else {
            // Issue #122 / #117: this branch used to be silent, which made
            // pnpm-workspace consumers with `"use client"` islands inside a
            // workspace package look "fine" while shipping no client
            // runtime. Surface it loudly so authoring problems (a missing
            // `"use client"` directive, an island reachable only through a
            // path the scanner can't follow) become discoverable.
            output::warn(format!(
                "scanned {} page entr{} but found no \"use client\" islands; \
                 no islands asset will be emitted. \
                 Verify each island module starts with the literal directive \
                 \"use client\" and is reachable from a page in pages/.",
                entries.len(),
                if entries.len() == 1 { "y" } else { "ies" }
            ));
        }
        return Ok((None, std::collections::BTreeSet::new()));
    }

    // Collect the registered marker names now — needed by the build-time
    // island-marker-check pass (#984 / #990) even when esbuild later decides
    // no bundle is needed (client-router-only projects).
    let registered_marker_names: std::collections::BTreeSet<String> =
        islands_set.iter().map(|i| i.marker_name.clone()).collect();

    // #999: scanning `node_modules` for dist-shipped islands makes
    // duplicate marker names far more likely — e.g. a local
    // `ThemeToggle` component and a package-provided `ThemeToggle` from
    // `@takazudo/zudo-doc`. The manifest keys on marker name and keeps
    // only the first by source-path sort order, silently dropping the
    // rest; the dropped island then ships a dead SSR marker that never
    // hydrates. Surface every such collision loudly with BOTH source
    // paths so the author can disambiguate (rename one component, or give
    // it a distinct `displayName`) instead of debugging a silent
    // dead-island. This does not change selection behaviour — it only
    // warns.
    let island_manifest = zfb_islands::Manifest::from_islands(&islands_set);
    for collision in island_manifest.collisions() {
        output::warn(format!(
            "island marker name collision: \"{}\" is produced by two different source files — \
             keeping {} and dropping {}. Only the kept island will hydrate; rename one component \
             or give it a distinct `displayName` so both register under unique marker names.",
            collision.name,
            collision.kept_path.display(),
            collision.dropped_path.display(),
        ));
    }

    // Sub #212 follow-up — extend embedded-binary AND embedded-node_modules
    // extraction to the islands bundler.
    //
    // 1. Binary path: `EsbuildSubprocessConfig::default()` resolves to a
    //    fixed `crates/zfb/binaries/esbuild/esbuild` slot when neither
    //    `binary_path` is overridden nor `ZFB_ESBUILD_BIN` is set; consumer
    //    projects without that workspace dir hit the same "esbuild binary
    //    not found at default slot" failure as the main bundler did before
    //    this PR's other fix. Mirror the wiring applied in `run_build` and
    //    the CSS engine path: when no env override is in play, pre-extract
    //    the embedded esbuild via `embedded_binary("esbuild")` and pin its
    //    path on the config via `with_binary_path`.
    //
    // 2. node_modules resolution: esbuild walks UP from each importing
    //    file's directory looking for `node_modules`. When the consumer
    //    project has no on-disk `node_modules`, every bare import in
    //    user-authored components (`preact/hooks`, `preact/jsx-runtime`,
    //    etc.) and in the synthesised entry (`@takazudo/zfb/runtime`,
    //    `preact`) fails. The main bundler addresses this via a "shadow
    //    tree" with a `node_modules` symlink (see
    //    `crates/zfb-build/src/bundler.rs:751`); the islands bundler runs
    //    esbuild directly against the user's source tree so a shadow tree
    //    is not available. Instead, extract the embedded vendor
    //    (`embedded_node_modules()`) into a tempdir and pass its path via
    //    the `NODE_PATH` env var on the esbuild subprocess. esbuild
    //    consults `NODE_PATH` as a fallback when the upward walk doesn't
    //    find a match — see esbuild's
    //    https://esbuild.github.io/api/#node-paths and the
    //    `with_extra_env` setter on `EsbuildSubprocessConfig`. This
    //    preserves project_root as the working_dir (so tsconfig
    //    discovery and the entry-tempfile placement comment block in
    //    `zfb-islands/src/esbuild.rs::bundle_one_entry` still hold).
    let _embedded_esbuild_handle: Option<tempfile::TempDir>;
    let _embedded_nm_handle: Option<tempfile::TempDir>;
    let mut esbuild_cfg =
        EsbuildSubprocessConfig::default().with_working_dir(project_root.to_path_buf());
    if detect_project_node_modules(project_root).is_some() {
        _embedded_nm_handle = None;
    } else {
        match embedded_node_modules() {
            Ok((handle, nm_path)) => {
                esbuild_cfg = esbuild_cfg.with_extra_env("NODE_PATH", nm_path.into_os_string());
                _embedded_nm_handle = Some(handle);
            }
            Err(e) => {
                output::warn(format!(
                    "could not extract embedded @takazudo packages for islands bundler ({e}); \
                     falling back to project_root node_modules walk"
                ));
                _embedded_nm_handle = None;
            }
        }
    }
    if std::env::var_os("ZFB_ESBUILD_BIN").is_none() {
        match crate::render_pipeline::embedded_binary("esbuild") {
            Ok((handle, path)) => {
                esbuild_cfg = esbuild_cfg.with_binary_path(path);
                _embedded_esbuild_handle = Some(handle);
            }
            Err(e) => {
                output::warn(format!(
                    "could not extract embedded esbuild for islands bundler ({e}); \
                     falling back to default slot resolver"
                ));
                _embedded_esbuild_handle = None;
            }
        }
    } else {
        _embedded_esbuild_handle = None;
    }
    // #261 — wire alias + virtual-module registries from the plugin setup
    // hook into the islands esbuild config. When both are empty (no plugin
    // registrations) no `--alias` flags are added and the bundle output is
    // byte-identical to a build without any plugins (zero-registration
    // regression guard).
    if !plugin_config.alias_entries.is_empty() {
        esbuild_cfg = esbuild_cfg.with_alias_entries(plugin_config.alias_entries.clone());
    }
    if !plugin_config.virtual_modules.is_empty() {
        esbuild_cfg = esbuild_cfg.with_virtual_modules(plugin_config.virtual_modules.clone());
    }

    let bundler = EsbuildSubprocessBundler::new(esbuild_cfg);
    // Thread the scanner's `<ClientRouter />` detection (#289) into the
    // bundler so the synthetic islands entry picks up the client-router
    // runtime's side-effect import. When false the generated entry is
    // byte-identical to a pre-#289 build.
    // Thread the configured framework's JSX import source into the
    // islands BundleConfig (gap: previously hardcoded to Preact via the
    // `BundleConfig::production()` default). This drives BOTH esbuild's
    // `--jsx-import-source` AND — because `produce_bundle_js` derives the
    // mount-glue framework back from this same field — the React vs
    // Preact hydration glue emitted into the shared bundle.
    let islands_jsx_import_source = match framework {
        crate::config::Framework::Preact => zfb_islands::FrameworkKind::Preact,
        crate::config::Framework::React => zfb_islands::FrameworkKind::React,
    }
    .jsx_import_source();
    let bundle_cfg = match bundle_mode {
        zfb_islands::BundleMode::Production => BundleConfig::production(),
        zfb_islands::BundleMode::Development => BundleConfig::dev(),
    }
    .with_outdir(outdir.to_path_buf())
    .with_jsx_import_source(islands_jsx_import_source)
    .with_client_router(scan_meta.uses_client_router)
    .with_loaders(crate::config::resolve_bundle_loaders(bundle_config))
    .with_define(crate::config::resolve_bundle_define(bundle_config))
    // Issue #1413: the shadow carries the exact symlink-mode decision.
    // Most shadows need `--preserve-symlinks`; copy-mode shadows use real
    // source copies and deliberately omit it to mirror the SSR bundler.
    // `false` on the no-shadow fast path keeps byte-identical argv.
    .with_preserve_symlinks(islands_preserve_symlinks);

    // Issue #1404: remap island `source_path`s into the shadow ONLY for the
    // bundle input, so the synthesized esbuild entry imports the in-shadow
    // (expanded) tree. Islands outside the mirrored tree keep their real path
    // (resolved through the shadow's node_modules symlink). On the no-shadow
    // fast path this is a zero-copy borrow of `islands_set` — `source_path`s
    // untouched, byte-identical to before.
    let bundle_islands: std::borrow::Cow<[zfb_islands::Island]> = match &_islands_shadow {
        Some(shadow) if !shadow.remap.is_empty() => {
            let mut remapped = islands_set.clone();
            for island in &mut remapped {
                if let Some(shadow_path) = shadow.remap.get(&island.source_path) {
                    island.source_path = shadow_path.clone();
                }
            }
            std::borrow::Cow::Owned(remapped)
        }
        _ => std::borrow::Cow::Borrowed(&islands_set),
    };

    // `_islands_shadow` MUST stay alive across this call — esbuild reads the
    // shadow tree during `bundle()`. It drops at end of function, after the
    // bundle bytes are in memory.
    match build_production_islands_asset(&bundler, &bundle_islands, &bundle_cfg)? {
        Some(asset) => {
            let companions = asset
                .chunks
                .into_iter()
                .map(|c| CompanionFile {
                    filename: c.filename,
                    bytes: c.bytes,
                })
                .collect();
            Ok((
                Some(AssetEmitterPayload {
                    bytes: asset.bytes,
                    relative_path: asset.relative_path,
                    stable_url: asset.stable_url,
                    companions,
                }),
                registered_marker_names,
            ))
        }
        None => Ok((None, registered_marker_names)),
    }
}

/// Temporary project mirror used when a client-script graph needs a Rust-side
/// pre-pass (`?raw` or a module-worker URL). Executable importers are rewritten
/// as real files, generated `.zfb-raw-*.mjs` wrappers sit beside them, and the
/// rest of the project is copied/symlinked so ordinary relative imports and
/// tsconfig paths continue to resolve from the mirrored entry.
#[derive(Debug)]
struct ClientScriptsPreprocessStage {
    _tempdir: tempfile::TempDir,
    root: PathBuf,
    entries: Vec<zfb_islands::client_scripts::ClientScriptEntry>,
    preserve_symlinks: bool,
    raw_targets: std::collections::BTreeSet<PathBuf>,
    worker_targets: std::collections::BTreeSet<PathBuf>,
    workers_by_entry: std::collections::BTreeMap<String, Vec<ClientScriptWorkerEntry>>,
}

fn materialise_client_preprocess_stage_file(
    physical: &Path,
    logical: &Path,
    to: &Path,
    stage_root: &Path,
    paths: &IslandsShadowPaths<'_>,
    expanded_by_key: &std::collections::HashMap<
        PathBuf,
        zfb_build::raw_import_expand::RawImportExpansion,
    >,
    worker_expanded_by_key: &std::collections::HashMap<PathBuf, String>,
    copy_mode: bool,
    force_copy: bool,
) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create client preprocess stage dir {}", parent.display()))?;
    }
    let key = paths.path_key(logical);
    let canonical_key = physical
        .canonicalize()
        .ok()
        .map(|path| paths.path_key(&path));
    let raw_expansion = expanded_by_key.get(&key).or_else(|| {
        canonical_key
            .as_ref()
            .and_then(|key| expanded_by_key.get(key))
    });
    let worker_expansion = worker_expanded_by_key.get(&key).or_else(|| {
        canonical_key
            .as_ref()
            .and_then(|key| worker_expanded_by_key.get(key))
    });
    let expanded_source = worker_expansion
        .map(String::as_str)
        .or_else(|| raw_expansion.map(|expansion| expansion.expanded_source.as_str()));
    if let Some(expanded_source) = expanded_source {
        std::fs::write(to, expanded_source.as_bytes())
            .with_context(|| format!("write client preprocessed importer {}", to.display()))?;
        let parent = to.parent().unwrap_or(stage_root);
        if let Some(raw_expansion) = raw_expansion {
            for module in &raw_expansion.generated_modules {
                let generated = parent.join(&module.filename);
                std::fs::write(&generated, module.source.as_bytes()).with_context(|| {
                    format!("write client generated raw module {}", generated.display())
                })?;
            }
        }
    } else if copy_mode || force_copy {
        shadow_copy_file(physical, to).with_context(|| {
            format!(
                "copy client preprocess stage {} -> {}",
                physical.display(),
                to.display()
            )
        })?;
    } else {
        shadow_symlink(physical, to).with_context(|| {
            format!(
                "symlink client preprocess stage {} -> {}",
                physical.display(),
                to.display()
            )
        })?;
    }
    Ok(())
}

fn stage_client_script_preprocessing(
    project_root: &Path,
    entries: &[zfb_islands::client_scripts::ClientScriptEntry],
) -> Result<Option<ClientScriptsPreprocessStage>> {
    if entries.is_empty() {
        return Ok(None);
    }
    let roots: Vec<PathBuf> = entries
        .iter()
        .map(|entry| entry.source_path.clone())
        .collect();
    let resolver = FsResolver::new();
    let graph = scan_reachable_modules_with_meta(&roots, &resolver)
        .context("scan client-script graph for ?raw and module-worker preprocessing")?;
    if graph.raw_import_edges.is_empty() && graph.module_worker_edges.is_empty() {
        return Ok(None);
    }

    let paths = IslandsShadowPaths::new(project_root);
    let mut external_entries_without_preprocessing = std::collections::BTreeSet::new();
    let mut worker_sources_by_entry: std::collections::BTreeMap<
        String,
        std::collections::BTreeSet<PathBuf>,
    > = std::collections::BTreeMap::new();
    for entry in entries {
        let entry_graph =
            scan_reachable_modules_with_meta(std::slice::from_ref(&entry.source_path), &resolver)
                .with_context(|| {
                format!(
                    "scan client-script entry {} for preprocessing ownership",
                    entry.source_path.display()
                )
            })?;
        let worker_sources = entry_graph
            .module_worker_edges
            .iter()
            .map(|edge| edge.source_path.clone())
            .collect::<std::collections::BTreeSet<_>>();
        if !worker_sources.is_empty() {
            worker_sources_by_entry.insert(entry.entry_name.clone(), worker_sources);
        }
        if paths.project_local_rel(&entry.source_path).is_none() {
            if !entry_graph.raw_import_edges.is_empty()
                || !entry_graph.module_worker_edges.is_empty()
            {
                return Err(anyhow!(
                    "external client-script entry {} has a graph requiring `?raw`/module-worker \
                     preprocessing; staging is limited to project-local graphs",
                    entry.source_path.display()
                ));
            }
            external_entries_without_preprocessing.insert(entry.source_path.clone());
        }
    }
    let mut expanded_by_key: std::collections::HashMap<
        PathBuf,
        zfb_build::raw_import_expand::RawImportExpansion,
    > = std::collections::HashMap::new();
    let raw_importers: std::collections::BTreeSet<PathBuf> = graph
        .raw_import_edges
        .iter()
        .map(|edge| edge.importer.clone())
        .collect();
    for importer in raw_importers {
        if paths.project_local_rel(&importer).is_none() {
            return Err(anyhow!(
                "client-script raw importer {} is outside the mirrorable project tree; \
                 move it under the project root or remove `?raw`",
                importer.display()
            ));
        }
        let source = std::fs::read_to_string(&importer)
            .with_context(|| format!("read client-script raw importer {}", importer.display()))?;
        let logical_importer = paths.logical_project_path(&importer).ok_or_else(|| {
            anyhow!(
                "client-script raw importer {} has no logical project path",
                importer.display()
            )
        })?;
        let expansion = zfb_build::raw_import_expand::expand_raw_imports(
            &source,
            &logical_importer,
            project_root,
            &|_| false,
        )
        .with_context(|| {
            format!(
                "preprocess client-script raw importer {}",
                importer.display()
            )
        })?;
        expanded_by_key.insert(paths.path_key(&importer), expansion);
    }

    let mut worker_expanded_by_key: std::collections::HashMap<PathBuf, String> =
        std::collections::HashMap::new();
    let mut worker_targets = std::collections::BTreeSet::new();
    let worker_importers: std::collections::BTreeSet<PathBuf> = graph
        .module_worker_edges
        .iter()
        .map(|edge| edge.importer.clone())
        .collect();
    for importer in worker_importers {
        if paths.project_local_rel(&importer).is_none() {
            return Err(anyhow!(
                "client-script module-worker importer {} is outside the mirrorable project \
                 tree; move it under the project root",
                importer.display()
            ));
        }
        let key = paths.path_key(&importer);
        let source = match expanded_by_key.get(&key) {
            Some(expansion) => expansion.expanded_source.clone(),
            None => std::fs::read_to_string(&importer).with_context(|| {
                format!(
                    "read client-script module-worker importer {}",
                    importer.display()
                )
            })?,
        };
        let logical_importer = paths.logical_project_path(&importer).ok_or_else(|| {
            anyhow!(
                "client-script module-worker importer {} has no logical project path",
                importer.display()
            )
        })?;
        let rewrite =
            zfb_build::rewrite_module_worker_urls(&source, &logical_importer, project_root)
                .with_context(|| {
                    format!(
                        "preprocess client-script module-worker importer {}",
                        importer.display()
                    )
                })?;
        for dependency in rewrite.dependencies {
            let logical_dependency = paths
                .logical_project_path(&dependency.dependency)
                .ok_or_else(|| {
                    anyhow!(
                        "client-script module-worker dependency {} is outside the mirrorable project tree",
                        dependency.dependency.display()
                    )
                })?;
            worker_targets.insert(logical_dependency);
        }
        worker_expanded_by_key.insert(key, rewrite.expanded_source);
    }

    let mut raw_targets = std::collections::BTreeSet::new();
    for edge in &graph.raw_import_edges {
        if paths.project_local_rel(&edge.target).is_none() {
            return Err(anyhow!(
                "client-script raw target {} imported from {} is outside the mirrorable \
                 project tree",
                edge.target.display(),
                edge.importer.display()
            ));
        }
        raw_targets.insert(
            paths
                .logical_project_path(&edge.target)
                .unwrap_or_else(|| edge.target.clone()),
        );
    }

    let tempdir = tempfile::Builder::new()
        .prefix("zfb-client-preprocess-")
        .tempdir()
        .context("allocate client-script preprocessing directory")?;
    let root = tempdir.path().to_path_buf();
    let project_node_modules = detect_project_node_modules(project_root);
    let copy_mode = project_node_modules.is_some() && !read_tsconfig_paths(project_root).is_empty();

    for entry in walkdir::WalkDir::new(project_root)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| {
            if is_islands_shadow_pruned_dir(entry) {
                return false;
            }
            if entry.depth() == 1 && entry.file_type().is_dir() {
                let name = entry.file_name().to_string_lossy();
                if matches!(name.as_ref(), "worktrees" | "dist" | "target") {
                    return false;
                }
            }
            true
        })
    {
        let entry = entry.context("walk project for client-script preprocessing")?;
        let from = entry.path();
        let rel = from.strip_prefix(project_root).map_err(|_| {
            anyhow!(
                "client-script preprocessing walked {} outside {}",
                from.display(),
                project_root.display()
            )
        })?;
        if rel.as_os_str().is_empty() {
            continue;
        }
        let to = root.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&to)
                .with_context(|| format!("create client preprocess stage dir {}", to.display()))?;
            continue;
        }
        if entry.path_is_symlink() && from.is_dir() {
            let physical_root = from.canonicalize().with_context(|| {
                format!(
                    "canonicalize client preprocess-stage symlink dir {}",
                    from.display()
                )
            })?;
            let canonical_project = project_root.canonicalize().with_context(|| {
                format!(
                    "canonicalize client preprocess-stage root {}",
                    project_root.display()
                )
            })?;
            if !physical_root.starts_with(&canonical_project) {
                continue;
            }
            std::fs::create_dir_all(&to)
                .with_context(|| format!("create client preprocess stage dir {}", to.display()))?;
            for nested in walkdir::WalkDir::new(&physical_root)
                .follow_links(true)
                .sort_by_file_name()
                .into_iter()
                .filter_entry(|nested| !is_islands_shadow_pruned_dir(nested))
            {
                let nested = nested.with_context(|| {
                    format!(
                        "walk client preprocess-stage symlink dir {}",
                        from.display()
                    )
                })?;
                let physical = nested.path();
                let nested_rel = physical.strip_prefix(&physical_root).map_err(|_| {
                    anyhow!(
                        "client preprocess-stage symlink walk escaped {} via {}",
                        physical_root.display(),
                        physical.display()
                    )
                })?;
                let nested_to = to.join(nested_rel);
                let logical = from.join(nested_rel);
                if nested.file_type().is_dir() {
                    std::fs::create_dir_all(&nested_to).with_context(|| {
                        format!("create client preprocess stage dir {}", nested_to.display())
                    })?;
                    continue;
                }
                if !nested.file_type().is_file() {
                    continue;
                }
                let canonical_file = physical.canonicalize().unwrap_or_else(|_| physical.into());
                if !canonical_file.starts_with(&canonical_project) {
                    continue;
                }
                materialise_client_preprocess_stage_file(
                    physical,
                    &logical,
                    &nested_to,
                    &root,
                    &paths,
                    &expanded_by_key,
                    &worker_expanded_by_key,
                    copy_mode,
                    true,
                )?;
            }
            continue;
        }
        // `WalkDir` reports a symlinked file as a symlink when link
        // following is disabled. Materialise it as a file in this temporary
        // mirror so the staged graph cannot escape back to an unexpanded raw
        // importer through the original symlink.
        let is_symlinked_file = entry.path_is_symlink() && from.is_file();
        if !entry.file_type().is_file() && !is_symlinked_file {
            continue;
        }
        materialise_client_preprocess_stage_file(
            from,
            from,
            &to,
            &root,
            &paths,
            &expanded_by_key,
            &worker_expanded_by_key,
            copy_mode,
            is_symlinked_file,
        )?;
    }

    if let Some(node_modules) = project_node_modules {
        shadow_symlink(&node_modules, &root.join("node_modules")).with_context(|| {
            format!(
                "symlink client preprocess stage node_modules {} -> {}",
                root.join("node_modules").display(),
                node_modules.display()
            )
        })?;
    }

    let staged_entries = entries
        .iter()
        .map(|entry| {
            let Some(rel) = paths.project_local_rel(&entry.source_path) else {
                if external_entries_without_preprocessing.contains(&entry.source_path) {
                    return Ok(entry.clone());
                }
                return Err(anyhow!(
                    "client-script entry {} requiring preprocessing is outside the project root",
                    entry.source_path.display()
                ));
            };
            Ok(zfb_islands::client_scripts::ClientScriptEntry {
                entry_name: entry.entry_name.clone(),
                source_path: root.join(rel),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let mut workers_by_entry = std::collections::BTreeMap::new();
    let mut filename_owners: std::collections::BTreeMap<String, PathBuf> =
        std::collections::BTreeMap::new();
    for (entry_name, sources) in worker_sources_by_entry {
        let mut workers = Vec::with_capacity(sources.len());
        for source in sources {
            let rel = paths.project_local_rel(&source).ok_or_else(|| {
                anyhow!(
                    "client-script module-worker source {} is outside the mirrorable project tree",
                    source.display()
                )
            })?;
            let logical_source = project_root.join(&rel);
            let filename = zfb_types::module_worker_filename(project_root, &logical_source)
                .map_err(|error| anyhow!("client-script module-worker naming failed: {error}"))?;
            if let Some(existing) = filename_owners.get(&filename) {
                if existing != &logical_source {
                    return Err(anyhow!(
                        "client-script module-worker filename collision for {filename:?}: {} vs {}",
                        existing.display(),
                        logical_source.display()
                    ));
                }
            } else {
                filename_owners.insert(filename.clone(), logical_source);
            }
            workers.push(ClientScriptWorkerEntry {
                filename,
                source_path: root.join(rel),
            });
        }
        workers.sort_by(|left, right| left.filename.cmp(&right.filename));
        workers_by_entry.insert(entry_name, workers);
    }

    let entry_filenames = staged_entries
        .iter()
        .map(|entry| {
            (
                zfb_types::stable_client_script_filename(&entry.entry_name),
                entry.source_path.as_path(),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    for (filename, worker_source) in &filename_owners {
        if let Some(entry_source) = entry_filenames.get(filename) {
            return Err(anyhow!(
                "client-script output filename collision for {filename:?}: entry {} vs module worker {}",
                entry_source.display(),
                worker_source.display()
            ));
        }
    }

    Ok(Some(ClientScriptsPreprocessStage {
        _tempdir: tempdir,
        root,
        entries: staged_entries,
        preserve_symlinks: !copy_mode,
        raw_targets,
        worker_targets,
        workers_by_entry,
    }))
}

/// Discover `*.client.{ts,tsx,js,jsx}` files under the conventional
/// project roots, bundle each with esbuild, and return their bytes-only
/// payloads for [`ProductionAssetPipeline`].
///
/// Returns an empty Vec when no client-script entries are found. In that
/// case the caller should NOT inject any `<script>` tags for client
/// scripts; the production pipeline treats the empty Vec as "no
/// client-script assets to emit" and skips all client-script emission —
/// builds without client scripts remain byte-identical to before.
///
/// On `Ok(payloads)` each payload carries the stable URL constant from
/// `zfb_types::stable_client_script_url(entry_name)` and the relative
/// path `assets/client/<name>.js`. The `apply_asset_url_base` step later
/// re-prefixes those stable URLs when `config.base` is set, keeping the
/// renderer-emitted reference and the `boundary_replace` rewrite key in
/// sync.
///
/// ## `import.meta.glob` is NOT supported in client scripts (issue #1404)
///
/// The islands `import.meta.glob` expansion remains deliberately unsupported
/// for client scripts. A graph containing `?raw` or a supported module worker
/// gets a conditional preprocessing mirror, but that mirror does not expand a
/// glob call. Thus a client script (or transitive module) containing
/// `import.meta.glob(...)` still ships that macro unexpanded and throws at
/// runtime. Do not over-claim glob support in the docs (#1406). Graphs without
/// either preprocessing feature keep the direct real-project-tree fast path.
pub(crate) fn build_default_client_scripts_payloads(
    project_root: &Path,
    outdir: &Path,
    framework: crate::config::Framework,
    registered: &zfb_build::ClientEntryList,
    bundle_config: Option<&crate::config::BundleConfig>,
) -> Result<Vec<AssetEmitterPayload>> {
    let (mut entries, collisions) =
        discover_client_scripts(project_root).context("client-script discovery failed")?;

    // Fail loudly on duplicate entry names — mirrors the islands
    // scanner's behavior for duplicate marker names.
    if !collisions.is_empty() {
        let details: Vec<String> = collisions
            .iter()
            .map(|c| {
                format!(
                    "  `{}`: {} vs {}",
                    c.name,
                    c.kept_path.display(),
                    c.dropped_path.display()
                )
            })
            .collect();
        return Err(anyhow::anyhow!(
            "duplicate client-script entry names found (entry names must be unique across all \
             discovery roots):\n{}",
            details.join("\n")
        ));
    }

    // #1196 — merge package-registered client entries (from `addClientEntry`
    // in the plugin `setup` hook) into the discovered set. User-authored
    // `*.client.*` files win on name collision (user file is already in
    // `entries`); a package entry whose name already exists in the
    // discovered set is silently dropped so presets can be adopted without
    // requiring users to remove their own copy.
    {
        let existing_names: std::collections::HashSet<String> =
            entries.iter().map(|e| e.entry_name.clone()).collect();
        for ce in registered.iter() {
            if !existing_names.contains(&ce.entry_name) {
                entries.push(zfb_islands::client_scripts::ClientScriptEntry {
                    entry_name: ce.entry_name.clone(),
                    source_path: ce.entrypoint.clone(),
                });
            }
        }
    }

    if entries.is_empty() {
        return Ok(Vec::new());
    }

    let preprocess_stage = stage_client_script_preprocessing(project_root, &entries)?;
    let (bundle_entries, bundler_working_dir, preserve_symlinks) = match preprocess_stage.as_ref() {
        Some(stage) => (
            stage.entries.as_slice(),
            stage.root.clone(),
            stage.preserve_symlinks,
        ),
        None => (entries.as_slice(), project_root.to_path_buf(), false),
    };

    // Reuse the same embedded-esbuild + embedded-node_modules wiring as
    // `build_default_islands_payload`. Client scripts are plain TS/JS
    // files so the same NODE_PATH resolution strategy applies.
    let _embedded_esbuild_handle: Option<tempfile::TempDir>;
    let _embedded_nm_handle: Option<tempfile::TempDir>;
    let mut esbuild_cfg = EsbuildSubprocessConfig::default().with_working_dir(bundler_working_dir);
    if detect_project_node_modules(project_root).is_some() {
        _embedded_nm_handle = None;
    } else {
        match embedded_node_modules() {
            Ok((handle, nm_path)) => {
                esbuild_cfg = esbuild_cfg.with_extra_env("NODE_PATH", nm_path.into_os_string());
                _embedded_nm_handle = Some(handle);
            }
            Err(e) => {
                output::warn(format!(
                    "could not extract embedded @takazudo packages for client-script bundler \
                     ({e}); falling back to project_root node_modules walk"
                ));
                _embedded_nm_handle = None;
            }
        }
    }
    if std::env::var_os("ZFB_ESBUILD_BIN").is_none() {
        match crate::render_pipeline::embedded_binary("esbuild") {
            Ok((handle, path)) => {
                esbuild_cfg = esbuild_cfg.with_binary_path(path);
                _embedded_esbuild_handle = Some(handle);
            }
            Err(e) => {
                output::warn(format!(
                    "could not extract embedded esbuild for client-script bundler ({e}); \
                     falling back to default slot resolver"
                ));
                _embedded_esbuild_handle = None;
            }
        }
    } else {
        _embedded_esbuild_handle = None;
    }

    let bundler = EsbuildSubprocessBundler::new(esbuild_cfg);
    // JSX is harmless for plain .ts files; reuse the islands JSX import
    // source so Preact/React aliases apply consistently to any .tsx
    // client scripts.
    let client_scripts_jsx_import_source = match framework {
        crate::config::Framework::Preact => FrameworkKind::Preact,
        crate::config::Framework::React => FrameworkKind::React,
    }
    .jsx_import_source();
    let bundle_cfg = BundleConfig::production()
        .with_outdir(outdir.to_path_buf())
        .with_jsx_import_source(client_scripts_jsx_import_source)
        .with_loaders(crate::config::resolve_bundle_loaders(bundle_config))
        .with_define(crate::config::resolve_bundle_define(bundle_config))
        .with_preserve_symlinks(preserve_symlinks);

    let empty_workers = std::collections::BTreeMap::new();
    let workers_by_entry = preprocess_stage
        .as_ref()
        .map(|stage| &stage.workers_by_entry)
        .unwrap_or(&empty_workers);
    let assets = build_production_client_scripts_with_workers(
        &bundler,
        bundle_entries,
        workers_by_entry,
        &bundle_cfg,
    )
    .context("client-script bundler failed")?;

    Ok(assets
        .into_iter()
        .map(|a| AssetEmitterPayload {
            bytes: a.bytes,
            relative_path: a.relative_path,
            stable_url: a.stable_url,
            companions: a
                .companions
                .into_iter()
                .map(|companion| CompanionFile {
                    filename: companion.filename,
                    bytes: companion.bytes,
                })
                .collect(),
        })
        .collect())
}

/// Discover, bundle (dev mode — no minification), and write all
/// `*.client.{ts,tsx,js,jsx}` entries to `dist_root/assets/client/<name>.js`.
///
/// This is the **dev-path** equivalent of [`build_default_client_scripts_payloads`]:
/// it writes stable (un-hashed) files directly to disk so the dev server's
/// `ServeDir` can serve `GET /assets/client/<name>.js` immediately.
///
/// ## Stale-file pruning
///
/// `prev_output_filenames` is the set of flat entry/worker filenames written
/// by the *previous* call. Any previous filename absent from the new output
/// set is deleted, including workers whose constructor import disappeared.
/// Pass an empty set on boot and retain the returned set for the next call.
///
/// ## Return value
///
/// Returns `(changed, current_output_filenames, raw_targets, worker_targets)`
/// where:
/// - `changed` is `true` when at least one file was written with new or
///   changed bytes (or any stale file was pruned). The dev-server wires
///   this to a `ReloadEvent::Page`.
/// - `current_output_filenames` is the set of entry and worker basenames that
///   were just written — pass it as `prev_output_filenames` on the next call.
/// - `raw_targets` is the logical original terminal-target set for dev
///   invalidation; the shared registry retains lexical + canonical aliases.
/// - `worker_targets` is the complete first-party worker dependency closure;
///   edits to any member must rerun the client-script pipeline.
fn write_dev_client_script_output_if_changed(path: &Path, bytes: &[u8]) -> Result<bool> {
    if std::fs::read(path).unwrap_or_default() == bytes {
        return Ok(false);
    }
    std::fs::write(path, bytes)
        .with_context(|| format!("client-scripts dev: failed to write {}", path.display()))?;
    Ok(true)
}

fn prune_dev_client_script_outputs(
    client_dir: &Path,
    previous: &std::collections::HashSet<String>,
    current: &std::collections::HashSet<String>,
) -> bool {
    let mut changed = false;
    for stale_filename in previous.difference(current) {
        let stale_path = client_dir.join(stale_filename);
        if !stale_path.exists() {
            continue;
        }
        if let Err(error) = std::fs::remove_file(&stale_path) {
            output::warn(format!(
                "client-scripts dev: failed to prune stale file {}: {error}",
                stale_path.display()
            ));
        } else {
            changed = true;
        }
    }
    changed
}

pub(crate) fn build_dev_client_scripts_to_disk(
    project_root: &Path,
    // Where dev client scripts are written + served from (issue #1189: the
    // isolated `.zfb-build/dev-assets` root, NOT the build-shared `dist/`).
    assets_root: &Path,
    framework: crate::config::Framework,
    bundle_config: Option<&crate::config::BundleConfig>,
    prev_output_filenames: &std::collections::HashSet<String>,
    registered: &zfb_build::ClientEntryList,
) -> Result<(
    bool,
    std::collections::HashSet<String>,
    std::collections::BTreeSet<PathBuf>,
    std::collections::BTreeSet<PathBuf>,
)> {
    let (mut entries, collisions) =
        discover_client_scripts(project_root).context("client-script discovery failed")?;

    // #1196 — merge package-registered client entries. User-authored files win
    // on name collision (silently drop the registered entry if already found).
    {
        let existing_names: std::collections::HashSet<String> =
            entries.iter().map(|e| e.entry_name.clone()).collect();
        for ce in registered.iter() {
            if !existing_names.contains(&ce.entry_name) {
                entries.push(zfb_islands::client_scripts::ClientScriptEntry {
                    entry_name: ce.entry_name.clone(),
                    source_path: ce.entrypoint.clone(),
                });
            }
        }
    }

    // Non-fatal collision warning (dev mode is lenient — the user sees
    // the first winning entry rather than a hard build failure, matching
    // the behaviour of not-yet-deployed production builds during active
    // development).
    if !collisions.is_empty() {
        for c in &collisions {
            output::warn(format!(
                "client-script name collision: `{}` is claimed by both {} and {} \
                 — only {} will be bundled",
                c.name,
                c.kept_path.display(),
                c.dropped_path.display(),
                c.kept_path.display(),
            ));
        }
    }

    let client_dir = assets_root
        .join(zfb_types::DIST_ASSETS_DIR)
        .join(zfb_types::DIST_CLIENT_SCRIPTS_DIR);

    let preprocess_stage = if entries.is_empty() {
        None
    } else {
        stage_client_script_preprocessing(project_root, &entries)?
    };
    let mut current_output_filenames: std::collections::HashSet<String> = entries
        .iter()
        .map(|entry| zfb_types::stable_client_script_filename(&entry.entry_name))
        .collect();
    if let Some(stage) = &preprocess_stage {
        current_output_filenames.extend(
            stage
                .workers_by_entry
                .values()
                .flatten()
                .map(|worker| worker.filename.clone()),
        );
    }

    // Prune stale entry and worker outputs before writing. Because the set is
    // filename-based, removing a Worker constructor prunes its stable
    // companion even while the owning client entry remains present.
    let mut any_changed = prune_dev_client_script_outputs(
        &client_dir,
        prev_output_filenames,
        &current_output_filenames,
    );

    if entries.is_empty() {
        return Ok((
            any_changed,
            current_output_filenames,
            std::collections::BTreeSet::new(),
            std::collections::BTreeSet::new(),
        ));
    }

    let (bundle_entries, bundler_working_dir, preserve_symlinks, raw_targets, worker_targets) =
        match preprocess_stage.as_ref() {
            Some(stage) => (
                stage.entries.as_slice(),
                stage.root.clone(),
                stage.preserve_symlinks,
                stage.raw_targets.clone(),
                stage.worker_targets.clone(),
            ),
            None => (
                entries.as_slice(),
                project_root.to_path_buf(),
                false,
                std::collections::BTreeSet::new(),
                std::collections::BTreeSet::new(),
            ),
        };

    // Set up the esbuild subprocess — same wiring as `build_default_client_scripts_payloads`
    // but using `BundleConfig::dev()` (no minification, sourcemaps on).
    let _embedded_esbuild_handle: Option<tempfile::TempDir>;
    let _embedded_nm_handle: Option<tempfile::TempDir>;
    let mut esbuild_cfg = EsbuildSubprocessConfig::default().with_working_dir(bundler_working_dir);
    if detect_project_node_modules(project_root).is_some() {
        _embedded_nm_handle = None;
    } else {
        match embedded_node_modules() {
            Ok((handle, nm_path)) => {
                esbuild_cfg = esbuild_cfg.with_extra_env("NODE_PATH", nm_path.into_os_string());
                _embedded_nm_handle = Some(handle);
            }
            Err(e) => {
                output::warn(format!(
                    "could not extract embedded @takazudo packages for client-script bundler \
                     ({e}); falling back to project_root node_modules walk"
                ));
                _embedded_nm_handle = None;
            }
        }
    }
    if std::env::var_os("ZFB_ESBUILD_BIN").is_none() {
        match crate::render_pipeline::embedded_binary("esbuild") {
            Ok((handle, path)) => {
                esbuild_cfg = esbuild_cfg.with_binary_path(path);
                _embedded_esbuild_handle = Some(handle);
            }
            Err(e) => {
                output::warn(format!(
                    "could not extract embedded esbuild for client-script bundler ({e}); \
                     falling back to default slot resolver"
                ));
                _embedded_esbuild_handle = None;
            }
        }
    } else {
        _embedded_esbuild_handle = None;
    }

    let bundler = EsbuildSubprocessBundler::new(esbuild_cfg);
    let jsx_import_source = match framework {
        crate::config::Framework::Preact => FrameworkKind::Preact,
        crate::config::Framework::React => FrameworkKind::React,
    }
    .jsx_import_source();
    let bundle_cfg = BundleConfig::dev()
        .with_outdir(assets_root.to_path_buf())
        .with_jsx_import_source(jsx_import_source)
        .with_loaders(crate::config::resolve_bundle_loaders(bundle_config))
        .with_define(crate::config::resolve_bundle_define(bundle_config))
        .with_preserve_symlinks(preserve_symlinks);

    if let Some(parent) = client_dir.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "client-scripts dev: failed to create parent dir {}",
                parent.display()
            )
        })?;
    }
    std::fs::create_dir_all(&client_dir).with_context(|| {
        format!(
            "client-scripts dev: failed to create client dir {}",
            client_dir.display()
        )
    })?;

    let empty_workers = std::collections::BTreeMap::new();
    let workers_by_entry = preprocess_stage
        .as_ref()
        .map(|stage| &stage.workers_by_entry)
        .unwrap_or(&empty_workers);
    let mut emitted_companions: std::collections::BTreeMap<String, Vec<u8>> =
        std::collections::BTreeMap::new();
    for entry in bundle_entries {
        let workers = workers_by_entry
            .get(&entry.entry_name)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let output = bundler
            .bundle_client_script_file_with_workers(
                &entry.entry_name,
                &entry.source_path,
                workers,
                &bundle_cfg,
            )
            .with_context(|| {
                format!(
                    "client-scripts dev: bundler failed for entry `{}` ({})",
                    entry.entry_name,
                    entry.source_path.display()
                )
            })?;

        let out_path = client_dir.join(zfb_types::stable_client_script_filename(&entry.entry_name));

        // Only write (and signal changed) when the bytes differ from
        // what's already on disk — avoids spurious page reloads on
        // no-op saves.
        let new_bytes = output.js.as_bytes();
        if write_dev_client_script_output_if_changed(&out_path, new_bytes)? {
            any_changed = true;
        }

        for companion in output.companions {
            if let Some(previous) = emitted_companions.get(&companion.filename) {
                if previous != &companion.bytes {
                    return Err(anyhow!(
                        "client-scripts dev: deterministic module-worker filename collision for {:?} produced different bytes",
                        companion.filename
                    ));
                }
                continue;
            }
            let companion_path = client_dir.join(&companion.filename);
            if write_dev_client_script_output_if_changed(&companion_path, &companion.bytes)? {
                any_changed = true;
            }
            emitted_companions.insert(companion.filename, companion.bytes);
        }
    }

    Ok((
        any_changed,
        current_output_filenames,
        raw_targets,
        worker_targets,
    ))
}

/// Drive the build for a fully-resolved input set. Returns the number
/// of pages written and the postBuild route manifest (#262).
fn run_build<R: BuildRunner, A: AdapterRunner>(
    args: BuildArgsResolved<'_, R, A>,
) -> Result<(usize, zfb_build::PostBuildRouteManifest)> {
    let BuildArgsResolved {
        project_root,
        build_pages_root,
        user_pages_dir,
        package_route_entrypoints,
        outdir,
        config,
        routes,
        runner,
        adapter_runner,
        plugin_alias_entries,
        plugin_virtual_modules,
        minify_html,
    } = args;

    // Resolve the adapter choice up front so we can fail fast if the
    // user wrote an empty string (typo) into the config. The choice
    // is consumed twice: once to decide whether SSR routes are even
    // allowed, and once after the build to wrap the SSR bundle into
    // the deploy-target shape.
    let adapter = AdapterChoice::from_config(config.adapter.as_deref())
        .context("invalid `adapter` value in zfb.config.json")?;

    // Build the renderer-shaped views of the route table.
    let RouteUniversePlan {
        mut static_routes,
        deferred_dynamic,
    } = build_route_universe(routes);
    let prerender_map = build_prerender_map(routes, project_root, |msg| output::warn(msg));

    // Pre-filter: split deferred_dynamic by prerender flag, mirroring dev.rs.
    //
    // SSR (`prerender = false`) dynamic routes bypass `paths()` expansion
    // entirely — they have no concrete URL list to enumerate and are handled
    // at request time by the runtime SSR adapter. Passing them through
    // `expand_dynamic_routes` would (a) fire a misleading
    // "no paths() export" warning for catch-all pages that legitimately
    // omit `paths()`, and (b) waste a V8 round-trip that can only fail.
    //
    // SSG (`prerender = true`, or no `prerender` key → default true) routes
    // keep the existing two-phase expansion path.
    //
    // The pre-filter mirrors the existing dev.rs:1346-1351 split so both
    // commands share the same semantics (see issue #520 / #517).
    let (ssr_deferred, ssg_deferred): (
        Vec<crate::render_pipeline::PendingDynamicRoute>,
        Vec<crate::render_pipeline::PendingDynamicRoute>,
    ) = deferred_dynamic
        .into_iter()
        .partition(|d| crate::render_pipeline::is_ssr_route(&prerender_map, &d.template));

    // Phase 1 — static paths() expansion (SSG routes only).
    //
    // Try static `paths()` extraction for every SSG dynamic route. Resolved
    // entries fold into the same `route_universe` as the static routes.
    // Entries whose `paths()` is non-literal (e.g. they `await import`
    // or query a content collection) are collected into
    // `still_deferred` for Phase 2.
    //
    // A missing `paths()` on an SSG dynamic route is now a hard build error
    // (see `expand_dynamic_routes` in render_pipeline.rs): without one the
    // route produces no pages and would silently 404 at serve time.
    let mut paths_cache = PathsCache::new();
    let expansion = expand_dynamic_routes(&ssg_deferred, project_root, &mut paths_cache)?;
    static_routes.extend(expansion.resolved);
    let still_deferred = expansion.deferred;

    // Phase 2 — embed content snapshot in the bundle.
    //
    // Build the content snapshot from the configured collections and
    // embed it in the worker bundle. This serves TWO consumers:
    //
    // 1. Runtime `paths()` calls — pages whose `paths()` calls
    //    `getCollection(...)` need the snapshot in the worker bundle so
    //    the `/__paths__/<route>` endpoint can return real entries.
    //
    // 2. `getStaticProps` calls — static pages (no dynamic `[slug].tsx`)
    //    that call `getCollection(...)` inside `getStaticProps` also need
    //    the snapshot at render time. The previous `!still_deferred.is_empty()`
    //    gate skipped snapshot construction for projects with no deferred
    //    `paths()`, which broke `getCollection(...)` returning `[]` for
    //    these pages (issue #495).
    //
    // The gate is therefore removed. `build_content_snapshot_json` already
    // short-circuits to `None` when `config.collections.is_empty()`, so
    // projects without any collections still pay nothing — no file walk,
    // no JSON serialisation.
    //
    // Errors are non-fatal: if the collection root is missing or a file is
    // malformed, we warn and fall back to an empty snapshot. The build
    // will proceed; pages that depend on the collection data will see empty
    // `getCollection(...)` results at render time.
    //
    // Snapshot construction is shared with `zfb dev` via
    // `build_content_snapshot_json` so both commands produce byte-identical
    // snapshots.
    let content_snapshot_json = build_content_snapshot_json(project_root, config);

    if static_routes.is_empty() && still_deferred.is_empty() && ssr_deferred.is_empty() {
        // Stay user-friendly: an all-dynamic project where every page
        // also failed static expansion still produces a valid build
        // artifact (an empty dist), but the user has clearly not gotten
        // what they asked for — both warn and exit happy. This matches
        // the previous "no pages found" behaviour shape so existing CI
        // configs don't regress.
        output::warn(
            "no routes to render; dist will be empty (every dynamic route deferred to runtime evaluation)",
        );
        return Ok((0, zfb_build::PostBuildRouteManifest::empty()));
    }

    // Adapter precondition check.
    //
    // A route with `prerender = false` cannot be served as a static
    // file — it needs the runtime SSR adapter to produce a deploy-
    // shaped wrapper. If the user has SSR routes but no adapter
    // configured, fail fast HERE (before the expensive bundle +
    // renderer boot) with a pointer at the offending route.
    //
    // We look in TWO places:
    //
    // 1. `static_routes` — the resolved static + statically-expanded
    //    dynamic routes whose `paths()` returned a literal array.
    // 2. `ssr_deferred` — SSR dynamic routes that were pre-filtered out
    //    before `expand_dynamic_routes`. These have no concrete URLs yet
    //    (their template IS the route key) and must also be flagged here.
    //
    // `still_deferred` is intentionally NOT consulted here: after the
    // `partition` above splits `deferred_dynamic` by the prerender flag,
    // every entry in `still_deferred` came from `ssg_deferred` and is
    // therefore SSG by construction. Including it here would be dead
    // code — every SSR-without-paths case lives in `ssr_deferred`.
    //
    // Missing the deferred set would let `output: "static"` /
    // `adapter = "none"` proceed on a project that obviously needs
    // SSR — exactly the contradiction these checks exist to catch.
    let ssr_route_refs: Vec<SsrRouteRef<'_>> = static_routes
        .iter()
        .filter(|entry| is_ssr_route(&prerender_map, &entry.route_key))
        .map(|entry| SsrRouteRef {
            route_key: entry.route_key.as_str(),
            url_path: entry.url_path.as_str(),
        })
        .chain(ssr_deferred.iter().map(|d| SsrRouteRef {
            route_key: d.template.as_str(),
            // SSR deferred routes have no concrete URL yet — the template IS
            // the most specific identifier available.
            url_path: d.template.as_str(),
        }))
        .collect();
    ensure_no_ssr_without_adapter(&adapter, &ssr_route_refs)?;

    // Resolve the V8-mode decision from `config.output` + the detected
    // SSR-route set (sub-task 4.1b / issue #373). The load-bearing
    // behaviour today is the precondition error returned when
    // `output: "static"` collides with detected SSR routes — this
    // fires BEFORE the expensive bundle + V8 host boot so the user
    // sees a clear, actionable error pointing at both the config
    // setting and the offending route.
    //
    // The resolved `_v8_mode` value is otherwise observational on the
    // shipping binary: the SSG render path always boots V8 (it's the
    // only renderer in this workspace), and `embed_v8 = off` is
    // already a hard `bail!` at `pub async fn run`. The mode is
    // computed here so future shipping paths (Tauri sidecar /
    // standalone SSR server) and the unit tests can read the same
    // decision; see `resolve_v8_mode` for the full decision tree.
    let _v8_mode = resolve_v8_mode(config.output, &ssr_route_refs)?;

    // Capture the SSR route key set for the deploy-adapter's runtime-only
    // bundle pass (zfb#283). The set is computed from the same source as
    // `ssr_route_refs` above; keeping it as an owned `BTreeSet` lets the
    // borrow above end before the runtime bundle (built much later in the
    // function) consumes the data.
    //
    // Contract: `worker_only_routes` is Hono-form (e.g. `/blog/:slug{.+}`),
    // populated from `route_key` (which equals `d.template` for deferred
    // routes — both are Hono-form). `BundlerInput::worker_only_routes`
    // filters against `RouteEntry::entry_key`, which the bundler also stores
    // in Hono-form (`bracket_to_hono(&route)`). The two sets therefore match
    // by exact string equality for every route shape, including catch-alls
    // (zfb#532).
    //
    // We must consult TWO sources, mirroring `ssr_route_refs`:
    //
    // 1. `static_routes` — keyed by `route_key`.
    // 2. `ssr_deferred` — SSR dynamic routes pre-filtered before expansion.
    //    Their template IS the route key.
    //
    // `still_deferred` is intentionally NOT consulted: post-partition it
    // only contains SSG routes (see the comment on `ssr_route_refs`).
    //
    // Missing `ssr_deferred` would tree-shake SSR catch-all pages (no paths()
    // export) out of the deploy adapter's runtime worker bundle — exactly the
    // #373 / #517 regression shape. The explicit chain here is the replacement
    // for the previous `still_deferred` side-effect path (issue #520).
    let ssr_route_keys_for_runtime_bundle: std::collections::BTreeSet<String> = static_routes
        .iter()
        .filter(|entry| is_ssr_route(&prerender_map, &entry.route_key))
        .map(|entry| entry.route_key.clone())
        .chain(ssr_deferred.iter().map(|d| d.template.clone()))
        .collect();

    // Fail fast if the runtime npm package isn't on disk — the renderer
    // will fail later anyway, but we can give the user an actionable
    // hint right at build start.
    check_runtime_installed(project_root)?;

    // ZFB_DEBUG_SNAPSHOT telemetry probe.
    //
    // The bundler currently emits a placeholder empty
    // `contentSnapshot`, so to give users a way to monitor V8 RAM
    // pressure today we materialise the snapshot here when the flag
    // is set and let `build_snapshot` log a one-line summary to
    // stderr. The result is discarded — this code path is strictly
    // telemetry. Errors are non-fatal: a probe failure must not
    // break the build.
    //
    // See README.md "Limits" for the user-facing contract.
    maybe_probe_content_snapshot(project_root, config);

    // 1. Bundle.
    //
    // The content snapshot (when present) is embedded in the worker
    // bundle so runtime `paths()` calls can resolve `getCollection(...)`.
    //
    // The full ~25-field BundlerInput assembly is shared with `zfb dev`
    // via `commands::bundler_input::assemble_bundler_input`. The two
    // per-command differences passed here:
    //   • BundleMode::Production  (dev uses Development)
    //   • CssModuleFailMode::HardFail  (dev uses WarnAndEmpty)
    let crate::commands::bundler_input::AssembledBundlerInput {
        bundler_input,
        _node_modules_handle: _embedded_nm_handle,
        _esbuild_handle: _embedded_esbuild_handle,
    } = crate::commands::bundler_input::assemble_bundler_input(
        project_root,
        config,
        BundleMode::Production,
        crate::commands::bundler_input::CssModuleFailMode::HardFail,
        content_snapshot_json,
        plugin_alias_entries,
        plugin_virtual_modules,
        // `zfb build` keeps the per-call embedded-esbuild extraction —
        // one extraction per build; the per-tick reuse (#994 item A) is a
        // dev-only concern.
        None,
        // #1193 — point the bundler at the SAME pages root the router scan
        // used (the overlay when package routes are present), so the
        // bundle's page imports include package-owned routes.
        Some(build_pages_root),
        // #1230 — the additive injected-route root is a `zfb dev`-only seam
        // (the build overlay above already merges package routes into
        // `build_pages_root`); build passes `None`.
        None,
    )?;

    // Snapshot the bundler input before consuming it so the runtime-only
    // bundle pass (zfb#283) can run later in this function with the same
    // shadow setup / esbuild handle / node_modules / plugin wiring, just
    // narrowed to SSR-only routes via `worker_only_routes`.
    //
    // Clones are cheap relative to the bundle step itself; the heavy
    // tempdir handles (`_embedded_esbuild_handle`, `_embedded_nm_handle`)
    // live in this function's scope and are NOT cloned — the cloned
    // `esbuild_binary` / `node_modules_dir` `PathBuf`s reference paths
    // those handles keep alive.
    let bundler_input_for_runtime = bundler_input.clone();
    let bundler_out = runner
        .bundle(bundler_input)
        .context("bundler step failed")?;

    // 2. Phase 3 — runtime paths() evaluation.
    //
    // For any dynamic routes whose `paths()` couldn't be statically
    // extracted (Phase 1), start the embedded V8 host against the freshly-
    // bundled worker and query the `/__paths__/<route>` endpoint for each.
    // `eval_deferred_paths` returns `Backend::EmbeddedV8` (with a factory)
    // in both the empty-deferred and non-empty branches; `render_all` always
    // constructs a fresh host from that factory — there is no host reuse.
    // `_worker_handle` keeps the eval-phase RendererState alive through the
    // subsequent `render_all` call so its resources (e.g. temp files) are
    // not dropped prematurely. The `_` prefix suppresses the unused-variable
    // warning without triggering immediate drop (only `_` alone drops
    // immediately; `_name` lives to end of scope).
    let (runtime_expansion, backend, _worker_handle) = runner
        .eval_deferred_paths(&still_deferred, &bundler_out, &mut paths_cache)
        .context("runtime paths() evaluation step failed")?;
    static_routes.extend(runtime_expansion.resolved);
    warn_deferred_dynamic(&runtime_expansion.deferred);

    if static_routes.is_empty() {
        output::warn("no routes to render after runtime paths() evaluation; dist will be empty");
        return Ok((0, zfb_build::PostBuildRouteManifest::empty()));
    }

    // 2.5. Pre-render: produce CSS / islands bytes so we know which
    //      stable URLs to inject into the rendered HTML head. We MUST
    //      know up front whether each emitter slot will produce
    //      bytes; injecting a `<link>` for a stylesheet that is then
    //      never written would leak the unhashed `/assets/styles.css`
    //      URL into shipped HTML (the prod pipeline only rewrites
    //      stable→hashed for slots it actually emits).
    let (mut prod_asset_inputs, registered_marker_names) = runner
        .emit_prod_assets(
            project_root,
            user_pages_dir,
            package_route_entrypoints,
            outdir,
            config,
        )
        .context("production asset emitters failed")?;
    apply_asset_url_base(&mut prod_asset_inputs, config.base.as_deref());
    let prod_head_assets = derive_prod_head_assets(&prod_asset_inputs);

    // Snapshot the route universe *before* moving it into RendererInput
    // — we need the per-page output paths to drive the post-render
    // rewrite step.
    let route_universe_for_rewrite: Vec<(String, std::path::PathBuf)> = static_routes
        .iter()
        .map(|e| (e.url_path.clone(), e.output_path.clone()))
        .collect();

    // 3. Render.
    let renderer_input = RendererInput {
        bundle_path: bundler_out.bundle_path.clone(),
        sourcemap_path: bundler_out.sourcemap_path.clone(),
        manifest: bundler_out.manifest.clone(),
        dist_dir: outdir.to_path_buf(),
        project_root: project_root.to_path_buf(),
        route_universe: static_routes,
        prerender_map: prerender_map.clone(),
        backend,
        request_timeout: None,
        // S4: inject the stable URLs for any asset slot that produced
        // bytes. `ProductionAssetPipeline` will rewrite each match in
        // the rendered HTML to the hashed URL after the file has
        // landed on disk.
        prod_head_assets,
    };
    let render_out = runner
        .render_all(renderer_input)
        .context("renderer step failed")?;

    // .html-page emission (Option B, zfb#409): files written from
    // `.html` page sources are emitted verbatim per the v1 contract —
    // no asset URL rewriting, no link-base rewriting, no sitemap
    // inclusion. Filter them out of the path list fed to the next two
    // post-processing passes so the contract is preserved when the
    // user has set `base` or enabled the prod asset pipeline.
    //
    // Known limitation (#976): because `.html`-source pages skip the
    // rewrite pass entirely, a client-script stable URL
    // (`/assets/client/<name>.js`) referenced inside one is NOT
    // rewritten to its hashed equivalent — same as CSS/islands URLs.
    let static_html_set: std::collections::HashSet<&std::path::Path> = render_out
        .static_html_files_written
        .iter()
        .map(std::path::PathBuf::as_path)
        .collect();
    let post_processable_pages: Vec<std::path::PathBuf> = render_out
        .ssg_files_written
        .iter()
        .filter(|p| !static_html_set.contains(p.as_path()))
        .cloned()
        .collect();

    // 3.5. Production asset pipeline pass.
    //
    // The renderer just wrote SSG HTML files to disk with stable
    // asset URLs spliced into `<head>` (when an emitter slot produced
    // bytes). Now hash + ship the asset bytes and rewrite each HTML
    // file's stable URLs to the hashed equivalents in place. This is
    // a no-op (no asset writes, HTML round-tripped) when both
    // emitter slots returned `None`.
    if prod_asset_inputs.css.is_some()
        || prod_asset_inputs.islands.is_some()
        || !prod_asset_inputs.client_scripts.is_empty()
    {
        let prod_pages =
            build_prod_rendered_files(outdir, &route_universe_for_rewrite, &post_processable_pages);
        apply_prod_asset_pipeline(outdir, prod_pages, prod_asset_inputs)
            .context("production asset pipeline (hash + URL rewrite) failed")?;
    }

    // 3.6. User-link base rewrite (issue #228).
    //
    // The asset rewrite above only touched `/assets/...` URLs in
    // `<link>` / `<script>`. User-authored absolute links —
    // `<a href="/about">`, `<form action="/login">` — were left bare,
    // so a sub-path deploy (`base = "/pj/foo/"`) shipped working
    // styling but broken navigation. Walk every emitted HTML file and
    // prefix root-absolute hrefs/actions; module-level docs cover the
    // skip rules and the `data-no-base` opt-out. No-op when `base` is
    // unset, `"/"`, or absolute-URL-shaped.
    crate::commands::link_base_rewrite::apply_link_base_rewrite(
        outdir,
        &post_processable_pages,
        config.base.as_deref(),
        config.trailing_slash,
    )
    .context("link base rewrite failed")?;

    // 3.7. Optional production HTML minification.
    //
    // This intentionally runs after the production asset URL rewrite and
    // link-base rewrite, but before island-marker validation and postBuild.
    // The target set is still renderer-reported SSG files minus `.html`
    // passthrough pages; filter it further to HTML-ish extensions so XML feeds
    // and other non-HTML SSG outputs are never passed to the HTML minifier.
    if minify_html {
        minify_rendered_html_files(&post_processable_pages).context("HTML minification failed")?;
    }

    // 3.8. Island-marker check (issue #984 / #990).
    //
    // Walk the post-processed HTML pages and warn for every
    // `data-zfb-island="X"` / `data-zfb-island-skip-ssr="X"` marker
    // whose name is absent from the scanner's registry set.  Non-fatal:
    // the build succeeds; the warning is the signal.  Runs even when
    // `registered_marker_names` is empty (zero registered islands + a
    // rendered marker is exactly the scenario this check targets).
    crate::commands::island_marker_check::check_island_markers(
        &post_processable_pages,
        &registered_marker_names,
    );

    // Surface embedded V8 host runtime logs (console output from the
    // worker) on a green build — they are often informative about
    // deprecations or routing oddities.
    if !render_out.runtime_logs.trim().is_empty() {
        output::info("runtime logs:");
        for line in render_out.runtime_logs.lines() {
            output::info(format!("  {line}"));
        }
    }

    // 3. Adapter dispatch.
    //
    // When an adapter is configured, run a SECOND bundle pass narrowed to
    // SSR-only routes (zfb#283) and hand that smaller bundle to the
    // adapter. Rationale: deploy targets like Cloudflare Workers Static
    // Assets serve prerendered routes through a static-asset server
    // (ASSETS first, inner worker on 404). SSG route code in the inner
    // worker bundle is dead code on the request path AND counts against
    // the platform's worker-size cap (Cloudflare Workers: 3 MiB free /
    // 10 MiB paid). Trimming the
    // bundle to SSR-only routes via `BundlerInput::worker_only_routes`
    // makes prerendered routes unreachable from the synthetic entry; the
    // resulting esbuild output is much smaller after tree-shake.
    //
    // The full SSG bundle from the first pass (`bundler_out.bundle_path`)
    // is still needed by `render_all` above for SSG render. After this
    // function returns, both bundles live under `.zfb-build/` but only
    // the runtime-narrowed one reaches the adapter (and therefore `dist/`).
    if !adapter.is_none() {
        let mut runtime_bundler_input = bundler_input_for_runtime;
        runtime_bundler_input.worker_only_routes = Some(ssr_route_keys_for_runtime_bundle);
        runtime_bundler_input.bundle_basename = Some("bundle-runtime.mjs".to_string());
        let runtime_bundler_out = runner
            .bundle(runtime_bundler_input)
            .context("runtime-only bundler step (for deploy adapter) failed")?;

        let adapter_in = AdapterBundleInput {
            project_root: project_root.to_path_buf(),
            input_bundle: runtime_bundler_out.bundle_path.clone(),
            outdir: outdir.to_path_buf(),
        };
        let adapter_out: AdapterBundleOutput =
            run_adapter_bundle_with(&adapter, adapter_in, adapter_runner)
                .context("adapter dispatch step failed")?;
        if !adapter_out.stdout.trim().is_empty() {
            output::info(format!(
                "adapter `{}`:",
                adapter.package_name().unwrap_or("(unknown)"),
            ));
            for line in adapter_out.stdout.lines() {
                output::info(format!("  {line}"));
            }
        }
        if !adapter_out.stderr.trim().is_empty() {
            for line in adapter_out.stderr.lines() {
                output::warn(format!("adapter stderr: {line}"));
            }
        }
    }

    // 4. Copy public/ into out_dir.
    //
    // Static assets in public/ must land in dist/ so they are served
    // verbatim in production. When the project mounts under a sub-path
    // (cfg.base = "/pj/test/") and `copy_public_with_base` is true
    // (the default), files arrive at <out_dir>/<base-segment>/... so
    // URLs emitted via withBase() resolve under the sub-path mount.
    // When `copy_public_with_base` is false, files land flat at
    // <out_dir>/<rel> regardless of base — use this when the deploy
    // pipeline relocates the entire dist/ tree into the base segment
    // itself (e.g. `cp -a dist/. deploy-root/pj/site/`).
    // Missing public/ is silently ignored — not every project has one.
    let effective_base = if config.copy_public_with_base {
        config.base.as_deref()
    } else {
        None
    };
    copy_public_dir(project_root, outdir, &config.public_dir, effective_base)
        .context("public dir copy step failed")?;

    // Build the postBuild route manifest (#262). Combines:
    // - static routes from the router scan (no params),
    // - statically-expanded dynamic routes (params from `expansion`),
    // - runtime-expanded dynamic routes (params from `runtime_expansion`).
    //
    // The manifest now includes BOTH prerendered (SSG) routes — which
    // appear in `render_out.ssg_files_written` — AND SSR routes (those
    // with `export const prerender = false`), which have no on-disk
    // artifact but ARE valid runtime URLs. Each entry carries a
    // `prerender` boolean derived from the build-time `prerender_map` so
    // plugins that should only enumerate on-disk URLs (sitemap.xml,
    // search-index.json) can filter `r.prerender !== false`.
    let manifest = build_post_build_manifest(
        routes,
        project_root,
        build_pages_root,
        &expansion.resolved_with_params,
        &runtime_expansion.resolved_with_params,
        &prerender_map,
    );

    Ok((render_out.ssg_files_written.len(), manifest))
}

/// Build the [`PostBuildRouteManifest`] from the routes scanned before the
/// build plus the params collected during dynamic expansion. Only routes whose
/// output path is a plain HTML or non-HTML page are included (adapter-specific
/// artefacts like `_worker.js` are not). The manifest is sorted by `url` for
/// byte-stable output across runs (#262 AC: "Manifest byte-stable across runs").
///
/// `prerender_map` is the same build-time map produced by
/// [`crate::render_pipeline::build_prerender_map`] and keyed by
/// `route_key` (the route template). Each emitted [`PostBuildRouteEntry`]
/// carries the resolved `prerender` boolean so consumer plugins can
/// distinguish SSG (on-disk) and SSR routes.
fn build_post_build_manifest(
    routes: &[zfb_router::Route],
    project_root: &Path,
    // #1193 — the build pages root (the overlay when package routes are
    // present). Used to report a package route's manifest `source` as a
    // clean `pages/<rel>` instead of leaking the absolute overlay temp path.
    build_pages_root: &Path,
    static_expansion_params: &[DynamicResolvedEntry],
    runtime_expansion_params: &[DynamicResolvedEntry],
    prerender_map: &std::collections::BTreeMap<String, bool>,
) -> zfb_build::PostBuildRouteManifest {
    use std::collections::BTreeMap;
    use zfb_build::{PostBuildParamValue, PostBuildRouteEntry, PostBuildRouteManifest};
    use zfb_router::RouteKind;

    // Render a route's `source_path` as a stable, relative manifest string.
    // Prefer project-relative; for a package route (source under the overlay
    // pages root, outside project_root) report `pages/<rel>` rather than the
    // ephemeral absolute temp path (#1193); otherwise fall back to the raw
    // path (matches the pre-#1193 behaviour for any unexpected shape).
    let manifest_source = |source_path: &Path| -> String {
        if let Ok(rel) = source_path.strip_prefix(project_root) {
            return rel.to_string_lossy().into_owned();
        }
        if let Ok(rel) = source_path.strip_prefix(build_pages_root) {
            return Path::new("pages").join(rel).to_string_lossy().into_owned();
        }
        source_path.to_string_lossy().into_owned()
    };

    let mut entries: Vec<PostBuildRouteEntry> = Vec::new();

    // 1. Static routes — no params.
    for route in routes {
        if route.kind != RouteKind::Static {
            continue;
        }
        let template = route.template();
        let output_path = route.output_filename(None);
        let ext = route
            .output_extension
            .as_deref()
            .unwrap_or("html")
            .to_string();
        let source = manifest_source(&route.source_path);
        // Default to SSG (`prerender = true`) when the map has no entry —
        // matches the rest of the build's interpretation of a missing
        // `export const prerender` (e.g. lines 1001 / 1019 above).
        let prerender = prerender_map.get(&template).copied().unwrap_or(true);
        entries.push(PostBuildRouteEntry {
            url: template,
            output: output_path.to_string_lossy().into_owned(),
            extension: ext,
            source,
            prerender,
            params: None,
        });
    }

    // 2. Statically-expanded + 3. runtime-expanded dynamic routes.
    for dyn_entry in static_expansion_params
        .iter()
        .chain(runtime_expansion_params.iter())
    {
        let source = manifest_source(&dyn_entry.source_path);

        // Build the params map only when there are bindings.
        let params = if dyn_entry.params.scalars.is_empty() && dyn_entry.params.arrays.is_empty() {
            None
        } else {
            let mut map: BTreeMap<String, PostBuildParamValue> = BTreeMap::new();
            for (k, v) in &dyn_entry.params.scalars {
                map.insert(k.clone(), PostBuildParamValue::Scalar(v.clone()));
            }
            for (k, v) in &dyn_entry.params.arrays {
                map.insert(k.clone(), PostBuildParamValue::Array(v.clone()));
            }
            Some(map)
        };

        let prerender = prerender_map
            .get(&dyn_entry.route_key)
            .copied()
            .unwrap_or(true);

        entries.push(PostBuildRouteEntry {
            url: dyn_entry.url_path.clone(),
            output: dyn_entry.output_path.to_string_lossy().into_owned(),
            extension: dyn_entry.extension.clone(),
            source,
            prerender,
            params,
        });
    }

    // Sort by URL for byte-stable output (#262 AC).
    entries.sort_by(|a, b| a.url.cmp(&b.url));

    PostBuildRouteManifest { routes: entries }
}

/// Where the on-disk route manifest lives, relative to `outDir`. The
/// `__zfb/` prefix matches the common "framework-private" directory
/// convention (Next's `_next/`, Astro's `_astro/`) and pairs with the
/// JS-side `globalThis.__zfb` runtime namespace, so the source-of-truth
/// for "this is internal zfb metadata" reads the same on disk and in
/// memory.
const ROUTES_MANIFEST_REL_PATH: &str = "__zfb/routes.json";

/// Emit the on-disk `routes.json` manifest under `<outdir>/__zfb/` (#347).
///
/// The serialised shape is **identical** to the in-memory `ctx.routes`
/// the plugin API hands to `postBuild` hooks — same fields (`url`,
/// `output`, `extension`, `source`, `prerender`, optional `params`),
/// same url-sorted order. Two access shapes, one source of truth.
///
/// Output is JSON, pretty-printed with 2-space indents and a trailing
/// newline so diffs read sensibly. A serialiser bug that swapped sort
/// order between runs would break the byte-stability contract this
/// shares with #262's in-memory manifest, so the entries are written
/// verbatim — the caller is responsible for the sort.
fn emit_routes_manifest_file(
    outdir: &Path,
    manifest: &zfb_build::PostBuildRouteManifest,
) -> Result<()> {
    let dest = outdir.join(ROUTES_MANIFEST_REL_PATH);
    let mut json = serde_json::to_string_pretty(manifest)
        .context("serialise postBuild route manifest to JSON")?;
    json.push('\n');
    zfb_build::atomic_write_string(&dest, &json)
        .with_context(|| format!("failed to write {}", dest.display()))?;
    Ok(())
}

/// Mount each emitter's `stable_url` under the project's configured
/// `base` prefix.
///
/// Both halves of the prod asset rewrite — the URL spliced into the
/// rendered `<head>` AND the stable→hashed mapping
/// `ProductionAssetPipeline` applies — read off
/// [`AssetEmitterPayload::stable_url`]. Mutating the payload in one
/// place therefore keeps the renderer-emitted reference and the
/// rewrite key in sync; if we prefixed only the head injection, the
/// `boundary_replace` rewrite would never match and the unhashed
/// `/assets/...` URL would leak into shipped HTML.
///
/// `None` / empty / `"/"` bases are no-ops (see
/// [`crate::config::asset_url_base_prefix`] for the canonical
/// normalisation rules), so projects that do not deploy under a
/// sub-path see byte-identical output to the pre-`base` build.
fn apply_asset_url_base(inputs: &mut ProdAssetEmitterInputs, base: Option<&str>) {
    let prefix = crate::config::asset_url_base_prefix(base);
    if prefix.is_empty() {
        return;
    }
    if let Some(css) = inputs.css.as_mut() {
        css.stable_url = format!("{prefix}{}", css.stable_url);
    }
    if let Some(islands) = inputs.islands.as_mut() {
        islands.stable_url = format!("{prefix}{}", islands.stable_url);
    }
    for cs in inputs.client_scripts.iter_mut() {
        cs.stable_url = format!("{prefix}{}", cs.stable_url);
    }
}

/// Derive the [`ProdHeadAssets`] payload for [`RendererInput`] from
/// the bytes-only emitter inputs. Returns `None` when no slot has
/// bytes — the renderer then ships HTML untouched (matching today's
/// behaviour for projects with no CSS / islands).
///
/// The stable URLs come straight from
/// [`zfb_build::pipeline::AssetEmitterPayload::stable_url`] (which the
/// CSS / islands adapters seed from `zfb_types::asset_urls`
/// constants, optionally re-prefixed by [`apply_asset_url_base`] when
/// the project mounts under a sub-path). This lets a caller mount
/// assets at a non-default URL prefix without rewriting this
/// function.
fn derive_prod_head_assets(inputs: &ProdAssetEmitterInputs) -> Option<ProdHeadAssets> {
    let css_url = inputs.css.as_ref().map(|p| p.stable_url.clone());
    let mut island_module_urls: Vec<String> = Vec::new();
    if let Some(islands) = inputs.islands.as_ref() {
        island_module_urls.push(islands.stable_url.clone());
    }
    // Client scripts are deliberately NOT auto-injected here. Unlike the CSS
    // and islands bundles — which every page needs — a client script ships to
    // a page ONLY when that page explicitly references it via the
    // `clientScript()` SSR helper (`<script src={clientScript("name")} />`).
    // Auto-injecting all of them into every page's head would defeat the
    // selective per-page loading contract and duplicate tags on pages that
    // already render the reference deliberately. The client-script payloads
    // still flow through `ProductionAssetPipeline` (hashing + the
    // stable→hashed HTML rewrite over each page's explicit reference) via
    // `prod_asset_inputs.client_scripts`; only this head auto-injection is
    // removed (#971 P2).
    if css_url.is_none() && island_module_urls.is_empty() {
        return None;
    }
    Some(ProdHeadAssets {
        css_url,
        island_module_urls,
    })
}

/// Pair each route-universe entry's relative `output_path` with the
/// renderer's report of which absolute paths were actually written —
/// returning the SSG-written subset as
/// [`ProdRenderedFile`]s ready for the prod orchestrator.
///
/// SSR-only entries are skipped (their absolute path never appears in
/// `ssg_files_written`). Each surviving relative path is validated
/// through [`RelDistPath::new`] so a malformed path can never reach
/// the orchestrator's atomic-write step.
fn build_prod_rendered_files(
    dist_dir: &Path,
    route_universe: &[(String, std::path::PathBuf)],
    ssg_files_written: &[std::path::PathBuf],
) -> Vec<ProdRenderedFile> {
    use std::collections::HashSet;
    // The renderer writes to `dist_dir.join(entry.output_path)`. Build
    // a set of the absolute paths it reported so we can filter SSG
    // entries from SSR-only entries cheaply.
    let written: HashSet<std::path::PathBuf> = ssg_files_written.iter().cloned().collect();

    let mut out: Vec<ProdRenderedFile> = Vec::with_capacity(written.len());
    let mut seen_paths: HashSet<std::path::PathBuf> = HashSet::new();
    for (_url, rel) in route_universe {
        let abs = dist_dir.join(rel);
        if !written.contains(&abs) {
            continue;
        }
        if !seen_paths.insert(rel.clone()) {
            // De-duplicate: two route entries sharing an output path
            // would otherwise produce duplicate synthetic page ids
            // and break the orchestrator's BTreeSet invariant.
            continue;
        }
        match RelDistPath::new(rel.clone()) {
            Ok(rel_path) => {
                let page = synthesize_page_id_from_output(&rel_path);
                out.push(ProdRenderedFile {
                    page,
                    output_path: rel_path,
                });
            }
            Err(err) => {
                output::warn(format!(
                    "production asset pipeline: skipping invalid output path {} ({err})",
                    rel.display(),
                ));
            }
        }
    }
    out
}

fn minify_rendered_html_files(paths: &[std::path::PathBuf]) -> Result<()> {
    for path in paths {
        if !is_html_output_path(path) {
            continue;
        }
        let bytes = std::fs::read(path)
            .with_context(|| format!("failed to read HTML for minification: {}", path.display()))?;
        let minified = crate::commands::html_minify::minify_rendered_html_bytes(&bytes);
        zfb_build::atomic_write(path, &minified).with_context(|| {
            format!(
                "failed to write minified HTML atomically: {}",
                path.display()
            )
        })?;
    }
    Ok(())
}

fn is_html_output_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("html") || ext.eq_ignore_ascii_case("htm"))
}

fn warn_deferred_dynamic(routes: &[DeferredDynamicRoute]) {
    for r in routes {
        output::warn(format!(
            "skipping {} ({}) — {}",
            r.template,
            r.source_path.display(),
            r.reason,
        ));
    }
}

/// Locate the project's `node_modules` directory so the bundler can
/// symlink it into the shadow tree (esbuild then walks into it for
/// package resolution). Returns `None` when no `node_modules` exists
/// — the build proceeds and esbuild will surface a clear "Could not
/// resolve" error if the user's pages import any third-party packages.
///
/// Today only the `<project_root>/node_modules` slot is checked. pnpm
/// monorepos with hoisted root-level `node_modules/` are typically
/// flat enough that this single-level lookup suffices; users with
/// deeper layouts can pre-stage a `node_modules` symlink at the root.
pub(crate) fn detect_project_node_modules(project_root: &Path) -> Option<std::path::PathBuf> {
    let candidate = project_root.join("node_modules");
    if candidate.is_dir() {
        Some(candidate)
    } else {
        None
    }
}

/// Discover the Next-style root `mdx-components.tsx` convention (#616): a
/// project-wide element→component override map applied to every `<Content>`
/// without per-call spreading. The file lives next to `zfb.config.ts` at the
/// project root; its default export is the canonical `{ h2: MyH2, … }` map.
///
/// Returns the absolute path when the file exists, else `None` — the "no
/// file ⇒ unchanged output" acceptance criterion depends on this gate.
/// Shared by both `zfb build` and `zfb dev`; the shadow is a fresh tempdir
/// per `bundle()` so discovery re-runs every build and dev/preview picks up
/// edits with no special-casing.
pub(crate) fn discover_mdx_components_file(project_root: &Path) -> Option<std::path::PathBuf> {
    let candidate = project_root.join("mdx-components.tsx");
    candidate.is_file().then_some(candidate)
}

/// Read `<project_root>/tsconfig.json` and return its
/// `compilerOptions.paths` map, suitable for forwarding into
/// [`BundlerInput::tsconfig_paths`]. Used so user-facing alias maps
/// like `"@/*": ["src/*"]` resolve at bundle time without each project
/// having to repeat them in `zfb.config.ts`.
///
/// Delegates to [`zfb_plugin_resolver::read_tsconfig_paths_into_map`],
/// which handles JSONC comment stripping, `extends` chain walking (any
/// tsconfig filename, up to depth 8), `baseUrl` resolution, and target
/// absolutisation in one shared place.
pub(crate) fn read_tsconfig_paths(
    project_root: &Path,
) -> std::collections::BTreeMap<String, Vec<String>> {
    zfb_plugin_resolver::read_tsconfig_paths_into_map(project_root)
}

/// Build the `(absolute path → URL)` source map the bundler hands to
/// `ResolveLinksPlugin`, so `build_snapshot_with_config` can drive the
/// snapshot-side pipeline through the same plugin shape. Returns `None`
/// when the project doesn't enable `resolveMarkdownLinks`. See zfb#188.
///
/// The shape of `spec` decides which collections to scan:
///
/// - `spec.dirs` non-empty → use those entries verbatim.
/// - `spec.dirs` empty → fall back to the legacy single-dir form, which
///   pairs `spec.docs_dir` with the hard-coded `/docs/` route prefix.
///
/// The bundler-side wiring in `crates/zfb-build/src/bundler.rs` MUST
/// produce the same URL strings or the snapshot's `content_hash` drifts
/// from the bundler's bridge-map key, which silently misses the
/// `globalThis.__zfb.content.get(specifier)` lookup at SSR time
/// (zfb#187 / #188). The shared helper
/// [`resolve_links_routes_from_config`] guarantees the two sites stay
/// in sync.
/// Build the JSON-serialized content snapshot for the configured
/// collections, mirroring the bundler's content pipeline exactly so the
/// snapshot's `content_hash` stays byte-identical to the bridge-map keys
/// the bundler emits (zfb#187 / #188).
///
/// Returns `None` when the project declares no collections, or when the
/// snapshot build / serialization fails (both warned, non-fatal — the
/// worker still boots with an empty snapshot, surfacing as empty
/// `getCollection(...)` results).
///
/// Shared by `zfb build` (where it feeds the embedded worker bundle so
/// runtime `paths()` can call `getCollection(...)`) and `zfb dev` (where
/// it feeds the long-lived dev renderer so a page's `getStaticProps()`
/// sees the same collection data the build does — without this, dev
/// `getCollection(...)` resolves against the placeholder empty snapshot
/// and every collection query returns `[]`).
pub(crate) fn build_content_snapshot_json(project_root: &Path, config: &Config) -> Option<String> {
    if config.collections.is_empty() {
        return None;
    }
    let collections: Vec<zfb_content::CollectionConfig> = config
        .collections
        .iter()
        .map(|c| zfb_content::CollectionConfig {
            name: c.name.clone(),
            root: project_root.join(&c.path),
            include: c.include.clone(),
            exclude: c.exclude.clone(),
            id_strip_suffix: c.id_strip_suffix.clone(),
        })
        .collect();
    // The pipeline shape comes from the SAME Config→spec assembly the
    // bundler input uses (`pipeline_spec_from_config`, zfb#917), so the
    // snapshot's JSX content_hash structurally cannot diverge from the
    // bundler's bridge-map keys — divergence would make
    // `bridge.get(specifier)` miss on every collection page, dumping the
    // rendered output into a `<pre data-zfb-content-fallback>` block
    // (zfb#188). Only `resolve_source_map` is filled per-surface: the
    // snapshot builds it eagerly here, the bundler derives it inside
    // `bundle()` from its route spec — same route helper, identical maps.
    let snapshot_config = {
        let mut spec =
            crate::commands::bundler_input::pipeline_spec_from_config(project_root, config);
        spec.resolve_source_map = build_resolve_source_map_for_snapshot(project_root, config);
        spec
    };
    match zfb_content::build_snapshot_with_config(&collections, &snapshot_config) {
        Ok(snap) => match serde_json::to_string(&snap) {
            Ok(json) => Some(json),
            Err(e) => {
                output::warn(format!(
                    "content snapshot serialization failed ({e}); getCollection(...) will see empty collections"
                ));
                None
            }
        },
        Err(e) => {
            output::warn(format!(
                "content snapshot build failed ({e}); getCollection(...) will see empty collections"
            ));
            None
        }
    }
}

fn build_resolve_source_map_for_snapshot(
    project_root: &Path,
    config: &Config,
) -> Option<std::collections::HashMap<std::path::PathBuf, String>> {
    use zfb_content::plugins::util::source_map::{build_docs_source_map, DocsSourceMapOptions};
    let routes = resolve_links_routes_from_config(project_root, config)?;
    let map = build_docs_source_map(DocsSourceMapOptions {
        collections: routes,
    });
    Some(map)
}

/// Build the `Vec<CollectionRoute>` the source-map helper expects from
/// a project-root-relative `Config`.
///
/// Returns `None` when `resolveMarkdownLinks` is absent or disabled, so
/// callers can short-circuit. When `spec.dirs` is non-empty, every
/// entry becomes one `CollectionRoute`; otherwise the legacy single-dir
/// fallback emits one route at the hard-coded `/docs/` prefix.
///
/// Both the snapshot-side helper above and the bundler-side wiring in
/// `commands/build.rs::build` consume this so the `path → URL` map is
/// identical at both sites — required for content-hash determinism.
pub(crate) fn resolve_links_routes_from_config(
    project_root: &Path,
    config: &Config,
) -> Option<Vec<zfb_content::plugins::util::source_map::CollectionRoute>> {
    use zfb_content::plugins::util::source_map::CollectionRoute;
    let spec = config.resolve_markdown_links.as_ref()?;
    if !spec.enabled {
        return None;
    }
    let routes = if !spec.dirs.is_empty() {
        spec.dirs
            .iter()
            .enumerate()
            .map(|(i, d)| CollectionRoute {
                name: format!("dirs[{i}]"),
                dir: project_root.join(&d.dir),
                route_prefix: d.route_prefix.clone(),
            })
            .collect()
    } else {
        vec![CollectionRoute {
            name: "docs".to_string(),
            dir: project_root.join(&spec.docs_dir),
            route_prefix: "/docs/".to_string(),
        }]
    };
    Some(routes)
}

/// When `ZFB_DEBUG_SNAPSHOT` is truthy, build the content snapshot from
/// the configured collections so [`zfb_content::build_snapshot`] logs
/// the entry count and serialized byte size to stderr. The snapshot
/// itself is discarded — wave 2 owns the bundler-side wiring; this
/// function exists so users can monitor V8 RAM pressure today.
///
/// Failure is non-fatal: a malformed collection root will print a
/// warning but the build proceeds. The flag is opt-in, so the cost of
/// walking the collections a second time only lands when the user has
/// asked for the telemetry.
fn maybe_probe_content_snapshot(project_root: &Path, config: &Config) {
    if !zfb_content::debug_snapshot_enabled() {
        return;
    }
    let collections: Vec<zfb_content::CollectionConfig> = config
        .collections
        .iter()
        .map(|c| zfb_content::CollectionConfig {
            name: c.name.clone(),
            root: project_root.join(&c.path),
            include: c.include.clone(),
            exclude: c.exclude.clone(),
            id_strip_suffix: c.id_strip_suffix.clone(),
        })
        .collect();
    if let Err(err) = zfb_content::build_snapshot(&collections) {
        output::warn(format!("ZFB_DEBUG_SNAPSHOT: snapshot probe failed: {err}"));
    }
}

/// Copy `<project_root>/<public_dir>` recursively into
/// `<outdir>/<base-segment>/`.
///
/// - `public_dir` is the configured public directory (default `public/`).
/// - `base` is the optional sub-path mount from `cfg.base` (e.g.
///   `"/pj/test/"`). Leading and trailing slashes are stripped to produce
///   the on-disk segment (`"pj/test"`). `None`, `""`, or `"/"` mean no
///   sub-path — files copy directly under `outdir`.
/// - A missing or empty `public_dir` is treated as a no-op (no error).
///
/// Files inside `public/` are placed at `<outdir>/<base-segment>/<rel>`,
/// matching the URL space that `withBase()` produces in the rendered HTML.
fn copy_public_dir(
    project_root: &Path,
    outdir: &Path,
    public_dir: &std::path::Path,
    base: Option<&str>,
) -> Result<()> {
    let src = if public_dir.is_absolute() {
        public_dir.to_path_buf()
    } else {
        project_root.join(public_dir)
    };

    if !src.is_dir() {
        // Missing public/ is a no-op — not every project has one.
        return Ok(());
    }

    // Strip leading/trailing slashes from the base to get the segment,
    // e.g. "/pj/test/" → "pj/test", "/" → "", None → "".
    // Absolute-URL bases ("https://cdn.example.com/") mean assets live on
    // another origin — there is no on-disk sub-path, so public files copy
    // directly under `outdir` (same interpretation as
    // `zfb_types::base_prefix::dev_mount_prefix`).
    let base_segment = base
        .filter(|b| !b.contains("://"))
        .map(|b| b.trim_matches('/'))
        .unwrap_or("")
        .to_string();

    let dest_root = if base_segment.is_empty() {
        outdir.to_path_buf()
    } else {
        outdir.join(&base_segment)
    };

    for entry in walkdir::WalkDir::new(&src)
        .into_iter()
        .filter_map(|r| match r {
            Ok(e) => Some(e),
            Err(err) => {
                output::warn(format!("public dir copy: skipping unreadable entry: {err}"));
                None
            }
        })
    {
        let rel = entry
            .path()
            .strip_prefix(&src)
            .expect("walkdir entry is always under src");
        if rel.as_os_str().is_empty() {
            // Skip the root entry itself.
            continue;
        }
        let dest = dest_root.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&dest)
                .with_context(|| format!("public dir copy: create dir {}", dest.display()))?;
        } else if entry.file_type().is_file() {
            // Route-vs-static collision: if the renderer already created
            // `dest` as a directory (e.g. `dist/foo/` for a `pages/foo.tsx`
            // route that emits `dist/foo/index.html`), copying the public
            // file flat onto that path would fail with EISDIR. The documented
            // precedence — rendered page > public file — means we skip it.
            // In dev the same result is achieved naturally because the
            // PageCache / html_root waterfall runs before the public_root
            // fallback in `serve_page`.
            if dest.is_dir() {
                output::warn(format!(
                    "public dir copy: skipping {} because destination {} is a \
                     rendered-route directory (page route takes precedence over public file)",
                    entry.path().display(),
                    dest.display(),
                ));
                continue;
            }
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("public dir copy: create parent dir {}", parent.display())
                })?;
            }
            std::fs::copy(entry.path(), &dest).with_context(|| {
                format!(
                    "public dir copy: copy {} → {}",
                    entry.path().display(),
                    dest.display()
                )
            })?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::PathBuf;
    use tempfile::tempdir;
    use zfb_build::bundler::{BundleManifest, BundlerOutput, RouteEntry};
    use zfb_build::renderer::{HttpResponseLike, RendererOutput, SsrManifest};
    use zfb_router::{Route, RouteKind, Segment};

    /// Fake [`BuildRunner`] that records the inputs it received and
    /// returns canned outputs. `RefCell` so multiple methods can mutate
    /// shared state through `&self` (tests run single-threaded).
    struct FakeRunner {
        bundle_calls: RefCell<Vec<BundlerInput>>,
        render_calls: RefCell<Vec<RendererInput>>,
        /// Counts how many times `eval_deferred_paths` was invoked.
        /// Guards the once-per-run_build call structure (issue #974).
        eval_deferred_paths_calls: RefCell<usize>,
        mock_bundle_path: PathBuf,
        /// Canned production asset emitter inputs returned from
        /// `emit_prod_assets`. Default = empty (parity with
        /// `DefaultRunner`); tests can preload bytes to exercise the
        /// hash + URL rewrite path.
        prod_asset_inputs: RefCell<ProdAssetEmitterInputs>,
        /// Stable client-script URLs that each rendered page explicitly
        /// references via the `clientScript()` SSR helper. The real renderer
        /// emits these because the page source calls `clientScript("name")`;
        /// the fake splices a `<script src="…">` per URL into every page's
        /// body so the post-render pipeline has an explicit reference to
        /// hash-rewrite. This is independent of `prod_head_assets` — client
        /// scripts are NOT auto-injected into the head (#971 P2).
        page_client_script_refs: RefCell<Vec<String>>,
        /// Relative output paths the fake should report as `.html` passthrough
        /// files. Real renderer output uses absolute paths in
        /// `static_html_files_written`, so `render_all` joins these against the
        /// input `dist_dir`.
        static_html_output_paths: RefCell<Vec<PathBuf>>,
    }

    impl FakeRunner {
        fn new(mock_bundle_path: PathBuf) -> Self {
            Self {
                bundle_calls: RefCell::new(Vec::new()),
                render_calls: RefCell::new(Vec::new()),
                eval_deferred_paths_calls: RefCell::new(0),
                mock_bundle_path,
                prod_asset_inputs: RefCell::new(ProdAssetEmitterInputs::default()),
                page_client_script_refs: RefCell::new(Vec::new()),
                static_html_output_paths: RefCell::new(Vec::new()),
            }
        }

        /// Preload canned bytes for the production asset emitters.
        /// Used by the orchestrator-wiring tests below.
        fn with_prod_asset_inputs(self, inputs: ProdAssetEmitterInputs) -> Self {
            *self.prod_asset_inputs.borrow_mut() = inputs;
            self
        }

        /// Declare client-script stable URLs that each rendered page
        /// references explicitly (simulating a page that calls
        /// `clientScript()`). The fake renderer splices a `<script src="…">`
        /// tag per URL into every page body. Used by the client-script tests
        /// to exercise the explicit-reference hash-rewrite path now that head
        /// auto-injection is gone (#971 P2).
        fn with_page_client_script_refs(self, urls: Vec<String>) -> Self {
            *self.page_client_script_refs.borrow_mut() = urls;
            self
        }

        fn with_static_html_output_paths(self, paths: Vec<PathBuf>) -> Self {
            *self.static_html_output_paths.borrow_mut() = paths;
            self
        }
    }

    impl BuildRunner for FakeRunner {
        fn bundle(&self, input: BundlerInput) -> Result<BundlerOutput> {
            self.bundle_calls.borrow_mut().push(input.clone());
            std::fs::create_dir_all(self.mock_bundle_path.parent().unwrap()).ok();
            std::fs::write(&self.mock_bundle_path, "// mock\n").ok();
            Ok(BundlerOutput {
                bundle_path: self.mock_bundle_path.clone(),
                sourcemap_path: self.mock_bundle_path.with_extension("mjs.map"),
                manifest: BundleManifest {
                    framework: "preact".into(),
                    jsx_import_source: "preact".into(),
                    hydrate_shim_specifier: "zfb:internal/preact/hydrate".into(),
                    bundle_basename: "bundle.mjs".into(),
                    routes: vec![RouteEntry {
                        route: "/".into(),
                        source_path: PathBuf::from("pages/index.tsx"),
                        entry_key: "/".into(),
                        static_html: false,
                        rel_under_pages: PathBuf::from("index.tsx"),
                    }],
                },
                route_module_deps: Vec::new(),
            })
        }
        fn eval_deferred_paths(
            &self,
            deferred: &[DeferredDynamicRoute],
            _bundle_out: &BundlerOutput,
            _cache: &mut PathsCache,
        ) -> Result<(
            crate::render_pipeline::DynamicExpansion,
            Backend,
            WorkerHandle,
        )> {
            *self.eval_deferred_paths_calls.borrow_mut() += 1;
            // The fake runner doesn't start a real host; return all deferred
            // routes unchanged (still deferred), and a no-op Stub backend
            // (the fake render_all ignores the backend).
            Ok((
                crate::render_pipeline::DynamicExpansion {
                    resolved: Vec::new(),
                    deferred: deferred.to_vec(),
                    ..Default::default()
                },
                Backend::Stub {
                    handler: std::sync::Arc::new(|_url| HttpResponseLike {
                        status: 200,
                        content_type: "text/html".into(),
                        body: b"<html/>".to_vec(),
                        ..Default::default()
                    }),
                },
                WorkerHandle(None),
            ))
        }
        fn render_all(&self, input: RendererInput) -> Result<RendererOutput> {
            // Honour the input contract: write each ssg route's output
            // path so callers that inspect `dist/` see real files. If
            // `prod_head_assets` is set, splice the stable URL tags
            // into a `<head>` so the post-render rewrite pass has
            // something to match. The shape mirrors what real
            // `render_all` produces with a non-`None` `prod_head_assets`.
            let head_extra = match input.prod_head_assets.as_ref() {
                Some(assets) => {
                    let mut s = String::new();
                    if let Some(href) = assets.css_url.as_deref() {
                        s.push_str(&format!("<link rel=\"stylesheet\" href=\"{href}\">"));
                    }
                    for src in &assets.island_module_urls {
                        s.push_str(&format!("<script type=\"module\" src=\"{src}\"></script>"));
                    }
                    s
                }
                None => String::new(),
            };
            // Simulate a page that explicitly references client scripts via
            // `clientScript()`: splice each declared URL as a `<script src>`
            // in the body. The real renderer emits these from the page source,
            // not from head auto-injection (#971 P2).
            let body_extra: String = self
                .page_client_script_refs
                .borrow()
                .iter()
                .map(|src| format!("<script type=\"module\" src=\"{src}\"></script>"))
                .collect();
            for entry in &input.route_universe {
                let dest = input.dist_dir.join(&entry.output_path);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                std::fs::write(
                    &dest,
                    format!(
                        "<html>\n  <head>{head_extra}</head>\n  <body>\n    <main> rendered {} </main>\n    <a href=\"/about\">About</a>{body_extra}\n  </body>\n</html>\n",
                        entry.url_path,
                    ),
                )
                .ok();
            }
            let written = input
                .route_universe
                .iter()
                .map(|e| input.dist_dir.join(&e.output_path))
                .collect::<Vec<_>>();
            let static_html_files_written = self
                .static_html_output_paths
                .borrow()
                .iter()
                .map(|rel| input.dist_dir.join(rel))
                .collect::<Vec<_>>();
            self.render_calls.borrow_mut().push(input);
            Ok(RendererOutput {
                ssg_files_written: written,
                static_html_files_written,
                ssr_manifest: SsrManifest::default(),
                runtime_logs: String::new(),
            })
        }

        fn emit_prod_assets(
            &self,
            _project_root: &Path,
            _user_pages_dir: &Path,
            _package_route_entrypoints: &[PathBuf],
            _outdir: &Path,
            _config: &Config,
        ) -> Result<(ProdAssetEmitterInputs, std::collections::BTreeSet<String>)> {
            // Clone the canned inputs so multiple tests can share the
            // same FakeRunner without consuming its state.
            let inputs = self.prod_asset_inputs.borrow();
            Ok((
                ProdAssetEmitterInputs {
                    css: inputs.css.clone(),
                    islands: inputs.islands.clone(),
                    client_scripts: inputs.client_scripts.clone(),
                },
                std::collections::BTreeSet::new(),
            ))
        }
    }

    fn static_route(segments: Vec<&str>, source: &str) -> Route {
        Route {
            source_path: PathBuf::from(source),
            segments: segments
                .into_iter()
                .map(|s| Segment::Static(s.to_string()))
                .collect(),
            kind: RouteKind::Static,
            specificity: 0,
            output_extension: None,
            static_html: false,
        }
    }

    fn dynamic_route(name: &str, source: &str) -> Route {
        Route {
            source_path: PathBuf::from(source),
            segments: vec![Segment::Dynamic(name.into())],
            kind: RouteKind::Dynamic,
            specificity: 0,
            output_extension: None,
            static_html: false,
        }
    }

    fn make_runtime(project_root: &Path) {
        // Make the runtime check happy.
        std::fs::create_dir_all(
            project_root
                .join("node_modules")
                .join("@takazudo")
                .join("zfb-runtime"),
        )
        .unwrap();
    }

    /// Fake adapter runner that records dispatch calls so tests can
    /// assert the post-render adapter dispatch fires (or doesn't) for
    /// each adapter configuration.
    struct FakeAdapterRunner {
        calls: RefCell<Vec<(String, AdapterBundleInput)>>,
    }
    impl FakeAdapterRunner {
        fn new() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
            }
        }
    }
    impl AdapterRunner for FakeAdapterRunner {
        fn run(&self, package: &str, input: &AdapterBundleInput) -> Result<AdapterBundleOutput> {
            self.calls
                .borrow_mut()
                .push((package.to_string(), input.clone()));
            Ok(AdapterBundleOutput {
                stdout: format!("fake adapter {package} ok\n"),
                stderr: String::new(),
            })
        }
    }

    #[test]
    fn resolve_minify_html_defaults_false_when_cli_and_config_omit() {
        let cfg = Config::default();
        assert!(!resolve_minify_html(BuildMinifyHtml::Unspecified, &cfg));
    }

    #[test]
    fn resolve_minify_html_uses_config_when_cli_omits() {
        let cfg = Config {
            minify_html: true,
            ..Config::default()
        };
        assert!(resolve_minify_html(BuildMinifyHtml::Unspecified, &cfg));
    }

    #[test]
    fn resolve_minify_html_cli_enable_beats_config_false() {
        let cfg = Config::default();
        assert!(resolve_minify_html(BuildMinifyHtml::Enabled, &cfg));
    }

    #[test]
    fn resolve_minify_html_cli_disable_beats_config_true() {
        let cfg = Config {
            minify_html: true,
            ..Config::default()
        };
        assert!(!resolve_minify_html(BuildMinifyHtml::Disabled, &cfg));
    }

    #[test]
    fn run_build_orchestrates_bundle_and_render_for_static_routes() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        let routes = vec![
            static_route(vec![], "pages/index.tsx"),
            static_route(vec!["about"], "pages/about.tsx"),
        ];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"));

        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        let (pages, _) = run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap();

        assert_eq!(pages, 2);
        assert_eq!(runner.bundle_calls.borrow().len(), 1);
        assert_eq!(runner.render_calls.borrow().len(), 1);
        let render_input = runner.render_calls.borrow();
        let render_input = render_input.first().unwrap();
        assert_eq!(render_input.route_universe.len(), 2);
        assert_eq!(render_input.dist_dir, outdir);

        // No v0 stub strings in the emitted files.
        for entry in &render_input.route_universe {
            let body = std::fs::read_to_string(outdir.join(&entry.output_path)).unwrap();
            assert!(
                !body.contains("<h1>zfb build (v1 stub)</h1>"),
                "v0 stub leaked into {}",
                entry.output_path.display()
            );
            assert!(body.contains("<main>"), "expected non-empty <main>");
        }
    }

    #[test]
    fn run_build_minify_html_disabled_preserves_rendered_html_shape() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        std::fs::create_dir_all(project_root.join("pages")).unwrap();
        std::fs::write(
            project_root.join("pages/index.tsx"),
            "export default function Page() { return null; }\n",
        )
        .unwrap();
        let routes = vec![static_route(vec![], "pages/index.tsx")];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"));
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();

        run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap();

        let html = std::fs::read_to_string(outdir.join("index.html")).unwrap();
        assert!(
            html.contains("\n  <body>"),
            "disabled minify path must preserve rendered formatting:\n{html}",
        );
        assert!(
            html.contains("<main> rendered / </main>"),
            "disabled minify path must preserve rendered text spacing:\n{html}",
        );
    }

    #[test]
    fn run_build_minify_html_enabled_runs_after_asset_and_base_rewrites() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        std::fs::create_dir_all(project_root.join("pages")).unwrap();
        std::fs::write(
            project_root.join("pages/index.tsx"),
            "export default function Page() { return null; }\n",
        )
        .unwrap();
        let routes = vec![static_route(vec![], "pages/index.tsx")];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"))
            .with_prod_asset_inputs(ProdAssetEmitterInputs {
                css: Some(AssetEmitterPayload {
                    bytes: b"body { color: red; }".to_vec(),
                    relative_path: PathBuf::from("assets/styles.css"),
                    stable_url: "/assets/styles.css".to_string(),
                    companions: Vec::new(),
                }),
                islands: None,
                client_scripts: Vec::new(),
            });
        let cfg = Config {
            base: Some("/docs/".to_string()),
            ..Config::default()
        };
        let fake_adapter = FakeAdapterRunner::new();

        run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: true,
        })
        .unwrap();

        let asset_name = std::fs::read_dir(outdir.join("assets"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .find(|name| name.starts_with("styles-") && name.ends_with(".css"))
            .expect("hashed stylesheet emitted");
        let html = std::fs::read_to_string(outdir.join("index.html")).unwrap();
        assert!(
            !html.contains("\n  <body>"),
            "enabled minify path must compact rendered HTML:\n{html}",
        );
        assert!(
            html.contains(&format!("/docs/assets/{asset_name}")),
            "minified HTML must contain the base-prefixed hashed asset URL:\n{html}",
        );
        assert!(
            !html.contains("/docs/assets/styles.css"),
            "stable asset URL must be rewritten before minification:\n{html}",
        );
        assert!(
            html.contains("/docs/about"),
            "link-base rewrite must run before minification:\n{html}",
        );
    }

    #[test]
    fn run_build_minify_html_skips_static_passthrough_and_non_html_outputs() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        std::fs::create_dir_all(project_root.join("pages")).unwrap();
        std::fs::write(
            project_root.join("pages/index.tsx"),
            "export default function Page() { return null; }\n",
        )
        .unwrap();
        std::fs::write(
            project_root.join("pages/feed.xml.tsx"),
            "export default function Feed() { return null; }\n",
        )
        .unwrap();
        let mut feed = static_route(vec!["feed.xml"], "pages/feed.xml.tsx");
        feed.output_extension = Some("xml".to_string());
        let mut raw = static_route(vec!["raw"], "pages/raw.html");
        raw.static_html = true;
        let routes = vec![static_route(vec![], "pages/index.tsx"), raw, feed];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"))
            .with_static_html_output_paths(vec![PathBuf::from("raw/index.html")]);
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();

        run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: true,
        })
        .unwrap();

        let rendered = std::fs::read_to_string(outdir.join("index.html")).unwrap();
        assert!(
            !rendered.contains("\n  <body>"),
            "ordinary rendered HTML should be minified:\n{rendered}",
        );

        let passthrough = std::fs::read_to_string(outdir.join("raw/index.html")).unwrap();
        assert!(
            passthrough.contains("\n  <body>"),
            "static .html passthrough output must stay verbatim:\n{passthrough}",
        );

        let feed = std::fs::read_to_string(outdir.join("feed.xml")).unwrap();
        assert!(
            feed.contains("\n  <body>"),
            "non-HTML SSG output must not be sent through HTML minification:\n{feed}",
        );
    }

    /// zfb#231 regression — the SSR worker bundle (`bundle.mjs` + its
    /// `.map`) is a build intermediate, NOT a deploy artifact. It must
    /// be written under `<project_root>/.zfb-build/`, not under
    /// `<outdir>/.zfb-build/`. Anything in `outdir` ships to the deploy
    /// upload (Cloudflare Pages, Netlify, S3 + CloudFront, etc.); the
    /// SSR bundle is ~350 KB of internal build state that exposes
    /// page-level JS authors wrote with the "runs server-side only"
    /// assumption. Pinning the location prevents a future refactor
    /// from accidentally re-introducing the leak.
    #[test]
    fn run_build_writes_intermediate_bundle_outside_dist() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        let routes = vec![static_route(vec![], "pages/index.tsx")];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"));
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap();

        // (a) The bundler was handed `<project_root>/.zfb-build/`, NOT
        //     `<outdir>/.zfb-build/`. This is the load-bearing wiring
        //     fix: change this and dist/ stops leaking the SSR bundle.
        let calls = runner.bundle_calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].outdir, project_root.join(".zfb-build"));
        assert_ne!(calls[0].outdir, outdir.join(".zfb-build"));

        // (b) On disk: dist/.zfb-build/ does NOT exist after build.
        //     A future change that moves the write target back under
        //     dist/ would fail this assertion.
        assert!(
            !outdir.join(".zfb-build").exists(),
            "dist/.zfb-build/ must not exist after build (zfb#231)",
        );

        // (c) The relocated intermediate IS written at project root
        //     (FakeRunner mirrors production by writing the mock
        //     bundle to its target path).
        assert!(
            project_root.join(".zfb-build/bundle.mjs").is_file(),
            "<project_root>/.zfb-build/bundle.mjs must exist after build",
        );
    }

    #[test]
    fn run_build_defers_dynamic_routes_whose_source_is_unreadable() {
        // The router yielded a dynamic route whose source file doesn't
        // exist on disk (e.g. it lives somewhere the test fixture
        // didn't stage). expand_dynamic_routes must defer it with a
        // reason and the build must continue with just the static
        // route.
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        let routes = vec![
            static_route(vec![], "pages/index.tsx"),
            dynamic_route("slug", "pages/[slug].tsx"),
        ];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"));
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        let (pages, _) = run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap();
        // Only the static route reaches the renderer; the dynamic one
        // was deferred because its source couldn't be read.
        assert_eq!(pages, 1);
        let render_input = runner.render_calls.borrow();
        assert_eq!(render_input[0].route_universe.len(), 1);
        assert_eq!(render_input[0].route_universe[0].url_path, "/");
    }

    #[test]
    fn run_build_expands_dynamic_routes_with_literal_paths_export() {
        // Stage a dynamic page on disk with a literal `paths()`. The
        // build should pass it through expand_dynamic_routes and hand
        // both the static index AND every resolved dynamic URL to the
        // renderer.
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        std::fs::create_dir_all(project_root.join("pages/blog")).unwrap();
        std::fs::write(
            project_root.join("pages/blog/[slug].tsx"),
            "export function paths() {\n\
                return [\n\
                    { params: { slug: \"a\" } },\n\
                    { params: { slug: \"b\" } }\n\
                ];\n\
             }\n\
             export default function P() { return null; }\n",
        )
        .unwrap();
        let routes = vec![
            static_route(vec![], "pages/index.tsx"),
            zfb_router::Route {
                source_path: PathBuf::from("pages/blog/[slug].tsx"),
                segments: vec![
                    zfb_router::Segment::Static("blog".into()),
                    zfb_router::Segment::Dynamic("slug".into()),
                ],
                kind: zfb_router::RouteKind::Dynamic,
                specificity: 0,
                output_extension: None,
                static_html: false,
            },
        ];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"));
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        let (pages, _) = run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap();
        // 1 static + 2 expanded dynamic.
        assert_eq!(pages, 3);
        let render_input = runner.render_calls.borrow();
        assert_eq!(render_input[0].route_universe.len(), 3);
        // Static route first, then expanded dynamic in input order.
        assert_eq!(render_input[0].route_universe[0].url_path, "/");
        assert_eq!(render_input[0].route_universe[1].url_path, "/blog/a");
        assert_eq!(render_input[0].route_universe[2].url_path, "/blog/b");
        // Resolved dynamic entries keep the dynamic template as their
        // route_key so the prerender map join still works.
        assert_eq!(render_input[0].route_universe[1].route_key, "/blog/:slug");
        assert_eq!(render_input[0].route_universe[2].route_key, "/blog/:slug");
    }

    #[test]
    fn run_build_with_no_static_routes_and_unresolvable_dynamic_routes_returns_zero() {
        // When all dynamic routes fail both static AND runtime path
        // expansion, the build returns 0 pages. With the runtime paths
        // evaluation path enabled, the bundler IS invoked (the bundle is
        // needed to host the /__paths__ endpoint), but the renderer is
        // NOT called (no routes to render after all attempts).
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        // pages/[slug].tsx doesn't exist on disk; static expansion
        // defers it, and the FakeRunner's eval_deferred_paths returns
        // empty (no runtime V8 host in unit tests).
        let routes = vec![dynamic_route("slug", "pages/[slug].tsx")];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"));
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        let (pages, _) = run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap();
        assert_eq!(pages, 0);
        // The bundler is called (needed for runtime paths() evaluation).
        assert_eq!(runner.bundle_calls.borrow().len(), 1);
        // The renderer is NOT called (zero routes resolved).
        assert!(runner.render_calls.borrow().is_empty());
    }

    #[test]
    fn run_build_propagates_renderer_error() {
        // A failing fake runner simulates a RenderFailed. The CLI must
        // not paper over this — it should bubble the anyhow error up
        // so the centralized error formatter renders it for the user.
        struct FailingRunner;
        impl BuildRunner for FailingRunner {
            fn bundle(&self, _input: BundlerInput) -> Result<BundlerOutput> {
                Ok(BundlerOutput {
                    bundle_path: PathBuf::from("/dev/null"),
                    sourcemap_path: PathBuf::from("/dev/null"),
                    manifest: BundleManifest {
                        framework: "preact".into(),
                        jsx_import_source: "preact".into(),
                        hydrate_shim_specifier: "zfb:internal/preact/hydrate".into(),
                        bundle_basename: "bundle.mjs".into(),
                        routes: vec![],
                    },
                    route_module_deps: Vec::new(),
                })
            }
            fn eval_deferred_paths(
                &self,
                deferred: &[DeferredDynamicRoute],
                _bundle_out: &BundlerOutput,
                _cache: &mut PathsCache,
            ) -> Result<(
                crate::render_pipeline::DynamicExpansion,
                Backend,
                WorkerHandle,
            )> {
                Ok((
                    crate::render_pipeline::DynamicExpansion {
                        resolved: Vec::new(),
                        deferred: deferred.to_vec(),
                        ..Default::default()
                    },
                    Backend::Stub {
                        handler: std::sync::Arc::new(|_url| HttpResponseLike {
                            status: 200,
                            content_type: "text/html".into(),
                            body: b"<html/>".to_vec(),
                            ..Default::default()
                        }),
                    },
                    WorkerHandle(None),
                ))
            }
            fn render_all(&self, _input: RendererInput) -> Result<RendererOutput> {
                Err(anyhow!("renderer crashed at pages/error.tsx:5:3"))
            }
            fn emit_prod_assets(
                &self,
                _project_root: &Path,
                _user_pages_dir: &Path,
                _package_route_entrypoints: &[PathBuf],
                _outdir: &Path,
                _config: &Config,
            ) -> Result<(ProdAssetEmitterInputs, std::collections::BTreeSet<String>)> {
                Ok((
                    ProdAssetEmitterInputs::default(),
                    std::collections::BTreeSet::new(),
                ))
            }
        }
        let tmp = tempdir().unwrap();
        make_runtime(tmp.path());
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        let routes = vec![static_route(vec!["about"], "pages/about.tsx")];
        let err = run_build(BuildArgsResolved {
            project_root: tmp.path(),
            build_pages_root: tmp.path(),
            user_pages_dir: tmp.path(),
            package_route_entrypoints: &[],
            outdir: &tmp.path().join("dist"),
            config: &cfg,
            routes: &routes,
            runner: &FailingRunner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("renderer step failed"), "{msg}");
        assert!(msg.contains("pages/error.tsx:5:3"), "{msg}");
    }

    // Note: there used to be a `run_build_errors_when_runtime_npm_package_missing`
    // test here, asserting `run_build` errored out at the runtime pre-check
    // when the project had no `node_modules`. That guard became unreachable
    // once `check_runtime_installed` started accepting the binary's embedded
    // vendor snapshot — which production binaries always carry. The legacy
    // on-disk-only error path is still covered by
    // `render_pipeline::tests::check_runtime_installed_errors_when_runtime_missing`
    // via the `_with_overrides` helper.

    #[test]
    fn run_build_with_adapter_none_passes_when_every_route_is_ssg() {
        // Default config has no adapter. As long as every route is SSG
        // (the prerender map default), the build proceeds. The adapter
        // runner must NOT be invoked.
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        let routes = vec![static_route(vec!["about"], "pages/about.tsx")];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"));
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap();
        assert!(
            fake_adapter.calls.borrow().is_empty(),
            "adapter dispatch must not fire when adapter is None",
        );
    }

    #[test]
    fn run_build_with_adapter_none_rejects_ssr_routes() {
        // Stage a page on disk that exports `prerender = false` so the
        // prerender map flips. With adapter:"none" the build must
        // refuse, naming the offending route, BEFORE bundling /
        // rendering happens (so neither runner is invoked).
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        std::fs::create_dir_all(project_root.join("pages/api")).unwrap();
        std::fs::write(
            project_root.join("pages/api/foo.tsx"),
            "export const frontmatter = { title: \"Foo\" };\nexport const prerender = false;\nexport default function() { return null; }\n",
        )
        .unwrap();

        let routes = vec![Route {
            source_path: PathBuf::from("pages/api/foo.tsx"),
            segments: vec![Segment::Static("api".into()), Segment::Static("foo".into())],
            kind: RouteKind::Static,
            specificity: 0,
            output_extension: None,
            static_html: false,
        }];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"));
        let cfg = Config::default(); // adapter is None
        let fake_adapter = FakeAdapterRunner::new();
        let err = run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("/api/foo"), "{msg}");
        assert!(msg.contains("requires SSR"), "{msg}");
        // The check must fail BEFORE bundling — heavy work shouldn't
        // happen for an unworkable config.
        assert!(runner.bundle_calls.borrow().is_empty());
        assert!(runner.render_calls.borrow().is_empty());
        assert!(fake_adapter.calls.borrow().is_empty());
    }

    #[test]
    fn run_build_with_adapter_set_invokes_adapter_runner_after_render() {
        // Same SSR-only fixture but with adapter set. The build must
        // succeed, bundle + render normally for SSG (none here), then
        // dispatch the adapter runner with the bundle path the runner
        // returned.
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        std::fs::create_dir_all(project_root.join("pages/api")).unwrap();
        std::fs::write(
            project_root.join("pages/api/foo.tsx"),
            "export const frontmatter = { title: \"Foo\" };\nexport const prerender = false;\nexport default function() { return null; }\n",
        )
        .unwrap();
        // Also stage one SSG page so static_routes is non-empty (the
        // build short-circuits with 0 pages otherwise, before dispatch).
        std::fs::write(
            project_root.join("pages/index.tsx"),
            "export default function() { return null; }\n",
        )
        .unwrap();

        let routes = vec![
            static_route(vec![], "pages/index.tsx"),
            Route {
                source_path: PathBuf::from("pages/api/foo.tsx"),
                segments: vec![Segment::Static("api".into()), Segment::Static("foo".into())],
                kind: RouteKind::Static,
                specificity: 0,
                output_extension: None,
                static_html: false,
            },
        ];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"));
        let cfg = Config {
            adapter: Some("@takazudo/zfb-adapter-cloudflare".into()),
            ..Config::default()
        };
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap();
        let calls = fake_adapter.calls.borrow();
        assert_eq!(calls.len(), 1, "adapter dispatch must run once");
        assert_eq!(calls[0].0, "@takazudo/zfb-adapter-cloudflare");
        // Adapter receives the same bundle the renderer used. The
        // bundle lives at <project_root>/.zfb-build/, not under
        // <outdir>/, so the deploy upload doesn't include it (zfb#231).
        assert_eq!(
            calls[0].1.input_bundle,
            project_root.join(".zfb-build/bundle.mjs")
        );
        assert_eq!(calls[0].1.outdir, outdir);
    }

    #[test]
    fn run_build_rejects_invalid_adapter_string() {
        // Empty adapter string is a typo, not "none". Surface the
        // problem instead of silently falling back.
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        let routes = vec![static_route(vec![], "pages/index.tsx")];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"));
        let cfg = Config {
            adapter: Some("   ".into()),
            ..Config::default()
        };
        let fake_adapter = FakeAdapterRunner::new();
        let err = run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("empty string"), "{msg}");
    }

    /// S4 happy path: when an emitter slot has bytes, the renderer
    /// receives `prod_head_assets` with the matching stable URL, the
    /// FakeRunner splices that URL into rendered HTML, and the
    /// post-render orchestration pass writes a hashed asset file to
    /// disk and rewrites the HTML to point at the hashed URL.
    #[test]
    fn run_build_writes_hashed_css_and_rewrites_html_when_css_emitter_has_bytes() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        let routes = vec![
            static_route(vec![], "pages/index.tsx"),
            static_route(vec!["about"], "pages/about.tsx"),
        ];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"))
            .with_prod_asset_inputs(ProdAssetEmitterInputs {
                css: Some(zfb_build::pipeline::AssetEmitterPayload {
                    bytes: b".btn{color:red}".to_vec(),
                    relative_path: PathBuf::from("assets/styles.css"),
                    stable_url: "/assets/styles.css".to_string(),
                    companions: Vec::new(),
                }),
                islands: None,
                ..Default::default()
            });
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap();

        // (a) CSS bytes land on disk under dist/assets/styles-<8hex>.css.
        let assets_entries: Vec<String> = std::fs::read_dir(outdir.join("assets"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            assets_entries.len(),
            1,
            "expected exactly one hashed asset on disk; got {assets_entries:?}",
        );
        let name = &assets_entries[0];
        assert!(
            name.starts_with("styles-")
                && name.ends_with(".css")
                && name.len() == "styles-12345678.css".len(),
            "expected styles-<8hex>.css; got {name}",
        );
        let bytes = std::fs::read(outdir.join("assets").join(name)).unwrap();
        assert!(!bytes.is_empty(), "hashed css must contain bytes");

        // (b) HTML files contain the hashed URL after rewrite.
        let hashed_url = format!("/assets/{name}");
        for rel in &["index.html", "about/index.html"] {
            let html = std::fs::read_to_string(outdir.join(rel)).unwrap();
            assert!(
                html.contains(&hashed_url),
                "hashed URL {hashed_url} missing from {rel}: {html}",
            );

            // (c) HTML does NOT contain the unhashed URL.
            assert!(
                !html.contains("/assets/styles.css\""),
                "stable URL leaked into {rel}: {html}",
            );
        }

        // The renderer was handed a non-None prod_head_assets carrying
        // the stable URL — that's the load-bearing wiring S4 fixes.
        let render_calls = runner.render_calls.borrow();
        assert_eq!(render_calls.len(), 1);
        let prod_assets = render_calls[0]
            .prod_head_assets
            .as_ref()
            .expect("prod build must populate prod_head_assets when emitter has bytes");
        assert_eq!(prod_assets.css_url.as_deref(), Some("/assets/styles.css"));
        assert!(prod_assets.island_module_urls.is_empty());
    }

    /// Gate regression (#976) + explicit-reference contract (#971 P2):
    /// with ONLY client-script payloads (css and islands both `None`) the
    /// post-render pipeline must still fire — the gate also checks
    /// `!client_scripts.is_empty()`. Without that check the hashed file
    /// would never land and the explicit `clientScript()` reference in the
    /// page would keep the stable URL.
    ///
    /// Client scripts are NOT auto-injected into the head; they ship only
    /// via the page's own `clientScript()` reference (simulated here by
    /// `with_page_client_script_refs`). `prod_head_assets` therefore stays
    /// `None` when css/islands have no bytes.
    #[test]
    fn run_build_writes_hashed_client_script_and_rewrites_html() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        let routes = vec![static_route(vec![], "pages/index.tsx")];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"))
            .with_prod_asset_inputs(ProdAssetEmitterInputs {
                css: None,
                islands: None,
                client_scripts: vec![zfb_build::pipeline::AssetEmitterPayload {
                    bytes: b"// search widget".to_vec(),
                    relative_path: PathBuf::from("assets/client/search-widget.js"),
                    stable_url: "/assets/client/search-widget.js".to_string(),
                    companions: Vec::new(),
                }],
            })
            // The page explicitly references the client script via
            // `clientScript("search-widget")`.
            .with_page_client_script_refs(vec!["/assets/client/search-widget.js".to_string()]);
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap();

        // (a) The bundle lands hashed at dist/assets/client/search-widget-<8hex>.js.
        let client_entries: Vec<String> = std::fs::read_dir(outdir.join("assets/client"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            client_entries.len(),
            1,
            "expected exactly one hashed client-script asset; got {client_entries:?}",
        );
        let name = &client_entries[0];
        assert!(
            name.starts_with("search-widget-")
                && name.ends_with(".js")
                && name.len() == "search-widget-12345678.js".len(),
            "expected search-widget-<8hex>.js; got {name}",
        );

        // (b) HTML carries the hashed URL (from the page's explicit
        // reference, rewritten by the pipeline); the stable URL does not leak.
        let hashed_url = format!("/assets/client/{name}");
        let html = std::fs::read_to_string(outdir.join("index.html")).unwrap();
        assert!(
            html.contains(&hashed_url),
            "hashed URL {hashed_url} missing from HTML: {html}",
        );
        assert!(
            !html.contains("\"/assets/client/search-widget.js\""),
            "stable URL leaked: {html}",
        );

        // (c) Client scripts are NOT auto-injected into the head: with no
        // CSS/islands bytes, `prod_head_assets` is `None` (#971 P2). The
        // client-script tag reaches HTML solely via the page's explicit
        // reference, not via head injection.
        let render_calls = runner.render_calls.borrow();
        assert!(
            render_calls[0].prod_head_assets.is_none(),
            "client scripts must not be auto-injected into the head; \
             prod_head_assets should be None with no css/islands bytes, got {:?}",
            render_calls[0].prod_head_assets,
        );
    }

    /// #971 P2 regression: a page that does NOT reference any client script
    /// gets NO client-script tag, even when the build has client-script
    /// payloads. The hashed asset still lands on disk (it is shipped for the
    /// pages that DO reference it), but the unreferencing page's HTML carries
    /// neither the stable nor the hashed client-script URL.
    #[test]
    fn run_build_does_not_inject_client_script_into_unreferencing_page() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        let routes = vec![static_route(vec![], "pages/index.tsx")];
        // A client-script payload exists, but the page never references it
        // (no `with_page_client_script_refs`).
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"))
            .with_prod_asset_inputs(ProdAssetEmitterInputs {
                css: None,
                islands: None,
                client_scripts: vec![zfb_build::pipeline::AssetEmitterPayload {
                    bytes: b"// search widget".to_vec(),
                    relative_path: PathBuf::from("assets/client/search-widget.js"),
                    stable_url: "/assets/client/search-widget.js".to_string(),
                    companions: Vec::new(),
                }],
            });
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap();

        // The hashed bundle still lands on disk (shipped for referencing pages).
        let client_entries: Vec<String> = std::fs::read_dir(outdir.join("assets/client"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(client_entries.len(), 1, "hashed asset still ships");

        // The unreferencing page carries NO client-script URL — neither the
        // stable URL nor any hashed `/assets/client/...` variant.
        let html = std::fs::read_to_string(outdir.join("index.html")).unwrap();
        assert!(
            !html.contains("/assets/client/"),
            "no client-script tag should appear on a page that did not \
             reference it: {html}",
        );

        // And `prod_head_assets` is `None` (no css/islands, no auto-inject).
        let render_calls = runner.render_calls.borrow();
        assert!(render_calls[0].prod_head_assets.is_none());
    }

    /// `apply_asset_url_base` mounts each emitter slot's `stable_url`
    /// under the configured `base` prefix. None / empty / "/" bases
    /// are pure no-ops (byte-identical to the pre-`base` engine).
    /// Subpath and absolute-URL bases prefix every populated slot.
    /// The function only mutates populated slots — `None` slots stay
    /// `None`.
    #[test]
    fn apply_asset_url_base_prefixes_populated_slots() {
        fn fixture() -> ProdAssetEmitterInputs {
            ProdAssetEmitterInputs {
                css: Some(zfb_build::pipeline::AssetEmitterPayload {
                    bytes: b".x{}".to_vec(),
                    relative_path: PathBuf::from("assets/styles.css"),
                    stable_url: "/assets/styles.css".to_string(),
                    companions: Vec::new(),
                }),
                islands: Some(zfb_build::pipeline::AssetEmitterPayload {
                    bytes: b"// js".to_vec(),
                    relative_path: PathBuf::from("assets/islands.js"),
                    stable_url: "/assets/islands.js".to_string(),
                    companions: Vec::new(),
                }),
                client_scripts: vec![zfb_build::pipeline::AssetEmitterPayload {
                    bytes: b"// widget".to_vec(),
                    relative_path: PathBuf::from("assets/client/search-widget.js"),
                    stable_url: "/assets/client/search-widget.js".to_string(),
                    companions: Vec::new(),
                }],
            }
        }

        // None ⇒ no mutation.
        let mut inputs = fixture();
        apply_asset_url_base(&mut inputs, None);
        assert_eq!(
            inputs.css.as_ref().unwrap().stable_url,
            "/assets/styles.css"
        );
        assert_eq!(
            inputs.islands.as_ref().unwrap().stable_url,
            "/assets/islands.js"
        );
        assert_eq!(
            inputs.client_scripts[0].stable_url,
            "/assets/client/search-widget.js"
        );

        // "" ⇒ no mutation.
        let mut inputs = fixture();
        apply_asset_url_base(&mut inputs, Some(""));
        assert_eq!(
            inputs.css.as_ref().unwrap().stable_url,
            "/assets/styles.css"
        );
        assert_eq!(
            inputs.islands.as_ref().unwrap().stable_url,
            "/assets/islands.js"
        );

        // "/" ⇒ no mutation (root-mounted site).
        let mut inputs = fixture();
        apply_asset_url_base(&mut inputs, Some("/"));
        assert_eq!(
            inputs.css.as_ref().unwrap().stable_url,
            "/assets/styles.css"
        );
        assert_eq!(
            inputs.islands.as_ref().unwrap().stable_url,
            "/assets/islands.js"
        );

        // "/pj/zudo-doc/" ⇒ subpath prefix.
        let mut inputs = fixture();
        apply_asset_url_base(&mut inputs, Some("/pj/zudo-doc/"));
        assert_eq!(
            inputs.css.as_ref().unwrap().stable_url,
            "/pj/zudo-doc/assets/styles.css"
        );
        assert_eq!(
            inputs.islands.as_ref().unwrap().stable_url,
            "/pj/zudo-doc/assets/islands.js"
        );
        assert_eq!(
            inputs.client_scripts[0].stable_url,
            "/pj/zudo-doc/assets/client/search-widget.js"
        );

        // "/pj/zudo-doc" (no trailing slash) ⇒ same prefix.
        let mut inputs = fixture();
        apply_asset_url_base(&mut inputs, Some("/pj/zudo-doc"));
        assert_eq!(
            inputs.css.as_ref().unwrap().stable_url,
            "/pj/zudo-doc/assets/styles.css"
        );

        // CDN-hosted absolute URL ⇒ absolute prefix.
        let mut inputs = fixture();
        apply_asset_url_base(&mut inputs, Some("https://cdn.example.com/"));
        assert_eq!(
            inputs.css.as_ref().unwrap().stable_url,
            "https://cdn.example.com/assets/styles.css"
        );
        assert_eq!(
            inputs.islands.as_ref().unwrap().stable_url,
            "https://cdn.example.com/assets/islands.js"
        );
        assert_eq!(
            inputs.client_scripts[0].stable_url,
            "https://cdn.example.com/assets/client/search-widget.js"
        );

        // None slots stay None; the empty client_scripts Vec stays empty.
        let mut inputs = ProdAssetEmitterInputs {
            css: None,
            islands: None,
            ..Default::default()
        };
        apply_asset_url_base(&mut inputs, Some("/pj/zudo-doc/"));
        assert!(inputs.css.is_none());
        assert!(inputs.islands.is_none());
        assert!(inputs.client_scripts.is_empty());
    }

    /// End-to-end: with `config.base = "/pj/zudo-doc/"` set, the
    /// hashed asset URL the rewrite pass injects into HTML is
    /// `/pj/zudo-doc/assets/styles-<hash>.css` — NOT
    /// `/assets/styles-<hash>.css`. This is the PR #1361 acceptance
    /// case the upstream PR is opened against.
    ///
    /// The pairing matters: the renderer-emitted reference and the
    /// `boundary_replace` rewrite key must both see the prefixed
    /// stable_url, otherwise the rewrite never fires and the
    /// unprefixed URL leaks through.
    #[test]
    fn run_build_with_base_emits_prefixed_hashed_css_url_in_html() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        let routes = vec![static_route(vec![], "pages/index.tsx")];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"))
            .with_prod_asset_inputs(ProdAssetEmitterInputs {
                css: Some(zfb_build::pipeline::AssetEmitterPayload {
                    bytes: b".btn{color:red}".to_vec(),
                    relative_path: PathBuf::from("assets/styles.css"),
                    // The CSS emitter seeds this from
                    // `zfb_types::STABLE_CSS_URL`; the build path
                    // re-prefixes with `config.base` before handing
                    // it to the renderer.
                    stable_url: "/assets/styles.css".to_string(),
                    companions: Vec::new(),
                }),
                islands: None,
                ..Default::default()
            });
        let cfg = Config {
            base: Some("/pj/zudo-doc/".to_string()),
            ..Config::default()
        };
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap();

        // (a) The hashed asset still lands at dist/assets/styles-<8hex>.css
        // — `base` only affects the public URL, not the on-disk layout.
        let assets_entries: Vec<String> = std::fs::read_dir(outdir.join("assets"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(assets_entries.len(), 1);
        let name = &assets_entries[0];
        assert!(
            name.starts_with("styles-") && name.ends_with(".css"),
            "expected styles-<hash>.css; got {name}",
        );

        // (b) The HTML carries the PREFIXED hashed URL.
        let prefixed_hashed = format!("/pj/zudo-doc/assets/{name}");
        let html = std::fs::read_to_string(outdir.join("index.html")).unwrap();
        assert!(
            html.contains(&prefixed_hashed),
            "prefixed hashed URL {prefixed_hashed} missing from HTML: {html}",
        );

        // (c) Neither the unprefixed stable URL nor the unprefixed
        // hashed URL leaked through the rewrite.
        assert!(
            !html.contains("\"/assets/styles.css\""),
            "stable URL leaked: {html}",
        );
        let unprefixed_hashed = format!("\"/assets/{name}\"");
        assert!(
            !html.contains(&unprefixed_hashed),
            "unprefixed hashed URL leaked: {html}",
        );

        // (d) The renderer was handed the PREFIXED stable URL — the
        // load-bearing wiring this PR adds.
        let render_calls = runner.render_calls.borrow();
        let prod_assets = render_calls[0]
            .prod_head_assets
            .as_ref()
            .expect("prod_head_assets must be populated");
        assert_eq!(
            prod_assets.css_url.as_deref(),
            Some("/pj/zudo-doc/assets/styles.css"),
        );
    }

    /// End-to-end (islands variant): with `config.base = "/pj/zudo-doc/"`
    /// AND both CSS + islands emitters populated, the rendered HTML
    /// must carry the prefix on BOTH the `<link rel="stylesheet">`
    /// href AND the `<script type="module">` src. The companion test
    /// `run_build_with_base_emits_prefixed_hashed_css_url_in_html`
    /// only populates the CSS slot, so the islands path was untested
    /// against the prefix-rewrite contract before this test landed.
    ///
    /// Specifically asserts the renderer-emission contract (FakeRunner
    /// splices the stable URL it receives, then `boundary_replace`
    /// rewrites stable→hashed in-place): if `apply_asset_url_base`
    /// fails to prefix the islands `stable_url`, the rendered HTML
    /// would carry an unprefixed `/assets/islands-<hash>.js` and the
    /// assertion below catches it.
    #[test]
    fn run_build_with_base_emits_prefixed_hashed_islands_url_in_html() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        let routes = vec![static_route(vec![], "pages/index.tsx")];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"))
            .with_prod_asset_inputs(ProdAssetEmitterInputs {
                css: Some(zfb_build::pipeline::AssetEmitterPayload {
                    bytes: b".btn{color:red}".to_vec(),
                    relative_path: PathBuf::from("assets/styles.css"),
                    stable_url: "/assets/styles.css".to_string(),
                    companions: Vec::new(),
                }),
                islands: Some(zfb_build::pipeline::AssetEmitterPayload {
                    bytes: b"globalThis.__zfb_islands??=[];".to_vec(),
                    relative_path: PathBuf::from("assets/islands.js"),
                    stable_url: "/assets/islands.js".to_string(),
                    companions: Vec::new(),
                }),
                ..Default::default()
            });
        let cfg = Config {
            base: Some("/pj/zudo-doc/".to_string()),
            ..Config::default()
        };
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap();

        // (a) Both hashed assets land at dist/assets/<name>-<8hex>.<ext>.
        let mut assets_entries: Vec<String> = std::fs::read_dir(outdir.join("assets"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assets_entries.sort();
        assert_eq!(
            assets_entries.len(),
            2,
            "expected both hashed assets; got {assets_entries:?}"
        );
        let css_name = assets_entries
            .iter()
            .find(|n| n.starts_with("styles-") && n.ends_with(".css"))
            .expect("hashed CSS asset missing")
            .clone();
        let js_name = assets_entries
            .iter()
            .find(|n| n.starts_with("islands-") && n.ends_with(".js"))
            .expect("hashed islands asset missing")
            .clone();

        // (b) The HTML carries the PREFIXED hashed URLs for BOTH
        // the stylesheet link AND the islands script.
        let html = std::fs::read_to_string(outdir.join("index.html")).unwrap();
        let prefixed_css = format!("/pj/zudo-doc/assets/{css_name}");
        let prefixed_js = format!("/pj/zudo-doc/assets/{js_name}");
        assert!(
            html.contains(&prefixed_css),
            "prefixed hashed CSS URL {prefixed_css} missing from HTML: {html}",
        );
        assert!(
            html.contains(&prefixed_js),
            "prefixed hashed islands URL {prefixed_js} missing from HTML: {html}",
        );

        // (c) No unprefixed asset URL — neither the stable nor the
        // hashed shape — leaked through the rewrite for either slot.
        for leaked in [
            "\"/assets/styles.css\"",
            "\"/assets/islands.js\"",
            &format!("\"/assets/{css_name}\""),
            &format!("\"/assets/{js_name}\""),
        ] {
            assert!(
                !html.contains(leaked),
                "unprefixed asset URL leaked into HTML ({leaked}): {html}",
            );
        }

        // (d) The renderer was handed PREFIXED stable URLs for both
        // slots — the `apply_asset_url_base` wiring covers islands
        // too, not just CSS.
        let render_calls = runner.render_calls.borrow();
        let prod_assets = render_calls[0]
            .prod_head_assets
            .as_ref()
            .expect("prod_head_assets must be populated");
        assert_eq!(
            prod_assets.css_url.as_deref(),
            Some("/pj/zudo-doc/assets/styles.css"),
        );
        assert_eq!(
            prod_assets.island_module_urls,
            vec!["/pj/zudo-doc/assets/islands.js".to_string()],
        );
    }

    /// Dev-no-regression assertion (S4 spec): with no emitter bytes
    /// (the production path users have today, before S5/S6 wires real
    /// CSS/islands), the renderer is handed `prod_head_assets: None`,
    /// no hashed asset file is written, and the dist HTML stays
    /// byte-identical to a pre-S4 build for the same fixture.
    ///
    /// We assert byte-identity by re-rendering the same fixture
    /// without any emitter inputs (the orchestrator's `if !inputs.css
    /// || !inputs.islands` gate short-circuits) and verifying the
    /// resulting HTML matches the FakeRunner's deterministic output
    /// shape with no head-injected tags.
    #[test]
    fn run_build_with_no_emitter_bytes_skips_orchestrator_and_does_not_inject_head() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        let routes = vec![static_route(vec![], "pages/index.tsx")];
        // Default FakeRunner: no preset emitter bytes ⇒ both slots None.
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"));
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap();

        // No assets/ directory — the orchestrator was never invoked.
        assert!(
            !outdir.join("assets").exists(),
            "assets/ must be absent when no emitter produced bytes",
        );

        // Renderer received `prod_head_assets: None` (matching the
        // pre-S4 / dev contract).
        let render_calls = runner.render_calls.borrow();
        assert!(
            render_calls[0].prod_head_assets.is_none(),
            "prod_head_assets must be None when no emitter has bytes",
        );

        // HTML body shape: FakeRunner emits `<head>` empty when no
        // head injection. No `<link>` or `<script>` snuck in.
        let html = std::fs::read_to_string(outdir.join("index.html")).unwrap();
        assert!(
            !html.contains("<link rel=\"stylesheet\""),
            "no CSS link should appear without emitter bytes: {html}",
        );
        assert!(
            !html.contains("<script type=\"module\""),
            "no islands script should appear without emitter bytes: {html}",
        );
    }

    /// `resolve_input_global_css` honours the legacy
    /// `<root>/styles/global.css` location.
    #[test]
    fn resolve_input_global_css_legacy_root_styles() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("styles")).unwrap();
        let target = project_root.join("styles/global.css");
        std::fs::write(&target, ":root{}\n").unwrap();
        assert_eq!(
            resolve_input_global_css(project_root),
            Some(target),
            "legacy <root>/styles/global.css should resolve",
        );
    }

    /// `resolve_input_global_css` falls back to
    /// `<root>/src/styles/global.css` when the legacy location is
    /// absent. This is the conventional `src/`-rooted layout used by
    /// real-world consumers (e.g. zudo-doc; zudolab/zudo-doc#1355
    /// wave 13). Pre-fix the upstream probe missed this file
    /// entirely and dropped the host's `@theme` block on the floor.
    #[test]
    fn resolve_input_global_css_src_styles_fallback() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("src/styles")).unwrap();
        let target = project_root.join("src/styles/global.css");
        std::fs::write(&target, ":root{}\n").unwrap();
        assert_eq!(
            resolve_input_global_css(project_root),
            Some(target),
            "src/styles/global.css fallback should resolve when legacy is absent",
        );
    }

    /// Legacy `<root>/styles/global.css` wins when both layouts are
    /// present, so existing projects on the original convention see
    /// no behaviour change.
    #[test]
    fn resolve_input_global_css_prefers_legacy_when_both_exist() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("styles")).unwrap();
        std::fs::create_dir_all(project_root.join("src/styles")).unwrap();
        let legacy = project_root.join("styles/global.css");
        let src = project_root.join("src/styles/global.css");
        std::fs::write(&legacy, ":root{ /* legacy */ }\n").unwrap();
        std::fs::write(&src, ":root{ /* src */ }\n").unwrap();
        assert_eq!(
            resolve_input_global_css(project_root),
            Some(legacy),
            "legacy convention should win when both files exist",
        );
    }

    /// Neither file present => `None`. The CSS emitter still runs
    /// (preflight + scanned utilities) but the user's `@theme` is
    /// simply not contributed.
    #[test]
    fn resolve_input_global_css_none_when_neither_exists() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        assert_eq!(resolve_input_global_css(project_root), None);
    }

    /// Regression (issue #824): `tailwind.enabled = false` disables only
    /// the Tailwind layers, NOT the authored-CSS pipeline. With an
    /// authored global stylesheet and a CSS Module present, the emitter
    /// must still ship a stylesheet containing both — and crucially WITHOUT
    /// the Tailwind preflight (no `@import "tailwindcss"`, no subprocess).
    /// This path runs `AuthoredCssEngine`, so the test is hermetic (no
    /// tailwind binary required).
    #[test]
    fn css_payload_ships_authored_css_when_tailwind_disabled() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();

        // Authored global stylesheet at the conventional location.
        std::fs::create_dir_all(project_root.join("styles")).unwrap();
        std::fs::write(
            project_root.join("styles/global.css"),
            ".authored-global { color: rebeccapurple; }\n",
        )
        .unwrap();

        // A page importing a CSS Module so auto-discovery picks it up.
        std::fs::create_dir_all(project_root.join("pages")).unwrap();
        std::fs::write(
            project_root.join("pages/index.tsx"),
            "import styles from \"./index.module.css\";\nexport default function() { return null }\n",
        )
        .unwrap();
        std::fs::write(
            project_root.join("pages/index.module.css"),
            ".box { display: grid; }\n",
        )
        .unwrap();

        let cfg = Config {
            tailwind: Some(crate::config::TailwindConfig { enabled: false }),
            ..Config::default()
        };
        let payload =
            build_default_css_payload(project_root, &project_root.join("dist"), &cfg, &[])
                .expect("should not error")
                .expect(
                    "expected Some payload: authored CSS + module must ship even with tailwind off",
                );

        let css = String::from_utf8(payload.bytes).unwrap();
        assert!(
            css.contains(".authored-global"),
            "authored global CSS must survive tailwind.enabled=false; got:\n{css}",
        );
        assert!(
            css.contains("display: grid") || css.contains("display:grid"),
            "CSS Module rule must be emitted; got:\n{css}",
        );
        // The Tailwind layers must be skipped entirely — no preflight, no
        // synthesised import.
        assert!(
            !css.contains("@import \"tailwindcss\""),
            "tailwind import must NOT be synthesised when disabled; got:\n{css}",
        );
        assert!(
            !css.contains("tailwindcss v4"),
            "tailwind preflight banner must NOT appear when disabled; got:\n{css}",
        );

        // And the class-map producer must run in lockstep so the HTML
        // `class` attributes reference classes that actually ship.
        let maps = compute_css_module_class_maps(project_root).expect("class maps");
        assert!(
            !maps.is_empty(),
            "CSS Modules class maps must be non-empty when tailwind is disabled",
        );
        let scoped = maps
            .values()
            .flat_map(|m| m.values())
            .any(|scoped| scoped.ends_with("_box") || scoped.contains("box"));
        assert!(
            scoped,
            "scoped class for `.box` must appear in the class map; got: {maps:?}",
        );
    }

    /// `tailwind.enabled = false` AND no authored CSS AND no CSS Modules
    /// => no stylesheet to ship, so the emitter slot stays `None` (avoids
    /// a `<link>` to an empty stylesheet).
    #[test]
    fn css_payload_none_when_tailwind_disabled_and_no_css() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        // A page with no module import and no global stylesheet.
        std::fs::create_dir_all(project_root.join("pages")).unwrap();
        std::fs::write(
            project_root.join("pages/index.tsx"),
            "export default function() { return null }\n",
        )
        .unwrap();
        let cfg = Config {
            tailwind: Some(crate::config::TailwindConfig { enabled: false }),
            ..Config::default()
        };
        let payload =
            build_default_css_payload(project_root, &project_root.join("dist"), &cfg, &[])
                .expect("should not error");
        assert!(
            payload.is_none(),
            "expected None when tailwind disabled and no authored CSS/modules; got {payload:?}",
        );
    }

    /// Regression (issue #824): with Tailwind disabled, a `@import
    /// "tailwindcss"` in the authored global stylesheet must be stripped
    /// (no subprocess resolves it, so emitting it would 404 in the
    /// browser) while the rest of the authored CSS survives.
    #[test]
    fn css_payload_strips_tailwind_import_when_disabled() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("styles")).unwrap();
        std::fs::write(
            project_root.join("styles/global.css"),
            "@import \"tailwindcss\";\n.real-rule { color: red; }\n",
        )
        .unwrap();
        let cfg = Config {
            tailwind: Some(crate::config::TailwindConfig { enabled: false }),
            ..Config::default()
        };
        let payload =
            build_default_css_payload(project_root, &project_root.join("dist"), &cfg, &[])
                .expect("should not error")
                .expect("authored CSS must still ship");
        let css = String::from_utf8(payload.bytes).unwrap();
        assert!(
            !css.contains("@import \"tailwindcss\""),
            "tailwind import must be stripped from authored CSS when disabled; got:\n{css}",
        );
        assert!(
            css.contains(".real-rule"),
            "non-import authored rules must survive the strip; got:\n{css}",
        );
    }

    #[test]
    fn strip_tailwind_imports_drops_only_tailwind_imports() {
        let input = concat!(
            "@import \"tailwindcss\";\n",
            "@import 'tailwindcss/preflight';\n",
            "@import \"tailwindcss/utilities\";\n",
            "@import \"./vendor.css\";\n",
            ".keep { color: blue; }\n",
        );
        let out = strip_tailwind_imports(input);
        assert!(
            !out.contains("tailwindcss"),
            "all tailwind imports gone; got:\n{out}"
        );
        assert!(
            out.contains("@import \"./vendor.css\""),
            "vendor import kept"
        );
        assert!(out.contains(".keep"), "authored rule kept");
    }

    #[test]
    fn strip_tailwind_imports_keeps_commented_import() {
        let input = "/* @import \"tailwindcss\"; */\n.keep { color: green; }\n";
        let out = strip_tailwind_imports(input);
        // A commented-out import is inert; leaving it is harmless and the
        // trimmed line starts with `/*`, not `@import`.
        assert!(
            out.contains("/* @import \"tailwindcss\"; */"),
            "commented import kept"
        );
        assert!(out.contains(".keep"), "authored rule kept");
    }

    /// No `pages/` directory => islands emitter slot is `None`.
    /// Mirror of the CSS-disabled coverage point for the islands
    /// branch — no subprocess required.
    #[test]
    fn default_runner_returns_none_islands_when_no_pages_dir() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        // No pages/, so the entry walk returns empty and we never
        // reach the scanner or esbuild.
        let (payload, names) = build_default_islands_payload(
            project_root,
            &project_root.join("pages"),
            &[],
            &project_root.join("dist"),
            crate::config::Framework::Preact,
            &IslandsPluginConfig::default(),
            IslandsGlobPolicy::HardError,
            None,
        )
        .expect("should not error");
        assert!(
            payload.is_none(),
            "expected None when project has no pages/; got {payload:?}",
        );
        assert!(
            names.is_empty(),
            "expected empty names when no pages/; got {names:?}"
        );
    }

    /// No `"use client"` components in the project => islands
    /// emitter slot is `None`. The scanner runs but yields an empty
    /// set; esbuild is never invoked.
    #[test]
    fn default_runner_returns_none_islands_when_no_use_client_components() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("pages")).unwrap();
        // A perfectly normal page with no `"use client"` directive.
        std::fs::write(
            project_root.join("pages/index.tsx"),
            "export default function Index() { return null; }\n",
        )
        .unwrap();
        let (payload, names) = build_default_islands_payload(
            project_root,
            &project_root.join("pages"),
            &[],
            &project_root.join("dist"),
            crate::config::Framework::Preact,
            &IslandsPluginConfig::default(),
            IslandsGlobPolicy::HardError,
            None,
        )
        .expect("should not error");
        assert!(
            payload.is_none(),
            "expected None when no use-client components; got {payload:?}",
        );
        assert!(
            names.is_empty(),
            "expected empty names when no islands; got {names:?}"
        );
    }

    /// Issue #822: a page that imports a module with a *misplaced*
    /// `"use client"` directive still emits no islands bundle (the
    /// directive is rejected), so the payload is `None`. This exercises
    /// the near-miss branch of the empty-islands report — the one that
    /// keeps the loud warning + verify-hint. We can't assert on the
    /// stderr text here, but the `None` return confirms the short-circuit
    /// fires on the same path the near-miss accounting flows through.
    #[test]
    fn default_runner_returns_none_islands_for_near_miss_directive() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("pages")).unwrap();
        std::fs::create_dir_all(project_root.join("components")).unwrap();
        std::fs::write(
            project_root.join("pages/index.tsx"),
            "import { Counter } from \"../components/counter\";\n\
             export default function Index() { return <Counter/>; }\n",
        )
        .unwrap();
        // Directive is not first in the prologue (an import precedes it),
        // so it is rejected and no island is registered.
        std::fs::write(
            project_root.join("components/counter.tsx"),
            "import { useState } from \"preact/hooks\";\n\
             \"use client\";\n\
             export function Counter() { return null; }\n",
        )
        .unwrap();
        let (payload, names) = build_default_islands_payload(
            project_root,
            &project_root.join("pages"),
            &[],
            &project_root.join("dist"),
            crate::config::Framework::Preact,
            &IslandsPluginConfig::default(),
            IslandsGlobPolicy::HardError,
            None,
        )
        .expect("should not error");
        assert!(
            payload.is_none(),
            "expected None for a misplaced use-client directive; got {payload:?}",
        );
        assert!(
            names.is_empty(),
            "expected empty names for near-miss directive; got {names:?}"
        );
    }

    /// Issue #1404 (relaxes the #1387 stopgap): an UNSUPPORTED
    /// `import.meta.glob` form (here the default LAZY form — no `{ eager:
    /// true }`) reachable from a `"use client"` island must still fail the
    /// build with a targeted message, because the islands esbuild pipeline
    /// cannot expand it and would ship the raw call to the browser (throwing
    /// at hydration). The unsupported-form detection is the shadow's
    /// pre-flight expand pass, which runs BEFORE any esbuild setup, so this
    /// test needs no subprocess/binary. (The SUPPORTED eager form is now
    /// expanded via the shadow — proven by the env-gated L3 test
    /// `islands_shadow_expands_glob_and_executes` in
    /// `crates/zfb-islands/tests/integration.rs`.)
    #[test]
    fn build_default_islands_payload_errors_on_unsupported_glob_form_reachable_from_island() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("pages")).unwrap();
        std::fs::create_dir_all(project_root.join("components")).unwrap();
        std::fs::write(
            project_root.join("pages/index.tsx"),
            "import { Gallery } from \"../components/gallery\";\n\
             export default function Index() { return <Gallery/>; }\n",
        )
        .unwrap();
        std::fs::write(
            project_root.join("components/gallery.tsx"),
            // Default (LAZY) form: no `{ eager: true }` — unsupported, so the
            // shadow pre-flight must keep the stopgap for it.
            "\"use client\";\n\
             const images = import.meta.glob('./images/*.png');\n\
             export function Gallery() { return null; }\n",
        )
        .unwrap();
        let err = build_default_islands_payload(
            project_root,
            &project_root.join("pages"),
            &[],
            &project_root.join("dist"),
            crate::config::Framework::Preact,
            &IslandsPluginConfig::default(),
            IslandsGlobPolicy::HardError,
            None,
        )
        .expect_err("an unsupported-form glob-using island must fail the build");
        let message = format!("{err:#}");
        assert!(
            message.contains("import.meta.glob"),
            "error must name the unsupported construct; got: {message}"
        );
        assert!(
            message.contains("gallery.tsx"),
            "error must name the offending file; got: {message}"
        );
        assert!(
            message.contains("1385"),
            "error must link the tracked follow-up issue #1385; got: {message}"
        );
    }

    #[test]
    fn build_default_islands_payload_hard_errors_on_shared_worker() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("pages")).unwrap();
        std::fs::create_dir_all(root.join("components")).unwrap();
        std::fs::write(
            root.join("pages/index.tsx"),
            "import { Island } from '../components/Island'; export default Island;\n",
        )
        .unwrap();
        std::fs::write(
            root.join("components/Island.tsx"),
            "'use client'; export function Island() { new SharedWorker(new URL('./shared.ts', import.meta.url)); return null; }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("components/shared.ts"),
            "self.onconnect = () => {};\n",
        )
        .unwrap();

        let error = build_default_islands_payload(
            root,
            &root.join("pages"),
            &[],
            &root.join("dist"),
            crate::config::Framework::Preact,
            &IslandsPluginConfig::default(),
            IslandsGlobPolicy::HardError,
            None,
        )
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("unsupported SharedWorker"), "{message}");
        assert!(message.contains("shared.ts"), "{message}");
    }

    /// Issue #1404: the shadow materialiser expands a SUPPORTED eager
    /// string-literal glob reachable from an island — it remaps the island's
    /// `source_path` into a shadow copy and never returns `KeepStopgap`, so
    /// the caller proceeds to esbuild rather than hard-erroring. This drives
    /// `materialise_islands_shadow` directly (no esbuild binary needed) and
    /// asserts the shadow's structure: the glob module is a REAL expanded
    /// file (no `import.meta.glob(` literal, the matched target present),
    /// while the plain island file is a symlink.
    #[test]
    fn materialise_islands_shadow_expands_supported_glob_and_remaps() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("components/widgets")).unwrap();
        // Island file (plain — no glob): imports the glob data module.
        let island_src = project_root.join("components/gallery.tsx");
        std::fs::write(
            &island_src,
            "\"use client\";\n\
             import { widgets } from \"./gallery-data\";\n\
             export function Gallery() { return null; }\n",
        )
        .unwrap();
        // Glob data module (eager string-literal — SUPPORTED).
        std::fs::write(
            project_root.join("components/gallery-data.tsx"),
            "export const widgets = import.meta.glob('./widgets/*.tsx', { eager: true });\n",
        )
        .unwrap();
        // A matched target of the glob — reachable ONLY through the macro.
        std::fs::write(
            project_root.join("components/widgets/a.tsx"),
            "export const a = 1;\n",
        )
        .unwrap();

        let islands = vec![zfb_islands::Island::new("Gallery", island_src.clone())];
        let scan_meta = zfb_islands::ScanMeta {
            uses_client_router: false,
            near_miss_candidates: 0,
            glob_reachable_from_islands: vec![project_root.join("components/gallery-data.tsx")],
            island_reachable_modules: vec![
                island_src.clone(),
                project_root.join("components/gallery-data.tsx"),
            ],
            raw_import_edges_from_islands: Vec::new(),
            module_worker_edges_from_islands: Vec::new(),
        };

        let outcome = materialise_islands_shadow(project_root, &islands, &scan_meta)
            .expect("shadow materialisation must not IO-error");
        let shadow = match outcome {
            IslandsShadowOutcome::Ready(s) => s,
            IslandsShadowOutcome::KeepStopgap(o) => {
                panic!("supported eager glob must NOT keep the stopgap; offenders: {o:?}")
            }
        };
        assert!(
            shadow.preserve_symlinks,
            "default islands shadow uses symlinked source files plus --preserve-symlinks"
        );

        // The island's source_path was remapped into the shadow.
        let shadow_island = shadow
            .remap
            .get(&island_src)
            .expect("island source_path must be remapped into the shadow");
        let shadow_root = shadow_island.parent().unwrap().parent().unwrap();

        // The glob module is a REAL expanded file: macro removed, matched
        // target imported as a namespace.
        let expanded =
            std::fs::read_to_string(shadow_root.join("components/gallery-data.tsx")).unwrap();
        assert!(
            !expanded.contains("import.meta.glob("),
            "shadow glob module must be expanded (no raw macro): {expanded}"
        );
        assert!(
            expanded.contains("./widgets/a.tsx"),
            "expanded glob must reference the matched target: {expanded}"
        );

        // The plain island file is a symlink (not a real copy).
        let island_meta =
            std::fs::symlink_metadata(shadow_root.join("components/gallery.tsx")).unwrap();
        assert!(
            island_meta.file_type().is_symlink(),
            "plain island file must be symlinked into the shadow, not copied"
        );

        // The glob target was materialised too (so esbuild can resolve the
        // generated `import * as … from \"./widgets/a.tsx\"`).
        assert!(
            std::fs::symlink_metadata(shadow_root.join("components/widgets/a.tsx")).is_ok(),
            "glob target must be present in the shadow"
        );
    }

    #[test]
    fn materialise_islands_shadow_expands_raw_import_and_keeps_js_target_terminal() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("pages")).unwrap();
        std::fs::create_dir_all(project_root.join("components")).unwrap();
        let page = project_root.join("pages/index.tsx");
        let island_src = project_root.join("components/shader.tsx");
        let raw_target = project_root.join("components/broken.js");
        std::fs::write(
            &page,
            "import { Shader } from '../components/shader';\nexport default Shader;\n",
        )
        .unwrap();
        std::fs::write(
            &island_src,
            "\"use client\";\nimport source from './broken.js?raw';\n\
             export function Shader() { return source; }\n",
        )
        .unwrap();
        // Invalid JS containing glob-looking bytes: as a raw target this is
        // text only and must neither parse nor trip nested-glob protection.
        std::fs::write(&raw_target, "not javascript {{{ import.meta.glob('./x')\n").unwrap();

        let resolver = FsResolver::new();
        let (islands, scan_meta) = scan_islands_with_meta(&[page], &resolver).unwrap();
        assert_eq!(scan_meta.raw_import_edges_from_islands.len(), 1);
        let shadow = match materialise_islands_shadow(project_root, &islands, &scan_meta).unwrap() {
            IslandsShadowOutcome::Ready(shadow) => shadow,
            IslandsShadowOutcome::KeepStopgap(offenders) => {
                panic!("terminal raw target must not keep glob stopgap: {offenders:?}")
            }
        };

        let shadow_island = shadow
            .remap
            .get(&island_src.canonicalize().unwrap())
            .unwrap();
        let rewritten = std::fs::read_to_string(shadow_island).unwrap();
        assert!(!rewritten.contains("?raw"), "{rewritten}");
        let generated = std::fs::read_dir(shadow_island.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".zfb-raw-") && name.ends_with(".mjs"))
            })
            .expect("generated raw module");
        let module = std::fs::read_to_string(generated).unwrap();
        assert!(module.contains("not javascript {{{ import.meta.glob"));
        assert!(
            shadow._tempdir.path().join("components/broken.js").exists(),
            "original target is mirrored as a terminal asset"
        );
        assert_eq!(
            shadow.raw_targets,
            std::collections::BTreeSet::from([raw_target.clone()])
        );
    }

    #[test]
    fn materialise_islands_shadow_rewrites_nested_worker_urls_without_importing_entries() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("pages")).unwrap();
        std::fs::create_dir_all(root.join("components/workers")).unwrap();
        let page = root.join("pages/index.tsx");
        let island = root.join("components/Island.tsx");
        let helper = root.join("components/start-worker.ts");
        let worker = root.join("components/workers/search.ts");
        let nested = root.join("components/workers/tokenize.ts");
        let worker_payload = root.join("components/workers/search.txt");
        let nested_payload = root.join("components/workers/tokenize.txt");
        std::fs::write(
            &page,
            "import { Island } from '../components/Island'; export default Island;\n",
        )
        .unwrap();
        std::fs::write(
            &island,
            "'use client'; import { start } from './start-worker'; export function Island() { start(); return null; }\n",
        )
        .unwrap();
        std::fs::write(
            &helper,
            "export const start = () => new Worker(new URL('./workers/search.ts', import.meta.url), { type: 'module' });\n",
        )
        .unwrap();
        std::fs::write(
            &worker,
            "import text from './search.txt?raw'; new Worker(new URL('./tokenize.ts', import.meta.url), { type: 'module' }); self.postMessage(text);\n",
        )
        .unwrap();
        std::fs::write(
            &nested,
            "import text from './tokenize.txt?raw'; self.postMessage(text);\n",
        )
        .unwrap();
        std::fs::write(&worker_payload, "search payload").unwrap();
        std::fs::write(&nested_payload, "tokenize payload").unwrap();

        let (islands, scan_meta) = scan_islands_with_meta(&[page], &FsResolver::new()).unwrap();
        assert_eq!(scan_meta.module_worker_edges_from_islands.len(), 2);
        assert_eq!(scan_meta.raw_import_edges_from_islands.len(), 2);
        let shadow = match materialise_islands_shadow(root, &islands, &scan_meta).unwrap() {
            IslandsShadowOutcome::Ready(shadow) => shadow,
            IslandsShadowOutcome::KeepStopgap(offenders) => {
                panic!("supported module workers must materialise: {offenders:?}")
            }
        };
        let shadow_island = shadow.remap.get(&island.canonicalize().unwrap()).unwrap();
        let shadow_root = shadow_island.parent().unwrap().parent().unwrap();
        let rewritten_helper =
            std::fs::read_to_string(shadow_root.join("components/start-worker.ts")).unwrap();
        assert!(
            rewritten_helper
                .contains("new URL(\"./worker-components-s-workers-s-search-d-ts.js?v="),
            "{rewritten_helper}"
        );
        assert!(rewritten_helper.contains(".js?v="), "{rewritten_helper}");
        assert!(
            !rewritten_helper.contains("import './workers/search.ts'"),
            "worker entry must not become an SSR/islands import: {rewritten_helper}"
        );
        let rewritten_worker =
            std::fs::read_to_string(shadow_root.join("components/workers/search.ts")).unwrap();
        assert!(
            rewritten_worker
                .contains("new URL(\"./worker-components-s-workers-s-tokenize-d-ts.js?v="),
            "{rewritten_worker}"
        );
        assert!(!rewritten_worker.contains("?raw"), "{rewritten_worker}");
        let rewritten_nested =
            std::fs::read_to_string(shadow_root.join("components/workers/tokenize.ts")).unwrap();
        assert!(!rewritten_nested.contains("?raw"), "{rewritten_nested}");
        assert!(
            shadow_root.join("components/workers/tokenize.ts").exists(),
            "nested worker entry is mirrored for the later emission pass"
        );
        assert_eq!(
            shadow.raw_targets,
            std::collections::BTreeSet::from([worker_payload, nested_payload])
        );
    }

    #[test]
    fn client_script_raw_stage_rewrites_transitive_importer_and_tracks_target() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("pages")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let entry = root.join("pages/widget.client.ts");
        let helper = root.join("src/helper.ts");
        let target = root.join("src/message.txt");
        std::fs::write(
            &entry,
            "import { message } from '../src/helper';\nconsole.log(message);\n",
        )
        .unwrap();
        std::fs::write(
            &helper,
            "import text from './message.txt?raw';\nexport const message = text;\n",
        )
        .unwrap();
        std::fs::write(&target, "hello\nraw\n").unwrap();
        let entries = vec![zfb_islands::client_scripts::ClientScriptEntry {
            entry_name: "widget".into(),
            source_path: entry,
        }];

        let stage = stage_client_script_preprocessing(root, &entries)
            .unwrap()
            .expect("raw graph needs a stage");
        assert!(stage.entries[0].source_path.exists());
        let staged_helper = stage.root.join("src/helper.ts");
        let rewritten = std::fs::read_to_string(&staged_helper).unwrap();
        assert!(!rewritten.contains("?raw"), "{rewritten}");
        let generated = std::fs::read_dir(stage.root.join("src"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".zfb-raw-") && name.ends_with(".mjs"))
            })
            .expect("generated raw module");
        assert!(std::fs::read_to_string(generated)
            .unwrap()
            .contains("hello\\nraw\\n"));
        assert_eq!(
            stage.raw_targets,
            std::collections::BTreeSet::from([target.clone()])
        );
    }

    #[test]
    fn client_script_worker_only_graph_gets_preprocessed_stage() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("pages")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let entry = root.join("pages/widget.client.ts");
        let helper = root.join("src/start.ts");
        let worker = root.join("src/search.worker.ts");
        let nested_worker = root.join("src/nested.worker.ts");
        let worker_helper = root.join("src/search-helper.ts");
        let payload = root.join("src/search.txt");
        std::fs::write(&entry, "import { start } from '../src/start'; start();\n").unwrap();
        std::fs::write(
            &helper,
            "export const start = () => new Worker(new URL('./search.worker.ts', import.meta.url), { type: 'module' });\n",
        )
        .unwrap();
        std::fs::write(
            &worker,
            "import { prefix } from './search-helper'; import text from './search.txt?raw'; new Worker(new URL('./nested.worker.ts', import.meta.url), { type: 'module' }); self.postMessage(prefix + text);\n",
        )
        .unwrap();
        std::fs::write(&nested_worker, "self.postMessage('nested');\n").unwrap();
        std::fs::write(&worker_helper, "export const prefix = 'ready:';\n").unwrap();
        std::fs::write(&payload, "ready payload").unwrap();
        let entries = vec![zfb_islands::client_scripts::ClientScriptEntry {
            entry_name: "widget".into(),
            source_path: entry,
        }];

        let stage = stage_client_script_preprocessing(root, &entries)
            .unwrap()
            .expect("worker-only graph needs the shared preprocessing stage");
        assert_eq!(
            stage.raw_targets,
            std::collections::BTreeSet::from([payload.clone()])
        );
        let rewritten = std::fs::read_to_string(stage.root.join("src/start.ts")).unwrap();
        assert!(
            rewritten.contains("new URL(\"./worker-src-s-search-d-worker-d-ts.js?v="),
            "{rewritten}"
        );
        assert!(rewritten.contains(".js?v="), "{rewritten}");
        let rewritten_worker =
            std::fs::read_to_string(stage.root.join("src/search.worker.ts")).unwrap();
        assert!(!rewritten_worker.contains("?raw"), "{rewritten_worker}");
        assert!(
            rewritten_worker.contains("./worker-src-s-nested-d-worker-d-ts.js?v="),
            "{rewritten_worker}"
        );
        assert!(stage.entries[0].source_path.starts_with(&stage.root));
        let workers = &stage.workers_by_entry["widget"];
        assert_eq!(workers.len(), 2);
        assert!(workers.iter().any(|worker| {
            worker.filename == "worker-src-s-search-d-worker-d-ts.js"
                && worker.source_path == stage.root.join("src/search.worker.ts")
        }));
        assert!(workers.iter().any(|worker| {
            worker.filename == "worker-src-s-nested-d-worker-d-ts.js"
                && worker.source_path == stage.root.join("src/nested.worker.ts")
        }));
        assert!(stage.worker_targets.contains(&worker));
        assert!(stage.worker_targets.contains(&nested_worker));
        assert!(stage.worker_targets.contains(&worker_helper));
        assert!(stage.worker_targets.contains(&payload));
    }

    #[test]
    fn client_script_worker_dev_outputs_change_on_second_tick_and_prune_on_third() {
        let tmp = tempdir().unwrap();
        let client_dir = tmp.path().join("assets/client");
        std::fs::create_dir_all(&client_dir).unwrap();
        let entry = client_dir.join("widget.js");
        let worker_name = "worker-src-s-search-d-worker-d-ts.js";
        let worker = client_dir.join(worker_name);

        assert!(write_dev_client_script_output_if_changed(
            &entry,
            b"new URL('./worker-src-s-search-d-worker-d-ts.js?v=11111111')"
        )
        .unwrap());
        assert!(write_dev_client_script_output_if_changed(&worker, b"worker-v1").unwrap());

        // Second watcher tick: a worker-source edit changes both its bundle
        // and the parent's rewritten cache query, so both files re-emit.
        assert!(write_dev_client_script_output_if_changed(
            &entry,
            b"new URL('./worker-src-s-search-d-worker-d-ts.js?v=22222222')"
        )
        .unwrap());
        assert!(write_dev_client_script_output_if_changed(&worker, b"worker-v2").unwrap());
        assert!(!write_dev_client_script_output_if_changed(&worker, b"worker-v2").unwrap());

        // Third tick removes the Worker constructor while retaining the
        // client entry; the stale stable companion is pruned.
        let previous =
            std::collections::HashSet::from(["widget.js".to_string(), worker_name.to_string()]);
        let current = std::collections::HashSet::from(["widget.js".to_string()]);
        assert!(prune_dev_client_script_outputs(
            &client_dir,
            &previous,
            &current
        ));
        assert!(!worker.exists());
        assert!(entry.exists());
    }

    #[cfg(unix)]
    #[test]
    fn client_script_raw_stage_materialises_symlinked_importer() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("pages")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let real_entry = root.join("src/widget.client.ts");
        let linked_entry = root.join("pages/widget.client.ts");
        std::fs::write(
            &real_entry,
            "import text from './message.txt?raw';\nconsole.log(text);\n",
        )
        .unwrap();
        // Resolution follows the logical symlinked entry location because
        // this staging mode enables esbuild's preserve-symlinks behavior.
        std::fs::write(root.join("pages/message.txt"), "from symlink\n").unwrap();
        std::os::unix::fs::symlink(&real_entry, &linked_entry).unwrap();
        let entries = vec![zfb_islands::client_scripts::ClientScriptEntry {
            entry_name: "widget".into(),
            source_path: linked_entry,
        }];

        let stage = stage_client_script_preprocessing(root, &entries)
            .unwrap()
            .expect("raw graph needs a stage");
        let staged_entry = &stage.entries[0].source_path;
        let meta = std::fs::symlink_metadata(staged_entry).unwrap();
        assert!(meta.file_type().is_file());
        assert!(!meta.file_type().is_symlink());
        let rewritten = std::fs::read_to_string(staged_entry).unwrap();
        assert!(!rewritten.contains("?raw"), "{rewritten}");
        assert!(std::fs::read_dir(staged_entry.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .any(|path| path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".zfb-raw-") && name.ends_with(".mjs"))));
    }

    #[test]
    fn client_raw_stage_retains_unrelated_external_registered_entry() {
        let project = tempdir().unwrap();
        let root = project.path();
        std::fs::create_dir_all(root.join("pages")).unwrap();
        let local = root.join("pages/local.client.ts");
        std::fs::write(
            &local,
            "import text from './message.txt?raw';\nconsole.log(text);\n",
        )
        .unwrap();
        std::fs::write(root.join("pages/message.txt"), "local raw").unwrap();
        let external_dir = tempdir().unwrap();
        let external = external_dir.path().join("package-entry.ts");
        std::fs::write(&external, "console.log('external');\n").unwrap();
        let entries = vec![
            zfb_islands::client_scripts::ClientScriptEntry {
                entry_name: "local".into(),
                source_path: local,
            },
            zfb_islands::client_scripts::ClientScriptEntry {
                entry_name: "package".into(),
                source_path: external.clone(),
            },
        ];

        let stage = stage_client_script_preprocessing(root, &entries)
            .unwrap()
            .expect("local raw graph needs a stage");
        let staged_external = stage
            .entries
            .iter()
            .find(|entry| entry.entry_name == "package")
            .unwrap();
        assert_eq!(staged_external.source_path, external);
        let staged_local = stage
            .entries
            .iter()
            .find(|entry| entry.entry_name == "local")
            .unwrap();
        assert!(staged_local.source_path.starts_with(&stage.root));
    }

    #[cfg(unix)]
    #[test]
    fn client_raw_stage_preprocesses_importer_beneath_symlinked_dir() {
        let project = tempdir().unwrap();
        let root = project.path();
        std::fs::create_dir_all(root.join("pages")).unwrap();
        let real = root.join("src-real");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(
            root.join("pages/widget.client.ts"),
            "import text from '../src-alias/helper';\nconsole.log(text);\n",
        )
        .unwrap();
        std::fs::write(
            real.join("helper.ts"),
            "import text from './message.txt?raw';\nexport default text;\n",
        )
        .unwrap();
        std::fs::write(real.join("message.txt"), "symlink dir client raw").unwrap();
        std::os::unix::fs::symlink(&real, root.join("src-alias")).unwrap();
        let entries = vec![zfb_islands::client_scripts::ClientScriptEntry {
            entry_name: "widget".into(),
            source_path: root.join("pages/widget.client.ts"),
        }];

        let stage = stage_client_script_preprocessing(root, &entries)
            .unwrap()
            .expect("raw graph needs a stage");
        let alias_helper = stage.root.join("src-alias/helper.ts");
        let staged = std::fs::read_to_string(&alias_helper).unwrap();
        assert!(!staged.contains("?raw"), "{staged}");
        assert!(std::fs::read_dir(alias_helper.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .any(|path| path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".zfb-raw-") && name.ends_with(".mjs"))));
    }

    #[test]
    fn client_script_without_raw_keeps_no_stage_fast_path() {
        let tmp = tempdir().unwrap();
        let entry = tmp.path().join("plain.client.ts");
        std::fs::write(&entry, "console.log('plain');\n").unwrap();
        let entries = vec![zfb_islands::client_scripts::ClientScriptEntry {
            entry_name: "plain".into(),
            source_path: entry,
        }];
        assert!(stage_client_script_preprocessing(tmp.path(), &entries)
            .unwrap()
            .is_none());
    }

    #[test]
    fn client_script_unsupported_query_is_a_hard_preprocess_error() {
        let tmp = tempdir().unwrap();
        let entry = tmp.path().join("bad.client.ts");
        std::fs::write(&entry, "import url from './x.txt?url';\n").unwrap();
        std::fs::write(tmp.path().join("x.txt"), "x").unwrap();
        let entries = vec![zfb_islands::client_scripts::ClientScriptEntry {
            entry_name: "bad".into(),
            source_path: entry,
        }];
        let error = stage_client_script_preprocessing(tmp.path(), &entries).unwrap_err();
        let error = format!("{error:#}");
        assert!(error.contains("scan client-script graph"), "{error}");
        assert!(error.contains("unsupported import query"), "{error}");
    }

    #[test]
    fn materialise_islands_shadow_does_not_walk_unmirrorable_glob_module_subtree() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outside = tempdir().unwrap();
        let outside_glob = outside.path().join("missing-dir/glob.tsx");
        let island_src = write_shadow_fixture(
            project_root,
            "components/gallery.tsx",
            "\"use client\";\nexport function Gallery() { return null; }\n",
        );

        let (islands, scan_meta) =
            basic_shadow_inputs(project_root, vec![outside_glob], vec![island_src]);
        let outcome = materialise_islands_shadow(project_root, &islands, &scan_meta)
            .expect("unmirrorable glob modules should keep the stopgap, not walk their subtree");
        let offenders = match outcome {
            IslandsShadowOutcome::KeepStopgap(o) => o,
            IslandsShadowOutcome::Ready(_) => panic!("outside-root glob must keep stopgap"),
        };
        let message = offenders.join("\n");
        assert!(
            message.contains("outside the mirrorable project tree"),
            "names unmirrorable glob reason: {message}"
        );
    }

    /// Issue #1404 review (prune-predicate divergence): the glob EXPANSION
    /// match-walk and the shadow MIRROR walk must agree on pruning a
    /// top-level `dist`/`target` under the glob module's directory, so the
    /// expander never references a matched target the mirror won't
    /// materialise (which would surface as an esbuild "Could not resolve").
    /// A `./**/*.tsx` glob next to a `dist/` subdir must therefore (a) NOT
    /// reference `./dist/…` in the expanded output AND (b) NOT mirror
    /// `dist/…` into the shadow — while still handling the ordinary
    /// `./widgets/a.tsx` target in BOTH. Pins the two walks in lockstep.
    #[test]
    fn materialise_islands_shadow_prunes_build_output_consistently() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("components/widgets")).unwrap();
        std::fs::create_dir_all(project_root.join("components/dist")).unwrap();
        // Island file (plain) importing the glob data module.
        let island_src = project_root.join("components/gallery.tsx");
        std::fs::write(
            &island_src,
            "\"use client\";\n\
             import { widgets } from \"./gallery-data\";\n\
             export function Gallery() { return null; }\n",
        )
        .unwrap();
        // Glob data module — a recursive `**` pattern that WOULD reach into
        // the sibling `dist/` build-output dir if the match-walk did not
        // prune it.
        std::fs::write(
            project_root.join("components/gallery-data.tsx"),
            "export const widgets = import.meta.glob('./**/*.tsx', { eager: true });\n",
        )
        .unwrap();
        // An ordinary matched target — expected in BOTH the expansion and
        // the shadow.
        std::fs::write(
            project_root.join("components/widgets/a.tsx"),
            "export const a = 1;\n",
        )
        .unwrap();
        // Build output under a top-level `dist/` — must be pruned from BOTH.
        std::fs::write(
            project_root.join("components/dist/generated.tsx"),
            "export const generated = 1;\n",
        )
        .unwrap();

        let islands = vec![zfb_islands::Island::new("Gallery", island_src.clone())];
        let scan_meta = zfb_islands::ScanMeta {
            uses_client_router: false,
            near_miss_candidates: 0,
            glob_reachable_from_islands: vec![project_root.join("components/gallery-data.tsx")],
            island_reachable_modules: vec![
                island_src.clone(),
                project_root.join("components/gallery-data.tsx"),
            ],
            raw_import_edges_from_islands: Vec::new(),
            module_worker_edges_from_islands: Vec::new(),
        };

        let outcome = materialise_islands_shadow(project_root, &islands, &scan_meta)
            .expect("shadow materialisation must not IO-error");
        let shadow = match outcome {
            IslandsShadowOutcome::Ready(s) => s,
            IslandsShadowOutcome::KeepStopgap(o) => {
                panic!("supported glob must NOT keep the stopgap; offenders: {o:?}")
            }
        };
        let shadow_island = shadow
            .remap
            .get(&island_src)
            .expect("island source_path must be remapped into the shadow");
        let shadow_root = shadow_island.parent().unwrap().parent().unwrap();

        let expanded =
            std::fs::read_to_string(shadow_root.join("components/gallery-data.tsx")).unwrap();
        // (a) MATCH-walk: ordinary target referenced, build output NOT.
        assert!(
            expanded.contains("./widgets/a.tsx"),
            "expanded glob must reference the ordinary target: {expanded}"
        );
        assert!(
            !expanded.contains("./dist/"),
            "expanded glob must NOT reference build output under dist/: {expanded}"
        );
        // (b) MIRROR-walk: ordinary target present, build output absent —
        // consistent with the expansion so esbuild can resolve every
        // referenced target and never a pruned one.
        assert!(
            std::fs::symlink_metadata(shadow_root.join("components/widgets/a.tsx")).is_ok(),
            "ordinary glob target must be present in the shadow"
        );
        assert!(
            std::fs::symlink_metadata(shadow_root.join("components/dist/generated.tsx")).is_err(),
            "build output under dist/ must be pruned from the shadow (matched set ⊆ mirrored set)"
        );
    }

    #[test]
    fn materialise_islands_shadow_mirrors_glob_target_transitive_project_imports() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let (island_src, glob_src, target) = write_basic_glob_shadow_project(project_root);
        std::fs::write(
            &target,
            "import { helper } from '../shared/helper';\nexport const a = helper;\n",
        )
        .unwrap();
        write_shadow_fixture(
            project_root,
            "components/shared/helper.ts",
            "export const helper = 1;\n",
        );

        let (islands, scan_meta) = basic_shadow_inputs(
            project_root,
            vec![glob_src.clone()],
            vec![island_src.clone(), glob_src],
        );
        let outcome = materialise_islands_shadow(project_root, &islands, &scan_meta)
            .expect("shadow materialisation must not IO-error");
        let shadow = match outcome {
            IslandsShadowOutcome::Ready(s) => s,
            IslandsShadowOutcome::KeepStopgap(o) => {
                panic!("supported glob target closure must NOT keep stopgap: {o:?}")
            }
        };
        let shadow_island = shadow
            .remap
            .get(&island_src)
            .expect("island source_path must be remapped into the shadow");
        let shadow_root = shadow_island.parent().unwrap().parent().unwrap();
        assert!(
            std::fs::symlink_metadata(shadow_root.join("components/shared/helper.ts")).is_ok(),
            "project-local helper imported only by a glob target must be mirrored"
        );
    }

    #[test]
    fn materialise_islands_shadow_mirrors_glob_target_tsconfig_alias_imports() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["components/*"]}}}"#,
        )
        .unwrap();
        let (island_src, glob_src, target) = write_basic_glob_shadow_project(project_root);
        std::fs::write(
            &target,
            "import { helper } from '@/shared/helper';\nexport const a = helper;\n",
        )
        .unwrap();
        write_shadow_fixture(
            project_root,
            "components/shared/helper.ts",
            "export const helper = 1;\n",
        );

        let (islands, scan_meta) = basic_shadow_inputs(
            project_root,
            vec![glob_src.clone()],
            vec![island_src.clone(), glob_src],
        );
        let outcome = materialise_islands_shadow(project_root, &islands, &scan_meta)
            .expect("shadow materialisation must not IO-error");
        let shadow = match outcome {
            IslandsShadowOutcome::Ready(s) => s,
            IslandsShadowOutcome::KeepStopgap(o) => {
                panic!("supported alias import closure must NOT keep stopgap: {o:?}")
            }
        };
        let shadow_island = shadow
            .remap
            .get(&island_src)
            .expect("island source_path must be remapped into the shadow");
        let shadow_root = shadow_island.parent().unwrap().parent().unwrap();
        assert!(
            std::fs::symlink_metadata(shadow_root.join("components/shared/helper.ts")).is_ok(),
            "tsconfig-aliased helper imported only by a glob target must be mirrored"
        );
    }

    #[test]
    fn materialise_islands_shadow_flags_glob_in_transitive_target_import() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let (island_src, glob_src, target) = write_basic_glob_shadow_project(project_root);
        std::fs::write(
            &target,
            "import { helper } from '../shared/helper';\nexport const a = helper;\n",
        )
        .unwrap();
        write_shadow_fixture(
            project_root,
            "components/shared/helper.ts",
            "export const helper = import.meta.glob('./nested/*.tsx', { eager: true });\n",
        );

        let (islands, scan_meta) = basic_shadow_inputs(
            project_root,
            vec![glob_src.clone()],
            vec![island_src, glob_src],
        );
        let outcome = materialise_islands_shadow(project_root, &islands, &scan_meta)
            .expect("shadow materialisation must not IO-error");
        let offenders = match outcome {
            IslandsShadowOutcome::KeepStopgap(o) => o,
            IslandsShadowOutcome::Ready(_) => {
                panic!("transitive raw-mirrored target glob must keep stopgap")
            }
        };
        let message = offenders.join("\n");
        assert!(
            message.contains("components/shared/helper.ts"),
            "names transitive helper offender: {message}"
        );
        assert!(
            message.contains("raw-mirrored glob target/subtree"),
            "preserves #1412 loudness language: {message}"
        );
    }

    #[test]
    fn materialise_islands_shadow_keeps_symlink_mode_with_project_node_modules_without_paths() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("node_modules")).unwrap();
        let (island_src, glob_src, _target) = write_basic_glob_shadow_project(project_root);

        let (islands, scan_meta) = basic_shadow_inputs(
            project_root,
            vec![glob_src.clone()],
            vec![island_src.clone(), glob_src],
        );
        let outcome = materialise_islands_shadow(project_root, &islands, &scan_meta)
            .expect("shadow materialisation must not IO-error");
        let shadow = match outcome {
            IslandsShadowOutcome::Ready(s) => s,
            IslandsShadowOutcome::KeepStopgap(o) => panic!("supported glob must be ready: {o:?}"),
        };
        let shadow_island = shadow
            .remap
            .get(&island_src)
            .expect("island source_path must be remapped into the shadow");
        assert!(
            shadow.preserve_symlinks,
            "project node_modules without tsconfig paths keeps --preserve-symlinks"
        );
        assert!(
            std::fs::symlink_metadata(shadow_island)
                .unwrap()
                .file_type()
                .is_symlink(),
            "plain source remains symlinked when preserve-symlinks is safe"
        );
    }

    #[test]
    fn materialise_islands_shadow_uses_copy_mode_with_project_node_modules_and_paths() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("node_modules")).unwrap();
        std::fs::write(
            project_root.join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/*"]}}}"#,
        )
        .unwrap();
        let (island_src, glob_src, _target) = write_basic_glob_shadow_project(project_root);

        let (islands, scan_meta) = basic_shadow_inputs(
            project_root,
            vec![glob_src.clone()],
            vec![island_src.clone(), glob_src],
        );
        let outcome = materialise_islands_shadow(project_root, &islands, &scan_meta)
            .expect("shadow materialisation must not IO-error");
        let shadow = match outcome {
            IslandsShadowOutcome::Ready(s) => s,
            IslandsShadowOutcome::KeepStopgap(o) => panic!("supported glob must be ready: {o:?}"),
        };
        let shadow_island = shadow
            .remap
            .get(&island_src)
            .expect("island source_path must be remapped into the shadow");
        let meta = std::fs::symlink_metadata(shadow_island).unwrap();
        assert!(
            !shadow.preserve_symlinks,
            "project node_modules plus tsconfig paths uses copy-mode and omits --preserve-symlinks"
        );
        assert!(
            meta.file_type().is_file() && !meta.file_type().is_symlink(),
            "plain source is a real copied file in copy-mode"
        );
    }

    fn write_shadow_fixture(project_root: &Path, rel: &str, body: &str) -> PathBuf {
        let path = project_root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, body).unwrap();
        path
    }

    fn basic_shadow_inputs(
        project_root: &Path,
        glob_reachable: Vec<PathBuf>,
        island_reachable: Vec<PathBuf>,
    ) -> (Vec<zfb_islands::Island>, zfb_islands::ScanMeta) {
        let island_src = project_root.join("components/gallery.tsx");
        let islands = vec![zfb_islands::Island::new("Gallery", island_src)];
        let scan_meta = zfb_islands::ScanMeta {
            uses_client_router: false,
            near_miss_candidates: 0,
            glob_reachable_from_islands: glob_reachable,
            island_reachable_modules: island_reachable,
            raw_import_edges_from_islands: Vec::new(),
            module_worker_edges_from_islands: Vec::new(),
        };
        (islands, scan_meta)
    }

    fn write_basic_glob_shadow_project(project_root: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let island_src = write_shadow_fixture(
            project_root,
            "components/gallery.tsx",
            "\"use client\";\n\
             import { widgets } from \"./gallery-data\";\n\
             export function Gallery() { return null; }\n",
        );
        let glob_src = write_shadow_fixture(
            project_root,
            "components/gallery-data.tsx",
            "export const widgets = import.meta.glob('./widgets/*.tsx', { eager: true });\n",
        );
        let target = write_shadow_fixture(
            project_root,
            "components/widgets/a.tsx",
            "export const a = 1;\n",
        );
        (island_src, glob_src, target)
    }

    #[test]
    fn materialise_islands_shadow_flags_nested_glob_in_target_file() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let (island_src, glob_src, target) = write_basic_glob_shadow_project(project_root);
        std::fs::write(
            &target,
            "export const nested = import.meta.glob('./nested/*.tsx', { eager: true });\n",
        )
        .unwrap();

        let (islands, scan_meta) = basic_shadow_inputs(
            project_root,
            vec![glob_src.clone()],
            vec![island_src, glob_src],
        );
        let outcome = materialise_islands_shadow(project_root, &islands, &scan_meta)
            .expect("shadow materialisation must not IO-error");
        let offenders = match outcome {
            IslandsShadowOutcome::KeepStopgap(o) => o,
            IslandsShadowOutcome::Ready(_) => panic!("nested target glob must keep stopgap"),
        };
        let message = offenders.join("\n");
        assert!(message.contains("widgets/a.tsx"), "names target: {message}");
        assert!(
            message.contains("raw-mirrored glob target/subtree"),
            "explains raw mirror location: {message}"
        );
        assert!(
            message.contains("Hoist the glob") && message.contains("explicit static imports"),
            "gives remediation: {message}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn materialise_islands_shadow_accepts_canonical_paths_under_symlinked_root() {
        let real = tempdir().unwrap();
        let link_parent = tempdir().unwrap();
        let project_root = link_parent.path().join("project-link");
        std::os::unix::fs::symlink(real.path(), &project_root).unwrap();

        let (island_src, glob_src, target) = write_basic_glob_shadow_project(&project_root);
        std::fs::write(
            &target,
            "export const nested = import.meta.glob('./nested/*.tsx', { eager: true });\n",
        )
        .unwrap();

        let island_src = island_src.canonicalize().unwrap();
        let glob_src = glob_src.canonicalize().unwrap();
        let islands = vec![zfb_islands::Island::new("Gallery", island_src.clone())];
        let scan_meta = zfb_islands::ScanMeta {
            uses_client_router: false,
            near_miss_candidates: 0,
            glob_reachable_from_islands: vec![glob_src.clone()],
            island_reachable_modules: vec![island_src, glob_src],
            raw_import_edges_from_islands: Vec::new(),
            module_worker_edges_from_islands: Vec::new(),
        };

        let outcome = materialise_islands_shadow(&project_root, &islands, &scan_meta)
            .expect("shadow materialisation must not IO-error");
        let offenders = match outcome {
            IslandsShadowOutcome::KeepStopgap(o) => o,
            IslandsShadowOutcome::Ready(_) => panic!("nested target glob must keep stopgap"),
        };
        let message = offenders.join("\n");
        assert!(
            !message.contains("outside the mirrorable project tree"),
            "does not misclassify canonical in-tree glob module: {message}"
        );
        assert!(
            message.contains("widgets/a.tsx"),
            "names nested raw-mirrored offender: {message}"
        );
    }

    fn write_nested_glob_build_project(project_root: &Path, nested_body: &str) -> PathBuf {
        write_shadow_fixture(
            project_root,
            "pages/index.tsx",
            "import { Gallery } from \"../components/gallery\";\n\
             export default function Index() { return <Gallery/>; }\n",
        );
        write_shadow_fixture(
            project_root,
            "components/gallery.tsx",
            "\"use client\";\n\
             import { widgets } from \"./gallery-data\";\n\
             export function Gallery() { return null; }\n",
        );
        write_shadow_fixture(
            project_root,
            "components/gallery-data.tsx",
            "export const widgets = import.meta.glob('./widgets/*.tsx', { eager: true });\n",
        );
        write_shadow_fixture(project_root, "components/widgets/a.tsx", nested_body)
    }

    #[test]
    fn build_default_islands_payload_hard_errors_on_nested_raw_mirrored_glob() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        write_nested_glob_build_project(
            project_root,
            "export const nested = import.meta.glob('./nested/*.tsx', { eager: true });\n",
        );

        let err = build_default_islands_payload(
            project_root,
            &project_root.join("pages"),
            &[],
            &project_root.join("dist"),
            crate::config::Framework::Preact,
            &IslandsPluginConfig::default(),
            IslandsGlobPolicy::HardError,
            None,
        )
        .expect_err("nested raw-mirrored glob must hard-error in build");
        let message = format!("{err:#}");
        assert!(
            message.contains("import.meta.glob"),
            "names glob: {message}"
        );
        assert!(
            message.contains("widgets/a.tsx"),
            "names offender: {message}"
        );
        assert!(message.contains("1385"), "links #1385: {message}");
        assert!(message.contains("1412"), "links #1412: {message}");
    }

    #[test]
    fn build_default_islands_payload_warn_and_skip_on_nested_raw_mirrored_glob() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        write_nested_glob_build_project(
            project_root,
            "export const nested = import.meta.glob('./nested/*.tsx', { eager: true });\n",
        );

        let (payload, names) = build_default_islands_payload(
            project_root,
            &project_root.join("pages"),
            &[],
            &project_root.join("dist"),
            crate::config::Framework::Preact,
            &IslandsPluginConfig::default(),
            IslandsGlobPolicy::WarnAndSkip,
            None,
        )
        .expect("warn-and-skip must not hard-error");
        assert!(payload.is_none(), "warn-and-skip returns no payload");
        assert!(names.is_empty(), "warn-and-skip returns empty marker set");
    }

    #[test]
    fn materialise_islands_shadow_flags_lazy_nested_glob_in_target_file() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let (island_src, glob_src, target) = write_basic_glob_shadow_project(project_root);
        std::fs::write(
            &target,
            "export const nested = import.meta.glob('./nested/*.tsx');\n",
        )
        .unwrap();

        let (islands, scan_meta) = basic_shadow_inputs(
            project_root,
            vec![glob_src.clone()],
            vec![island_src, glob_src],
        );
        let outcome = materialise_islands_shadow(project_root, &islands, &scan_meta)
            .expect("shadow materialisation must not IO-error");
        let offenders = match outcome {
            IslandsShadowOutcome::KeepStopgap(o) => o,
            IslandsShadowOutcome::Ready(_) => panic!("lazy nested target glob must keep stopgap"),
        };
        let message = offenders.join("\n");
        assert!(message.contains("widgets/a.tsx"), "names target: {message}");
        assert!(
            message.contains("import.meta.glob"),
            "names glob: {message}"
        );
    }

    #[test]
    fn materialise_islands_shadow_ignores_string_and_comment_only_in_target_file() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let (island_src, glob_src, target) = write_basic_glob_shadow_project(project_root);
        std::fs::write(
            &target,
            "// import.meta.glob('./comment.tsx', { eager: true })\n\
             const doc = \"import.meta.glob('./string.tsx')\";\n",
        )
        .unwrap();

        let (islands, scan_meta) = basic_shadow_inputs(
            project_root,
            vec![glob_src.clone()],
            vec![island_src, glob_src],
        );
        let outcome = materialise_islands_shadow(project_root, &islands, &scan_meta)
            .expect("shadow materialisation must not IO-error");
        match outcome {
            IslandsShadowOutcome::Ready(_) => {}
            IslandsShadowOutcome::KeepStopgap(o) => {
                panic!("string/comment-only target occurrences must not flag: {o:?}")
            }
        }
    }

    #[test]
    fn materialise_islands_shadow_allows_benign_glob_subtree() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let (island_src, glob_src, _target) = write_basic_glob_shadow_project(project_root);
        write_shadow_fixture(
            project_root,
            "components/widgets/readme.md",
            "import.meta.glob('./not-real.tsx')\n",
        );

        let (islands, scan_meta) = basic_shadow_inputs(
            project_root,
            vec![glob_src.clone()],
            vec![island_src, glob_src],
        );
        let outcome = materialise_islands_shadow(project_root, &islands, &scan_meta)
            .expect("shadow materialisation must not IO-error");
        match outcome {
            IslandsShadowOutcome::Ready(_) => {}
            IslandsShadowOutcome::KeepStopgap(o) => panic!("benign subtree must be ready: {o:?}"),
        }
    }

    #[test]
    fn materialise_islands_shadow_treats_parse_error_as_nested_glob_offender() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let (island_src, glob_src, target) = write_basic_glob_shadow_project(project_root);
        std::fs::write(
            &target,
            "export const nested = import.meta.glob('./nested/*.tsx', { eager: true ",
        )
        .unwrap();

        let (islands, scan_meta) = basic_shadow_inputs(
            project_root,
            vec![glob_src.clone()],
            vec![island_src, glob_src],
        );
        let outcome = materialise_islands_shadow(project_root, &islands, &scan_meta)
            .expect("shadow materialisation must not IO-error");
        let offenders = match outcome {
            IslandsShadowOutcome::KeepStopgap(o) => o,
            IslandsShadowOutcome::Ready(_) => panic!("parse-error target glob must keep stopgap"),
        };
        let message = offenders.join("\n");
        assert!(message.contains("widgets/a.tsx"), "names target: {message}");
        assert!(
            message.contains("could not be parsed"),
            "surfaces conservative parse handling: {message}"
        );
    }

    #[test]
    fn materialise_islands_shadow_js_like_extension_gate_batches_offenders() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let (island_src, glob_src, _target) = write_basic_glob_shadow_project(project_root);
        write_shadow_fixture(
            project_root,
            "components/widgets/module.mts",
            "export const nested = import.meta.glob('./nested/*.tsx', { eager: true });\n",
        );
        write_shadow_fixture(
            project_root,
            "components/widgets/common.cts",
            "export const nested = import.meta.glob('./nested/*.tsx', { eager: true });\n",
        );
        write_shadow_fixture(
            project_root,
            "components/widgets/Upper.TSX",
            "export const nested = import.meta.glob('./nested/*.tsx', { eager: true });\n",
        );
        write_shadow_fixture(
            project_root,
            "components/widgets/notes.md",
            "export const nested = import.meta.glob('./nested/*.tsx', { eager: true });\n",
        );

        let (islands, scan_meta) = basic_shadow_inputs(
            project_root,
            vec![glob_src.clone()],
            vec![island_src, glob_src],
        );
        let outcome = materialise_islands_shadow(project_root, &islands, &scan_meta)
            .expect("shadow materialisation must not IO-error");
        let offenders = match outcome {
            IslandsShadowOutcome::KeepStopgap(o) => o,
            IslandsShadowOutcome::Ready(_) => panic!("JS-like nested globs must keep stopgap"),
        };
        let message = offenders.join("\n");
        assert_eq!(
            offenders.len(),
            3,
            "batches only JS-like offenders: {message}"
        );
        assert!(message.contains("module.mts"), "flags .mts: {message}");
        assert!(message.contains("common.cts"), "flags .cts: {message}");
        assert!(
            message.contains("Upper.TSX"),
            "flags uppercase ext: {message}"
        );
        assert!(!message.contains("notes.md"), "skips .md: {message}");
    }

    #[test]
    fn materialise_islands_shadow_flags_unused_js_like_sibling_in_glob_subtree() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let (island_src, glob_src, _target) = write_basic_glob_shadow_project(project_root);
        write_shadow_fixture(
            project_root,
            "components/unused.tsx",
            "export const unused = import.meta.glob('./unused/*.tsx', { eager: true });\n",
        );

        let (islands, scan_meta) = basic_shadow_inputs(
            project_root,
            vec![glob_src.clone()],
            vec![island_src, glob_src],
        );
        let outcome = materialise_islands_shadow(project_root, &islands, &scan_meta)
            .expect("shadow materialisation must not IO-error");
        let offenders = match outcome {
            IslandsShadowOutcome::KeepStopgap(o) => o,
            IslandsShadowOutcome::Ready(_) => panic!("unused sibling glob must keep stopgap"),
        };
        let message = offenders.join("\n");
        assert!(
            message.contains("components/unused.tsx"),
            "conservative sibling flag names file: {message}"
        );
    }

    #[test]
    fn materialise_islands_shadow_does_not_flag_glob_target_that_is_also_island_reachable() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let (island_src, glob_src, target) = write_basic_glob_shadow_project(project_root);
        std::fs::create_dir_all(project_root.join("components/widgets/nested")).unwrap();
        std::fs::write(
            &target,
            "export const nested = import.meta.glob('./nested/*.tsx', { eager: true });\n",
        )
        .unwrap();
        write_shadow_fixture(
            project_root,
            "components/widgets/nested/leaf.tsx",
            "export const leaf = 1;\n",
        );

        let (islands, scan_meta) = basic_shadow_inputs(
            project_root,
            vec![glob_src.clone(), target.clone()],
            vec![island_src.clone(), glob_src, target.clone()],
        );
        let outcome = materialise_islands_shadow(project_root, &islands, &scan_meta)
            .expect("shadow materialisation must not IO-error");
        let shadow = match outcome {
            IslandsShadowOutcome::Ready(s) => s,
            IslandsShadowOutcome::KeepStopgap(o) => {
                panic!("island-reachable target glob must be expanded, not flagged: {o:?}")
            }
        };
        let shadow_island = shadow
            .remap
            .get(&island_src)
            .expect("island source_path must be remapped into the shadow");
        let shadow_root = shadow_island.parent().unwrap().parent().unwrap();
        let expanded =
            std::fs::read_to_string(shadow_root.join("components/widgets/a.tsx")).unwrap();
        assert!(
            !expanded.contains("import.meta.glob("),
            "also-island-reachable target must be expanded: {expanded}"
        );
        assert!(
            expanded.contains("./nested/leaf.tsx"),
            "expanded target glob references its match: {expanded}"
        );
    }

    // Zero-regression coverage for "no glob anywhere" is already provided by
    // `default_runner_returns_none_islands_when_no_pages_dir`,
    // `_when_no_use_client_components`, and `_for_near_miss_directive`
    // above — all three now thread `IslandsGlobPolicy::HardError` and still
    // assert `.expect("should not error")`. A companion test using a REAL
    // (non-empty) island set here would exercise the real esbuild
    // subprocess via `build_production_islands_asset`, which none of this
    // module's other non-`#[ignore]`d tests do — see the doc comments on
    // the three tests above ("esbuild is never invoked").

    /// End-to-end check that `DefaultRunner::emit_prod_assets`
    /// invokes the real Tailwind v4 CLI and returns non-empty CSS
    /// bytes for a fixture project with a single page. Mirrors the
    /// `#[ignore]` gate already used by
    /// `crates/zfb-css/tests/integration.rs::subprocess_engine_against_real_binary`
    /// — both depend on the Tailwind binary slot at
    /// `crates/zfb/binaries/tailwindcss-v4`, which `crates/zfb/build.rs`
    /// DOES stage in CI as a side effect of building the `zfb` crate, but
    /// no CI step runs with `--ignored`/`--include-ignored` yet. Run
    /// locally with `--include-ignored` once a build has staged the slot.
    // Requires `DefaultRunner` which carries `PluginRegistryHooks` and
    // constructs `Backend::EmbeddedV8` — only available when the
    // `embed_v8` feature is on (issue #371, sub-task 4.1a).
    #[cfg(feature = "embed_v8")]
    #[test]
    #[ignore = "env-gate: tailwindcss v4 binary — cargo test -p zfb --lib \
                commands::build:: -- --include-ignored (ZFB_TAILWIND_BIN or the \
                staged crates/zfb/binaries/tailwindcss-v4 slot)"]
    fn default_runner_emit_prod_assets_returns_non_empty_css_for_real_project() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("pages")).unwrap();
        std::fs::write(
            project_root.join("pages/index.tsx"),
            "export default function Index() { return <div className=\"text-red-500\">hi</div>; }\n",
        )
        .unwrap();
        let cfg = Config::default(); // tailwind defaults to enabled
        let outdir = project_root.join("dist");
        let runner = DefaultRunner {
            islands_plugin_config: IslandsPluginConfig::default(),
            v8_plugin_hooks: zfb_render::PluginRegistryHooks::default(),
            registered_client_entries: zfb_build::ClientEntryList::new(),
        };
        let (inputs, _marker_names) = runner
            .emit_prod_assets(
                project_root,
                &project_root.join("pages"),
                &[],
                &outdir,
                &cfg,
            )
            .expect("emit_prod_assets must succeed");
        let css = inputs
            .css
            .expect("css slot must be Some when tailwind is enabled and a page exists");
        assert!(
            !css.bytes.is_empty(),
            "CSS bytes must be non-empty when Tailwind ran against a TSX source"
        );
        assert_eq!(css.stable_url, "/assets/styles.css");
        // The pipeline writes its hashed asset elsewhere, but the
        // bytes-only `build_emitter` path must NOT have written the
        // stable filename to disk on its own — that's the prod
        // pipeline's job after hashing.
        let stable_on_disk = outdir.join("assets").join("styles.css");
        assert!(
            !stable_on_disk.exists(),
            "build_emitter must not write the stable-name file; found {}",
            stable_on_disk.display()
        );
    }

    // `end_to_end_basic_blog_build` moved out of this lib unit-test module —
    // `CARGO_BIN_EXE_zfb` / `zfb_binary!()` is only set for integration
    // tests, so a lib unit test here could never spawn the real binary. The
    // real test now lives at `crates/zfb/tests/end_to_end_basic_blog_build.rs`
    // (issue #1361, Test B of #1354).

    // -------------------------------------------------------------------------
    // copy_public_dir unit tests
    // -------------------------------------------------------------------------

    /// Missing public/ directory is silently ignored (no error).
    #[test]
    fn copy_public_dir_missing_source_is_noop() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        std::fs::create_dir_all(&outdir).unwrap();
        // No public/ staged — must succeed without creating anything.
        copy_public_dir(project_root, &outdir, std::path::Path::new("public"), None)
            .expect("missing public/ must not error");
        // dist/ contains nothing (no phantom files).
        let entries: Vec<_> = std::fs::read_dir(&outdir).unwrap().collect();
        assert!(
            entries.is_empty(),
            "dist/ must stay empty when public/ is absent"
        );
    }

    /// Without base, files copy directly under out_dir.
    #[test]
    fn copy_public_dir_no_base_copies_under_outdir() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        std::fs::create_dir_all(project_root.join("public/img")).unwrap();
        std::fs::write(project_root.join("public/img/logo.svg"), b"<svg/>").unwrap();
        copy_public_dir(project_root, &outdir, std::path::Path::new("public"), None)
            .expect("copy must succeed");
        let dest = outdir.join("img/logo.svg");
        assert!(
            dest.is_file(),
            "public/img/logo.svg must land at dist/img/logo.svg"
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"<svg/>");
    }

    /// With base = "/pj/test/", files land under out_dir/pj/test/.
    #[test]
    fn copy_public_dir_with_base_copies_under_base_segment() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        std::fs::create_dir_all(project_root.join("public/img")).unwrap();
        let logo_content = b"<svg id=\"logo\"/>";
        std::fs::write(project_root.join("public/img/logo.svg"), logo_content).unwrap();
        copy_public_dir(
            project_root,
            &outdir,
            std::path::Path::new("public"),
            Some("/pj/test/"),
        )
        .expect("copy must succeed");
        // Files land under the base segment.
        let dest = outdir.join("pj/test/img/logo.svg");
        assert!(
            dest.is_file(),
            "public/img/logo.svg must land at dist/pj/test/img/logo.svg; path: {}",
            dest.display()
        );
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            logo_content,
            "file content must be preserved",
        );
    }

    /// Base with no trailing slash is normalised identically.
    #[test]
    fn copy_public_dir_base_without_trailing_slash() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        std::fs::create_dir_all(project_root.join("public")).unwrap();
        std::fs::write(project_root.join("public/favicon.ico"), b"\x00").unwrap();
        copy_public_dir(
            project_root,
            &outdir,
            std::path::Path::new("public"),
            Some("/pj/test"),
        )
        .expect("copy must succeed");
        assert!(outdir.join("pj/test/favicon.ico").is_file());
    }

    /// Absolute-URL base (CDN) has no on-disk sub-path — files copy
    /// directly under outdir instead of a literal "https:/…" directory.
    #[test]
    fn copy_public_dir_absolute_url_base_copies_under_outdir() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        std::fs::create_dir_all(project_root.join("public")).unwrap();
        std::fs::write(project_root.join("public/favicon.ico"), b"\x00").unwrap();
        copy_public_dir(
            project_root,
            &outdir,
            std::path::Path::new("public"),
            Some("https://cdn.example.com/"),
        )
        .expect("copy must succeed");
        assert!(
            outdir.join("favicon.ico").is_file(),
            "absolute-URL base must not create an on-disk URL-shaped path"
        );
        assert!(
            !outdir.join("https:").exists(),
            "no literal 'https:' directory must be created"
        );
    }

    /// Route-vs-static collision: if the renderer already created
    /// `dest` as a directory (e.g. `dist/foo/`), `copy_public_dir` must
    /// skip the conflicting `public/foo` file rather than crashing with
    /// EISDIR — rendered page wins over public file (issue #1167).
    #[test]
    fn copy_public_dir_skips_file_when_dest_is_directory() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        // Stage public/foo (plain file) — same name as a rendered route.
        std::fs::create_dir_all(project_root.join("public")).unwrap();
        std::fs::write(project_root.join("public/foo"), b"public-foo-content").unwrap();
        // Pre-create dist/foo/ as a directory (what the renderer would write
        // for a pages/foo.tsx route that emits dist/foo/index.html).
        std::fs::create_dir_all(outdir.join("foo")).unwrap();
        std::fs::write(outdir.join("foo/index.html"), b"<h1>rendered foo</h1>").unwrap();
        // The copy must succeed without crashing (EISDIR).
        copy_public_dir(project_root, &outdir, std::path::Path::new("public"), None)
            .expect("copy must succeed even when dest is a rendered-route directory");
        // The rendered index.html must be untouched.
        let index = std::fs::read(outdir.join("foo/index.html")).unwrap();
        assert_eq!(
            index, b"<h1>rendered foo</h1>",
            "dist/foo/index.html must not be overwritten by public/foo",
        );
        // The public/foo raw content must NOT appear as a flat file at dist/foo.
        // dist/foo is still a directory — verifying it was not replaced.
        assert!(
            outdir.join("foo").is_dir(),
            "dist/foo must remain a directory after copy_public_dir",
        );
    }

    /// Base = "/" is treated as no prefix.
    #[test]
    fn copy_public_dir_base_root_slash_is_noop_prefix() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        std::fs::create_dir_all(project_root.join("public")).unwrap();
        std::fs::write(project_root.join("public/favicon.ico"), b"\x00").unwrap();
        copy_public_dir(
            project_root,
            &outdir,
            std::path::Path::new("public"),
            Some("/"),
        )
        .expect("copy must succeed");
        assert!(
            outdir.join("favicon.ico").is_file(),
            "base='/' must copy under root, not under '/'"
        );
    }

    /// With base = "/pj/test/" and copy_public_with_base = false, files
    /// land flat under out_dir — NOT under the base segment.
    /// Fixture: issue #932 acceptance criterion (false branch).
    #[test]
    fn run_build_copy_public_with_base_false_copies_flat() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);

        // Stage public/img/logo.svg.
        std::fs::create_dir_all(project_root.join("public/img")).unwrap();
        let logo_content = b"<svg id=\"flat-logo\"/>";
        std::fs::write(project_root.join("public/img/logo.svg"), logo_content).unwrap();

        let routes = vec![static_route(vec![], "pages/index.tsx")];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"));
        let cfg = Config {
            base: Some("/pj/test/".to_string()),
            copy_public_with_base: false,
            ..Config::default()
        };
        let fake_adapter = FakeAdapterRunner::new();

        run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap();

        // Acceptance: file must land flat at dist/img/logo.svg, NOT
        // under the base segment (dist/pj/test/img/logo.svg).
        let flat_dest = outdir.join("img/logo.svg");
        assert!(
            flat_dest.is_file(),
            "copyPublicWithBase:false — public/img/logo.svg must land at dist/img/logo.svg; \
             not found at {}",
            flat_dest.display()
        );
        assert_eq!(
            std::fs::read(&flat_dest).unwrap(),
            logo_content,
            "file content must be preserved",
        );
        let nested_dest = outdir.join("pj/test/img/logo.svg");
        assert!(
            !nested_dest.is_file(),
            "copyPublicWithBase:false — file must NOT appear under base segment at {}",
            nested_dest.display()
        );
    }

    /// Full run_build integration: public/img/logo.svg + base = "/pj/test/"
    /// results in dist/pj/test/img/logo.svg with correct content.
    /// Fixture: issue #192 acceptance criterion.
    #[test]
    fn run_build_copies_public_dir_under_base_segment() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);

        // Stage public/img/logo.svg.
        std::fs::create_dir_all(project_root.join("public/img")).unwrap();
        let logo_content = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><text>logo</text></svg>";
        std::fs::write(project_root.join("public/img/logo.svg"), logo_content).unwrap();

        let routes = vec![static_route(vec![], "pages/index.tsx")];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"));
        let cfg = Config {
            base: Some("/pj/test/".to_string()),
            ..Config::default()
        };
        let fake_adapter = FakeAdapterRunner::new();

        run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap();

        // Acceptance: file must exist at out_dir/pj/test/img/logo.svg.
        let dest = outdir.join("pj/test/img/logo.svg");
        assert!(
            dest.is_file(),
            "public/img/logo.svg must be copied to dist/pj/test/img/logo.svg; not found at {}",
            dest.display()
        );
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            logo_content,
            "file content must match the source",
        );
    }

    // ---------------------------------------------------------------
    // #347 — on-disk routes.json manifest emission tests.
    //
    // The helper writes the same data that postBuild plugins see via
    // `ctx.routes`, just at `<outdir>/__zfb/routes.json` instead of
    // through a JS callback. These tests pin three things the issue
    // body called out explicitly:
    //
    //   1. shape (field set, params shape, prerender boolean),
    //   2. byte-stability across runs (#262 carry-over AC),
    //   3. SSG / SSR routes both land with the correct `prerender` flag.
    //
    // The feature-flag wiring (default-on, opt-out via
    // `emitRoutesManifest: false`) is exercised by Config::default()
    // returning `emit_routes_manifest: None` — a missing field must
    // emit, matching the documented default.
    // ---------------------------------------------------------------

    fn sample_post_build_manifest() -> zfb_build::PostBuildRouteManifest {
        use std::collections::BTreeMap;
        use zfb_build::{PostBuildParamValue, PostBuildRouteEntry, PostBuildRouteManifest};

        let mut params = BTreeMap::new();
        params.insert("slug".into(), PostBuildParamValue::Scalar("hello".into()));

        PostBuildRouteManifest {
            routes: vec![
                PostBuildRouteEntry {
                    url: "/".into(),
                    output: "index.html".into(),
                    extension: "html".into(),
                    source: "pages/index.tsx".into(),
                    prerender: true,
                    params: None,
                },
                PostBuildRouteEntry {
                    url: "/api/me".into(),
                    output: "api/me/index.html".into(),
                    extension: "html".into(),
                    source: "pages/api/me.tsx".into(),
                    prerender: false,
                    params: None,
                },
                PostBuildRouteEntry {
                    url: "/blog/hello/".into(),
                    output: "blog/hello/index.html".into(),
                    extension: "html".into(),
                    source: "pages/blog/[slug].tsx".into(),
                    prerender: true,
                    params: Some(params),
                },
            ],
        }
    }

    /// Schema: every documented field must round-trip through the
    /// on-disk JSON unchanged. `prerender` must be a boolean (NOT a
    /// stringified value), `params` must be omitted for static routes
    /// and present for dynamic ones, and the file must live exactly at
    /// `<outdir>/__zfb/routes.json`.
    #[test]
    fn emit_routes_manifest_writes_documented_schema() {
        let tmp = tempdir().unwrap();
        let outdir = tmp.path();
        let manifest = sample_post_build_manifest();

        emit_routes_manifest_file(outdir, &manifest).unwrap();

        let dest = outdir.join("__zfb/routes.json");
        assert!(
            dest.is_file(),
            "routes.json must live at <outdir>/__zfb/routes.json, not at {}",
            dest.display()
        );

        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&dest).unwrap()).unwrap();
        let routes = parsed["routes"].as_array().expect("routes is an array");
        assert_eq!(routes.len(), 3);

        // Field set on a static SSG route.
        let root = &routes[0];
        assert_eq!(root["url"], "/");
        assert_eq!(root["output"], "index.html");
        assert_eq!(root["extension"], "html");
        assert_eq!(root["source"], "pages/index.tsx");
        assert_eq!(root["prerender"], serde_json::json!(true));
        assert!(
            root.get("params").is_none(),
            "static route must omit params, got: {root}",
        );

        // SSR route must surface prerender:false verbatim.
        let ssr = &routes[1];
        assert_eq!(ssr["url"], "/api/me");
        assert_eq!(ssr["prerender"], serde_json::json!(false));

        // Dynamic route must carry params as a string map.
        let blog = &routes[2];
        assert_eq!(blog["url"], "/blog/hello/");
        assert_eq!(blog["params"]["slug"], "hello");
    }

    /// Byte-stability: emitting the same manifest twice must produce
    /// identical bytes — the same guarantee #262 made for the
    /// in-memory manifest. Anything that breaks this (a timestamp, a
    /// build-id field, non-deterministic JSON serialisation) would
    /// regress consumer scripts that diff the file across runs.
    #[test]
    fn emit_routes_manifest_is_byte_stable_across_runs() {
        let tmp = tempdir().unwrap();
        let manifest = sample_post_build_manifest();

        let outdir_a = tmp.path().join("a");
        let outdir_b = tmp.path().join("b");
        emit_routes_manifest_file(&outdir_a, &manifest).unwrap();
        emit_routes_manifest_file(&outdir_b, &manifest).unwrap();

        let a = std::fs::read(outdir_a.join("__zfb/routes.json")).unwrap();
        let b = std::fs::read(outdir_b.join("__zfb/routes.json")).unwrap();
        assert_eq!(
            a, b,
            "routes.json must be byte-stable across runs (mirrors #262)",
        );

        // Trailing newline so the file is well-behaved under POSIX
        // line-oriented tools and CI diff viewers.
        assert!(a.ends_with(b"\n"));
    }

    /// SSG-vs-SSR pinning: the in-memory `prerender_map` is the source
    /// of truth, and the emit helper must surface both shapes without
    /// dropping or coercing entries. Mirrors
    /// `post_build_route_manifest_preserves_prerender_field` over in
    /// `plugin_runner.rs`, but pins the on-disk surface.
    #[test]
    fn emit_routes_manifest_preserves_ssg_and_ssr_entries() {
        let tmp = tempdir().unwrap();
        let outdir = tmp.path();
        emit_routes_manifest_file(outdir, &sample_post_build_manifest()).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(outdir.join("__zfb/routes.json")).unwrap(),
        )
        .unwrap();
        let routes = parsed["routes"].as_array().unwrap();
        let by_url: std::collections::HashMap<&str, &serde_json::Value> = routes
            .iter()
            .map(|r| (r["url"].as_str().unwrap(), r))
            .collect();
        assert_eq!(by_url["/"]["prerender"], serde_json::json!(true));
        assert_eq!(by_url["/api/me"]["prerender"], serde_json::json!(false));
        assert_eq!(by_url["/blog/hello/"]["prerender"], serde_json::json!(true));
    }

    // --- resolve_v8_mode (sub-task 4.1b / issue #373) ----------------------
    //
    // The decision tree is the load-bearing surface for the V8-mode gate.
    // Tests target the pure function so the four scenarios from the
    // sub-issue's acceptance criteria are exercised without spawning a
    // bundler or V8 host.

    fn ssr_route(key: &'static str, url: &'static str) -> SsrRouteRef<'static> {
        SsrRouteRef {
            route_key: key,
            url_path: url,
        }
    }

    /// AC: default `output: "auto"` on a pure-SSG project resolves to
    /// V8-off (no SSR routes → no V8 needed in the future-runtime sense).
    #[test]
    fn resolve_v8_mode_auto_on_pure_ssg_is_off() {
        let mode = resolve_v8_mode(OutputMode::Auto, &[]).expect("auto + no SSR routes resolves");
        assert_eq!(mode, V8Mode::Off);
    }

    /// AC: default `output: "auto"` with `prerender = false` routes
    /// resolves to V8-on — detection-driven.
    #[test]
    fn resolve_v8_mode_auto_with_ssr_routes_is_on() {
        let routes = [ssr_route("/api/me", "/api/me")];
        let mode = resolve_v8_mode(OutputMode::Auto, &routes).expect("auto + SSR route resolves");
        assert_eq!(mode, V8Mode::On);
    }

    /// AC: explicit `output: "hybrid"` on a pure-SSG project forces
    /// V8-on regardless of detection.
    #[test]
    fn resolve_v8_mode_hybrid_on_pure_ssg_forces_on() {
        let mode =
            resolve_v8_mode(OutputMode::Hybrid, &[]).expect("hybrid + no SSR routes resolves");
        assert_eq!(mode, V8Mode::On);
        // Mirror with a project that already has SSR routes — same
        // result, just confirming hybrid is "always on" not "on when
        // SSR routes happen to be present".
        let routes = [ssr_route("/api/me", "/api/me")];
        let mode =
            resolve_v8_mode(OutputMode::Hybrid, &routes).expect("hybrid + SSR route resolves");
        assert_eq!(mode, V8Mode::On);
    }

    /// Explicit `output: "static"` on a pure-SSG project resolves
    /// cleanly to V8-off. Mirror of the auto+SSG path; the difference
    /// from auto is that the user has declared intent.
    #[test]
    fn resolve_v8_mode_static_on_pure_ssg_is_off() {
        let mode =
            resolve_v8_mode(OutputMode::Static, &[]).expect("static + no SSR routes resolves");
        assert_eq!(mode, V8Mode::Off);
    }

    /// AC: explicit `output: "static"` on a project with `prerender =
    /// false` routes errors with a clear message naming both the
    /// config setting and the offending route.
    #[test]
    fn resolve_v8_mode_static_with_ssr_routes_errors() {
        let routes = [ssr_route("/api/me", "/api/me")];
        let err = resolve_v8_mode(OutputMode::Static, &routes)
            .expect_err("static + SSR route must error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("output: \"static\""),
            "error must name the config setting; got: {msg}"
        );
        assert!(
            msg.contains("/api/me"),
            "error must name the offending route; got: {msg}"
        );
        assert!(
            msg.contains("prerender = false"),
            "error must name the route-side knob; got: {msg}"
        );
    }

    /// The error message should mention the "and N more" suffix when
    /// multiple SSR routes are present — mirrors the
    /// `ensure_no_ssr_without_adapter` shape so the user knows fixing
    /// just one route won't be enough.
    #[test]
    fn resolve_v8_mode_static_error_counts_extra_routes() {
        let routes = [
            ssr_route("/api/me", "/api/me"),
            ssr_route("/api/sessions", "/api/sessions"),
            ssr_route("/api/health", "/api/health"),
        ];
        let err = resolve_v8_mode(OutputMode::Static, &routes)
            .expect_err("static + multiple SSR routes must error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("(and 2 more)"),
            "multi-route message should count the extras; got: {msg}"
        );
    }

    /// #1198 regression: a page with a lone `export const prerender = false`
    /// and NO `export const frontmatter` must be caught by the `output:
    /// static` gate, not silently shipped as SSG. Exercises the real chain —
    /// `build_prerender_map` → `is_ssr_route` → `resolve_v8_mode(Static)` —
    /// so a regression in any link re-opens the safety hole. No V8 boot.
    #[test]
    fn output_static_rejects_frontmatterless_prerender_false_page() {
        let dir = tempdir().unwrap();
        let pages = dir.path().join("pages");
        std::fs::create_dir_all(&pages).unwrap();
        // Lone `prerender = false`, no `frontmatter` — the exact shape #1198
        // used to let slip through as SSG.
        std::fs::write(
            pages.join("ssr.tsx"),
            "export const prerender = false;\nexport default function() { return null; }\n",
        )
        .unwrap();

        let routes = vec![static_route(vec!["ssr"], "pages/ssr.tsx")];
        let map = build_prerender_map(&routes, dir.path(), |_| {});

        // The frontmatter-less SSR page must now register as SSR (the bug was
        // it registered as SSG and never reached the gate).
        let templates: Vec<String> = routes.iter().map(|r| r.template()).collect();
        let ssr_refs: Vec<SsrRouteRef<'_>> = templates
            .iter()
            .filter(|t| is_ssr_route(&map, t))
            .map(|t| SsrRouteRef {
                route_key: t,
                url_path: t,
            })
            .collect();
        assert_eq!(
            ssr_refs.len(),
            1,
            "frontmatter-less `prerender = false` page must be detected as SSR"
        );

        let err = resolve_v8_mode(OutputMode::Static, &ssr_refs)
            .expect_err("output: static must reject the frontmatter-less SSR page");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("output: \"static\""),
            "error must name the config setting; got: {msg}"
        );
        assert!(
            msg.contains("/ssr"),
            "error must name the offending route; got: {msg}"
        );
    }

    /// Codex review (4.1b PR) flagged a missing case: a dynamic route
    /// with `prerender = false` AND a non-literal `paths()` lives in
    /// `still_deferred`, not `static_routes`. The detection seam must
    /// include both surfaces or `output: "static"` + `adapter: "none"`
    /// would slip through for projects using runtime-expanded SSR
    /// dynamic pages.
    ///
    /// This test exercises the run_build path with a synthetic dynamic
    /// SSR route whose source file does not exist on disk — the static
    /// `paths()` extractor fails and the route ends up deferred. The
    /// build must still refuse adapter: none.
    #[test]
    fn run_build_with_adapter_none_rejects_deferred_dynamic_ssr_routes() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        std::fs::create_dir_all(project_root.join("pages/api")).unwrap();
        // Page source exports `prerender = false` AND a `paths()` that
        // cannot be statically extracted — call into a non-literal value
        // (a function-returned array). The static extractor gives up,
        // build_route_universe + expand_dynamic_routes place this route in
        // `still_deferred`, and the precondition check must STILL fire.
        std::fs::write(
            project_root.join("pages/api/[slug].tsx"),
            "export const frontmatter = { title: \"Slug\" };\n\
             export const prerender = false;\n\
             export function paths() { return makePaths(); }\n\
             export default function P() { return null; }\n",
        )
        .unwrap();

        let routes = vec![zfb_router::Route {
            source_path: PathBuf::from("pages/api/[slug].tsx"),
            segments: vec![
                zfb_router::Segment::Static("api".into()),
                zfb_router::Segment::Dynamic("slug".into()),
            ],
            kind: zfb_router::RouteKind::Dynamic,
            specificity: 0,
            output_extension: None,
            static_html: false,
        }];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"));
        let cfg = Config::default(); // adapter is None
        let fake_adapter = FakeAdapterRunner::new();
        let err = run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap_err();
        let msg = format!("{err:#}");
        // The error must reach the user via the same path the static-route
        // case takes — `ensure_no_ssr_without_adapter` first, then resolve_v8_mode
        // if an adapter is configured. With adapter:none, the adapter check fires
        // and names the route template.
        assert!(msg.contains("/api/:slug"), "{msg}");
        assert!(msg.contains("SSR"), "{msg}");
        assert!(runner.bundle_calls.borrow().is_empty());
        assert!(runner.render_calls.borrow().is_empty());
    }

    /// Mirrors the test above but uses `output: "static"` + adapter set.
    /// With an adapter configured the no-adapter check passes; the
    /// `output: "static"` gate must catch the deferred-dynamic SSR
    /// route through `resolve_v8_mode`.
    #[test]
    fn run_build_with_output_static_rejects_deferred_dynamic_ssr_routes() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        std::fs::create_dir_all(project_root.join("pages/api")).unwrap();
        std::fs::write(
            project_root.join("pages/api/[slug].tsx"),
            "export const frontmatter = { title: \"Slug\" };\n\
             export const prerender = false;\n\
             export function paths() { return makePaths(); }\n\
             export default function P() { return null; }\n",
        )
        .unwrap();
        let routes = vec![zfb_router::Route {
            source_path: PathBuf::from("pages/api/[slug].tsx"),
            segments: vec![
                zfb_router::Segment::Static("api".into()),
                zfb_router::Segment::Dynamic("slug".into()),
            ],
            kind: zfb_router::RouteKind::Dynamic,
            specificity: 0,
            output_extension: None,
            static_html: false,
        }];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"));
        let cfg = Config {
            adapter: Some("@takazudo/zfb-adapter-cloudflare".into()),
            output: OutputMode::Static,
            ..Config::default()
        };
        let fake_adapter = FakeAdapterRunner::new();
        let err = run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("output: \"static\""), "{msg}");
        assert!(msg.contains("/api/:slug"), "{msg}");
        assert!(runner.bundle_calls.borrow().is_empty());
        assert!(runner.render_calls.borrow().is_empty());
    }

    /// Deep-review regression (PR #376): the SSR-route-key set handed to
    /// the deploy adapter's runtime-only bundle pass must include
    /// deferred-dynamic `prerender = false` routes. Mirrors the #373
    /// gate fix one level down: a dynamic route with a non-literal
    /// `paths()` sits in `still_deferred`, not `static_routes`. Earlier
    /// versions of `ssr_route_keys_for_runtime_bundle` only iterated
    /// `static_routes`, so the runtime worker bundle's tree-shake
    /// dropped the deferred SSR page entirely — breaking the deploy
    /// surface for projects that use runtime-expanded SSR.
    #[test]
    fn run_build_runtime_bundle_includes_deferred_dynamic_ssr_routes() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        std::fs::create_dir_all(project_root.join("pages/api")).unwrap();
        // SSG page so the build proceeds past the "no static routes"
        // short-circuit and reaches adapter dispatch.
        std::fs::write(
            project_root.join("pages/index.tsx"),
            "export default function() { return null; }\n",
        )
        .unwrap();
        // Dynamic [slug] page with prerender=false AND a non-literal
        // paths() — the static extractor defers it, and the FakeRunner's
        // eval_deferred_paths leaves it deferred (no V8 host in unit
        // tests). The runtime-bundle SSR key set must STILL pick it up.
        std::fs::write(
            project_root.join("pages/api/[slug].tsx"),
            "export const frontmatter = { title: \"Slug\" };\n\
             export const prerender = false;\n\
             export function paths() { return makePaths(); }\n\
             export default function P() { return null; }\n",
        )
        .unwrap();

        let routes = vec![
            static_route(vec![], "pages/index.tsx"),
            zfb_router::Route {
                source_path: PathBuf::from("pages/api/[slug].tsx"),
                segments: vec![
                    zfb_router::Segment::Static("api".into()),
                    zfb_router::Segment::Dynamic("slug".into()),
                ],
                kind: zfb_router::RouteKind::Dynamic,
                specificity: 0,
                output_extension: None,
                static_html: false,
            },
        ];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"));
        let cfg = Config {
            adapter: Some("@takazudo/zfb-adapter-cloudflare".into()),
            ..Config::default()
        };
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap();
        // Two bundle calls: the full SSG bundle, then the runtime-only
        // bundle for the adapter. The second carries worker_only_routes.
        let calls = runner.bundle_calls.borrow();
        assert_eq!(
            calls.len(),
            2,
            "expected one SSG bundle pass + one runtime-only bundle pass"
        );
        let worker_only = calls[1]
            .worker_only_routes
            .as_ref()
            .expect("runtime-only bundle pass must set worker_only_routes");
        assert!(
            worker_only.contains("/api/:slug"),
            "deferred-dynamic SSR route must reach the runtime bundle's \
             worker_only_routes; got {:?}",
            worker_only
        );
    }

    // ---------------------------------------------------------------------------
    // SSR catch-all with no paths() — issue #520 / #517 regression guard
    //
    // Note: this test covers the build-command population path (FakeRunner
    // stubbing). The bundler-side filter correctness (entry_key Hono-form so
    // worker_only_routes lookup matches) is exercised by the real-bundler test
    // in `crates/zfb-build/tests/worker_only_routes_filter.rs` (zfb#532).
    // ---------------------------------------------------------------------------

    /// A `prerender = false` catch-all with NO `paths()` export must:
    ///
    /// (a) build cleanly with no "skipping … no paths() export" warning
    ///     (the eval_deferred_paths input must be empty for this route),
    /// (b) have its `route_key` present in `worker_only_routes` of the
    ///     runtime-only bundle pass,
    /// (c) NOT take the `eval_deferred_paths` round-trip (the FakeRunner
    ///     receives an empty deferred slice for this route),
    /// (d) NOT be rendered to a `dist/` artifact (SSR route must not be
    ///     SSG'd to disk).
    ///
    /// Before the Approach-B pre-filter (#520) this route would land in
    /// `still_deferred` after `expand_dynamic_routes` returned a Missing
    /// reason, then be passed to `eval_deferred_paths` (wasted V8 round-
    /// trip) and `warn_deferred_dynamic` (misleading warning).
    #[test]
    fn run_build_ssr_catchall_no_paths_no_warning_and_in_worker_only() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        // SSG index page so the build proceeds past the "no static routes"
        // short-circuit.
        std::fs::create_dir_all(project_root.join("pages/foo")).unwrap();
        std::fs::write(
            project_root.join("pages/index.tsx"),
            "export default function() { return null; }\n",
        )
        .unwrap();
        // SSR catch-all with NO `paths()` export — the core of #517/#520.
        // A legitimate SSR catch-all only needs `prerender = false`; it
        // does not need (and must not be required to have) `paths()`.
        // Frontmatter is included so `build_prerender_map` can read the
        // `prerender = false` flag (the extractor requires a frontmatter
        // object to succeed; without it the route defaults to SSG).
        std::fs::write(
            project_root.join("pages/foo/[...rest].tsx"),
            "export const frontmatter = { title: \"Catch-all\" };\n\
             export const prerender = false;\n\
             export default function P() { return null; }\n",
        )
        .unwrap();

        let routes = vec![
            static_route(vec![], "pages/index.tsx"),
            zfb_router::Route {
                source_path: PathBuf::from("pages/foo/[...rest].tsx"),
                segments: vec![
                    zfb_router::Segment::Static("foo".into()),
                    zfb_router::Segment::Catchall("rest".into()),
                ],
                kind: zfb_router::RouteKind::Dynamic,
                specificity: 0,
                output_extension: None,
                static_html: false,
            },
        ];

        // Track whether eval_deferred_paths was called with the SSR
        // catch-all route. We use FakeRunner which records its inputs;
        // the SSR route must NOT appear in the deferred slice.
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"));
        let cfg = Config {
            adapter: Some("@takazudo/zfb-adapter-cloudflare".into()),
            ..Config::default()
        };
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap();

        let bundle_calls = runner.bundle_calls.borrow();

        // (b) The route_key /foo/:rest{.+} must appear in worker_only_routes
        //     of the runtime-only bundle pass (second bundle call).
        assert_eq!(
            bundle_calls.len(),
            2,
            "expected SSG bundle pass + runtime-only bundle pass"
        );
        let worker_only = bundle_calls[1]
            .worker_only_routes
            .as_ref()
            .expect("runtime-only bundle pass must set worker_only_routes");
        // Catch-all `[...rest]` segments compile to the exact route key
        // `/foo/:rest{.+}` (see `zfb_router`'s template formatter); pin
        // the assertion to that exact string so a future router-format
        // change fails this guard loudly instead of silently matching a
        // stray `/foo/whatever` key.
        assert!(
            worker_only.iter().any(|k| k == "/foo/:rest{.+}"),
            "SSR catch-all route_key /foo/:rest{{.+}} must reach worker_only_routes; got {:?}",
            worker_only
        );

        // (a) + (c) The SSR catch-all must NOT appear in the deferred slice
        //     passed to eval_deferred_paths. We verify this indirectly: the
        //     FakeRunner's eval_deferred_paths returns the input unchanged,
        //     so if the route reached it, it would show up in the second
        //     bundle call's still_deferred (which feeds warn_deferred_dynamic).
        //     We do this by checking the render input's route_universe: the
        //     SSR catch-all must NOT have been SSG'd to a concrete URL entry.
        let render_calls = runner.render_calls.borrow();
        assert_eq!(render_calls.len(), 1);
        let universe = &render_calls[0].route_universe;
        let catchall_in_universe = universe
            .iter()
            .any(|e| e.url_path.starts_with("/foo/") || e.route_key.starts_with("/foo/"));
        assert!(
            !catchall_in_universe,
            "(d) SSR catch-all must NOT appear in the SSG render universe (would shadow SSR handler); \
             got universe: {:?}",
            universe.iter().map(|e| &e.url_path).collect::<Vec<_>>()
        );
    }

    // ---------------------------------------------------------------------------
    // build_content_snapshot_json — no-collections cost guard (issue #495)
    // ---------------------------------------------------------------------------

    /// `build_content_snapshot_json` must return `None` immediately when
    /// `config.collections` is empty. This is the "projects without collections
    /// still pay nothing" guarantee preserved by the helper's own early-return
    /// guard after the `!still_deferred.is_empty()` gate was removed from the
    /// orchestrator (issue #495).
    ///
    /// A real project root is not needed — the helper exits before touching the
    /// filesystem when `config.collections` is empty.
    #[test]
    fn build_content_snapshot_json_returns_none_for_empty_collections() {
        let cfg = Config::default();
        assert!(
            cfg.collections.is_empty(),
            "Config::default() must produce no collections for this test to be meaningful"
        );
        let tmp = tempdir().unwrap();
        let result = build_content_snapshot_json(tmp.path(), &cfg);
        assert!(
            result.is_none(),
            "build_content_snapshot_json must return None when collections is empty \
             (no-collections cost guard must be preserved after gate removal)"
        );
    }

    // Note: read_tsconfig_paths tests have been moved to
    // crates/zfb-plugin-resolver/src/lib.rs (read_tsconfig_paths_into_map
    // tests) as part of the shared-helper extraction in issue #901.

    /// Issue #974 — eval_deferred_paths must be invoked exactly once per
    /// run_build call, not once per deferred route or once per render pass.
    ///
    /// This guards the orchestration call structure: the build pipeline
    /// resolves all deferred dynamic routes in a single `eval_deferred_paths`
    /// call so that the runtime (embedded V8 or HTTP worker) is started once,
    /// queried for every pending route, and then shut down. Re-invoking per
    /// route would restart the worker N times and defeat the paths() memo in
    /// the JS router.
    #[test]
    fn run_build_calls_eval_deferred_paths_exactly_once() {
        // Stage two dynamic pages so there are multiple deferred routes.
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);

        // pages/[slug].tsx — source exists so static expansion defers it
        // (non-literal paths()); FakeRunner will keep it deferred.
        std::fs::create_dir_all(project_root.join("pages")).unwrap();
        std::fs::write(
            project_root.join("pages/[slug].tsx"),
            "export async function paths() {\n\
                const { getCollection } = await import('zfb/content');\n\
                return [];\n\
             }\n",
        )
        .unwrap();

        // pages/[tag].tsx — same pattern, second deferred route.
        std::fs::write(
            project_root.join("pages/[tag].tsx"),
            "export async function paths() {\n\
                const { getCollection } = await import('zfb/content');\n\
                return [];\n\
             }\n",
        )
        .unwrap();

        let routes = vec![
            dynamic_route("slug", "pages/[slug].tsx"),
            dynamic_route("tag", "pages/[tag].tsx"),
        ];
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"));
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap();

        // eval_deferred_paths must be called exactly once per run_build,
        // not once per deferred route (which would be 2 here).
        assert_eq!(
            *runner.eval_deferred_paths_calls.borrow(),
            1,
            "eval_deferred_paths must be called exactly once per run_build (issue #974)"
        );
    }

    // ---------------------------------------------------------------------------
    // base="/foo/" + client-script end-to-end (#978)
    // ---------------------------------------------------------------------------
    //
    // Acceptance: with `config.base = "/foo/"`, a client-script stable URL
    // emitted by the renderer as `/foo/assets/client/x.js` (the base-prefixed
    // stable URL) must be rewritten by `ProductionAssetPipeline` to
    // `/foo/assets/client/x-<hash>.js`.  This proves that the base-prefixed
    // stable URL is the exact rewrite key end-to-end.
    //
    // The stable URL that the renderer receives already carries the `/foo`
    // prefix because `apply_asset_url_base` mutates `stable_url` in-place
    // before handing it to the renderer.  `boundary_replace` then searches
    // for that same prefixed string in the rendered HTML and replaces it with
    // the hashed equivalent.  If either side used an unprefixed URL, the
    // rewrite would never fire and the stable URL would leak.

    #[test]
    fn run_build_with_base_emits_prefixed_hashed_client_script_url_in_html() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        let routes = vec![static_route(vec![], "pages/index.tsx")];
        // Seed the runner with one client-script payload carrying the UNPREFIXED
        // stable URL.  `apply_asset_url_base` (called inside run_build) will
        // prepend "/foo" before handing it to the renderer and pipeline, so the
        // test exercises the full base-rewrite path.
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"))
            .with_prod_asset_inputs(ProdAssetEmitterInputs {
                css: None,
                islands: None,
                client_scripts: vec![zfb_build::pipeline::AssetEmitterPayload {
                    bytes: b"// search widget".to_vec(),
                    relative_path: PathBuf::from("assets/client/x.js"),
                    // Seeded with the unprefixed stable URL; apply_asset_url_base
                    // will prepend "/foo" → "/foo/assets/client/x.js".
                    stable_url: "/assets/client/x.js".to_string(),
                    companions: Vec::new(),
                }],
            })
            // The page references the client script via `clientScript("x")`,
            // which under `base="/foo/"` emits the base-prefixed stable URL.
            // This is the explicit reference the pipeline rewrites — client
            // scripts are not auto-injected into the head (#971 P2).
            .with_page_client_script_refs(vec!["/foo/assets/client/x.js".to_string()]);
        let cfg = Config {
            base: Some("/foo/".to_string()),
            ..Config::default()
        };
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap();

        // (a) The hashed bundle lands under dist/assets/client/x-<8hex>.js.
        let client_entries: Vec<String> = std::fs::read_dir(outdir.join("assets/client"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            client_entries.len(),
            1,
            "expected exactly one hashed client-script asset; got {client_entries:?}",
        );
        let name = &client_entries[0];
        assert!(
            name.starts_with("x-") && name.ends_with(".js") && name.len() == "x-12345678.js".len(),
            "expected x-<8hex>.js; got {name}",
        );

        // (b) The HTML carries the PREFIXED hashed URL; neither the prefixed
        //     stable URL nor the unprefixed variants leak.
        let prefixed_hashed = format!("/foo/assets/client/{name}");
        let html = std::fs::read_to_string(outdir.join("index.html")).unwrap();
        assert!(
            html.contains(&prefixed_hashed),
            "prefixed hashed URL {prefixed_hashed} missing from HTML:\n{html}",
        );
        assert!(
            !html.contains("\"/foo/assets/client/x.js\""),
            "prefixed stable URL leaked into HTML:\n{html}",
        );
        assert!(
            !html.contains("\"/assets/client/x.js\""),
            "unprefixed stable URL leaked into HTML:\n{html}",
        );

        // (c) Client scripts are NOT auto-injected into the head (#971 P2):
        //     with no css/islands bytes, `prod_head_assets` is `None`. The
        //     prefixed stable URL reaches HTML only via the page's explicit
        //     `clientScript()` reference, and `apply_asset_url_base` still
        //     prefixes the payload's `stable_url` so the pipeline's
        //     boundary_replace rewrite key matches that explicit reference.
        let render_calls = runner.render_calls.borrow();
        assert!(
            render_calls[0].prod_head_assets.is_none(),
            "client scripts must not be auto-injected; prod_head_assets should \
             be None with no css/islands bytes, got {:?}",
            render_calls[0].prod_head_assets,
        );
    }

    /// Zero-script build with no-base should produce byte-identical bundle to a
    /// pre-#978 build: `globalThis.__zfb.base` must NOT appear in the bundle.
    /// Guards the #261 zero-registration parity and #940 byte-identical dev
    /// bundle skip invariants.
    #[test]
    fn run_build_without_client_scripts_does_not_emit_base_in_bundle() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        let routes = vec![static_route(vec![], "pages/index.tsx")];
        // No client_scripts in the prod asset inputs → base_prefix must stay None.
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs"));
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            build_pages_root: project_root,
            user_pages_dir: project_root,
            package_route_entrypoints: &[],
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
            minify_html: false,
        })
        .unwrap();

        // The bundler input must have had base_prefix = None, which means the
        // entry.mjs written to the shadow tree must NOT contain the base setter.
        // FakeRunner records the BundlerInput via bundle_calls; check its base_prefix.
        let bundle_calls = runner.bundle_calls.borrow();
        assert!(
            bundle_calls[0].base_prefix.is_none(),
            "zero-script build must pass base_prefix=None to the bundler; got {:?}",
            bundle_calls[0].base_prefix,
        );
    }
}
