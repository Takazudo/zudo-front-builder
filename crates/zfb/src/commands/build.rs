//! `zfb build` command — one-shot production build.
//!
//! Contract:
//!   pub async fn run(args: &crate::cli::BuildArgs) -> anyhow::Result<()>
//!
//! `args.outdir` is the production output directory (default `dist`).
//! Resolved relative to the current working directory if not absolute.
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
//! `dist/`; a follow-up sub-task adds runtime evaluation for those.
//!
//! The contract for callers (project-root sanity check, `outdir`
//! handling, `✓ N pages built in X.XXs` summary) is unchanged.

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
    apply_prod_asset_pipeline, synthesize_page_id_from_output, AssetEmitterPayload,
    ProdAssetEmitterInputs, ProdRenderedFile, RelDistPath,
};
use zfb_build::renderer::{render_all, Backend, RendererInput, RendererOutput};
use zfb_css::{
    css_relative_path, CssPipeline, CssPipelineConfig, TailwindSubprocessConfig,
    TailwindSubprocessEngine,
};
use zfb_islands::{
    build_production_islands_asset, scan_islands_with_meta, BundleConfig, EsbuildSubprocessBundler,
    EsbuildSubprocessConfig, FsResolver,
};
use zfb_router::Router;

use zfb_render::paths::PathsCache;

use crate::cli::BuildArgs;
use crate::commands::resolve::resolve_outdir;
use crate::config::{Config, OutputMode};
use crate::output;
use crate::render_pipeline::{
    build_prerender_map, build_route_universe, cfg_framework_to_render, check_runtime_installed,
    embedded_binary, embedded_node_modules, eval_deferred_paths_via_worker, expand_dynamic_routes,
    is_ssr_route, DeferredDynamicRoute, DynamicResolvedEntry,
    RouteUniversePlan, WorkerDispatch,
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

    // Project-root sanity check (cheap and matches the watcher's idea
    // of "is this a zfb project").
    let pages_dir = project_root.join("pages");
    if !pages_dir.is_dir() {
        return Err(anyhow!(
            "no `pages/` directory found in {}; run `zfb build` from a project root",
            project_root.display()
        ));
    }

    let config = crate::config::load_from_dir(&project_root)
        .await
        .context("failed to load project configuration")?;

    let outdir = resolve_outdir(&project_root, &args.outdir);

    // Sub 3 / #108 — plugin lifecycle. Spawn the host before any heavy
    // work so `preBuild` can prepare files the bundler will see (e.g.
    // claude-resources index emission). If no plugins are declared, we
    // skip the spawn entirely so a config-less project pays nothing.
    let plugin_host = crate::commands::plugins::maybe_spawn_host(&config).await?;

    // #255 — run the new `setup` hook once, before `preBuild`. The
    // returned registries (aliases, virtual modules, injected routes)
    // are owned by `zfb-build` and consumed downstream by Wave 2
    // (#260 V8 host resolver, #261 islands esbuild resolver). For the
    // build command `injectRoute` is rejected by the accumulator with
    // `InjectRouteInBuildMode` — see crates/zfb-build/src/plugin_registries.rs.
    let setup_registries = if let Some(host) = plugin_host.as_ref() {
        let cfg_json = serde_json::to_value(&config)
            .context("plugin lifecycle: serialise config for setup ctx")?;
        let regs = host
            .run_setup(&project_root, zfb_build::SetupCommand::Build, &cfg_json)
            .await
            .map_err(zfb_build::annotate_with_plugin_error)
            .context("setup lifecycle hook")?;
        regs
    } else {
        zfb_build::SetupRegistries::empty()
    };

    // #261 — pre-fetch virtual module sources (async) before entering the
    // synchronous block_in_place section. `invoke_virtual_loader` is async,
    // so we collect all sources here and pass the plain strings to the
    // synchronous `build_default_islands_payload` via `IslandsPluginConfig`.
    // Pre-fetch all virtual-module sources once. They feed BOTH the islands
    // esbuild plugin config (#261) and the V8 host's plugin hooks (#260), so
    // a single async loop here invokes each loader exactly once per build.
    let mut virtual_sources: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    if let Some(host) = plugin_host.as_ref() {
        for (specifier, vm_entry) in setup_registries.virtual_modules.iter() {
            match host.invoke_virtual_loader(&vm_entry.loader_id).await {
                Ok(source) => {
                    virtual_sources.insert(specifier.clone(), source);
                }
                Err(e) => {
                    return Err(zfb_build::annotate_with_plugin_error(e))
                        .with_context(|| {
                            format!(
                                "plugin lifecycle: failed to load virtual module \
                                 `{specifier}` (plugin: `{plugin}`)",
                                plugin = vm_entry.plugin
                            )
                        });
                }
            }
        }
    }

    let islands_plugin_config = {
        let alias_entries: Vec<(String, String)> = setup_registries
            .aliases
            .iter()
            .map(|(from, entry)| (from.clone(), entry.target.to_string_lossy().into_owned()))
            .collect();
        let virtual_modules: Vec<(String, String)> = virtual_sources
            .iter()
            .map(|(spec, src)| (spec.clone(), src.clone()))
            .collect();
        IslandsPluginConfig {
            alias_entries,
            virtual_modules,
        }
    };
    let v8_plugin_hooks = crate::v8_host_adapter::translate_setup_registries_to_hooks(
        &setup_registries,
        &virtual_sources,
    );

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

    let router = Router::scan(&pages_dir)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("scanning routes under {}", pages_dir.display()))?;
    let routes = router.routes();

    // The build pipeline (bundler subprocess, embedded V8 host boot,
    // all SSG dispatch calls) is fundamentally synchronous, but
    // `#[tokio::main]` keeps a multi-thread runtime live in the
    // background. Without `block_in_place` the inner blocking
    // subroutines panic on drop with `Cannot drop a runtime in a
    // context where blocking is not allowed`. Telling the outer
    // runtime up-front that the next stretch of work is blocking is
    // the supported escape hatch.
    // Derive the alias / virtual-module lists for the main bundler from the
    // same sources used for the islands path. The islands path consumed these
    // via `IslandsPluginConfig`; here we produce the same `Vec<(String,
    // String)>` shape expected by `BundlerInput::plugin_alias_entries` /
    // `plugin_virtual_modules` (#268).
    let main_bundler_alias_entries: Vec<(String, String)> = setup_registries
        .aliases
        .iter()
        .map(|(from, entry)| (from.clone(), entry.target.to_string_lossy().into_owned()))
        .collect();
    let main_bundler_virtual_modules: Vec<(String, String)> = virtual_sources
        .iter()
        .map(|(spec, src)| (spec.clone(), src.clone()))
        .collect();

    let (pages_built, route_manifest) = tokio::task::block_in_place(|| {
        run_build(BuildArgsResolved {
            project_root: &project_root,
            outdir: &outdir,
            config: &config,
            routes,
            runner: &DefaultRunner {
                islands_plugin_config,
                v8_plugin_hooks,
            },
            adapter_runner: &DefaultAdapterRunner,
            plugin_alias_entries: main_bundler_alias_entries,
            plugin_virtual_modules: main_bundler_virtual_modules,
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
                let first = ssr_routes
                    .first()
                    .expect("checked non-empty above");
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

// ---------------------------------------------------------------------------
// Internals — testable orchestration
// ---------------------------------------------------------------------------

/// Resolved inputs to the orchestration. Kept as a struct so the
/// orchestration body and the tests share one signature; adding a field
/// later doesn't ripple into call sites.
struct BuildArgsResolved<'a, R: BuildRunner, A: AdapterRunner> {
    project_root: &'a Path,
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
    fn emit_prod_assets(
        &self,
        project_root: &Path,
        outdir: &Path,
        config: &Config,
    ) -> Result<ProdAssetEmitterInputs>;
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
        let factory = crate::v8_host_adapter::make_v8_host_factory_with_hooks(
            self.v8_plugin_hooks.clone(),
        );
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
        outdir: &Path,
        config: &Config,
    ) -> Result<ProdAssetEmitterInputs> {
        // Run `CssPipeline::build_emitter` and
        // `build_production_islands_asset` eagerly (before render) so
        // head injection knows which stable URLs are backed by
        // bytes. Either slot independently returns `None` when the
        // project doesn't exercise it (Tailwind disabled, no
        // `"use client"` components, etc.).
        let css = build_default_css_payload(project_root, outdir, config)
            .context("CSS emitter (DefaultRunner) failed")?;
        let islands = build_default_islands_payload(
            project_root,
            outdir,
            config.framework,
            &self.islands_plugin_config,
        )
        .context("islands emitter (DefaultRunner) failed")?;
        Ok(ProdAssetEmitterInputs { css, islands })
    }
}

/// Run the real `CssPipeline::build_emitter` for a project and return
/// its bytes packaged for [`ProductionAssetPipeline`].
///
/// Returns `Ok(None)` when:
///
/// - the user explicitly disabled Tailwind via
///   `zfb.config.{ts,json}` (`tailwind: { enabled: false }`), OR
/// - no scannable source files were found under the conventional
///   project roots (`pages/`, `components/`, `layouts/`, `content/`).
///   In that case the project carries no utility-class authoring
///   surface and emitting an empty stylesheet would just leave a
///   broken `<link>` tag in HTML.
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
) -> Result<Option<AssetEmitterPayload>> {
    // Honour the user's opt-out switch before doing any source
    // discovery work — keeps the default runner free of subprocess
    // cost when the project explicitly does not want Tailwind.
    let tailwind_enabled = config.tailwind.as_ref().map(|t| t.enabled).unwrap_or(true);
    if !tailwind_enabled {
        return Ok(None);
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
    let content_globs = zfb_css::engine::DEFAULT_CONTENT_ROOTS
        .iter()
        .map(|root| project_root.join(root).to_string_lossy().into_owned())
        .collect::<Vec<_>>();

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

    let pipe_cfg = CssPipelineConfig {
        sources,
        // The on-disk class-map JSON writer is not used: the build-time
        // CSS Modules rewrite consumes the maps in-memory instead.
        // `compute_css_module_class_maps` runs `CssModulesProcessor`
        // directly (same default config this emitter uses, so scoped
        // names agree) and feeds `BundlerInput::css_module_class_maps`,
        // which the bundler applies in the shadow tree. No JSON channel
        // is needed, so `class_map_dir` stays `None`.
        class_map_dir: None,
        // `output_root` is unused by `build_emitter` (it does not
        // write the hashed asset itself) but is read by the
        // class-map writer when `class_map_dir` is `Some`. Pin it to
        // the configured outdir for forward-compat.
        output_root: outdir.to_path_buf(),
        ..CssPipelineConfig::default()
    };

    let pipeline = CssPipeline::new(engine, pipe_cfg);
    let emitter_out = pipeline.build_emitter()?;

    Ok(Some(AssetEmitterPayload {
        bytes: emitter_out.bytes,
        relative_path: css_relative_path(),
        stable_url: emitter_out.stable_url,
    }))
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
fn resolve_input_global_css(project_root: &Path) -> Option<PathBuf> {
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
/// Returns an empty map when Tailwind/CSS is disabled or no
/// `.module.css` files are reachable — the build then behaves exactly
/// as before.
pub(crate) fn compute_css_module_class_maps(
    project_root: &Path,
    config: &Config,
) -> Result<std::collections::HashMap<PathBuf, std::collections::HashMap<String, String>>> {
    use std::collections::HashMap;

    // CSS Modules processing is independent of Tailwind, but the CSS
    // emitter is gated on `tailwind.enabled`. When CSS is disabled
    // entirely there is no stylesheet to carry the scoped CSS, so a
    // class-map rewrite would point at classes that never ship. Keep
    // the two sides consistent: skip the rewrite when CSS is off.
    let css_enabled = config.tailwind.as_ref().map(|t| t.enabled).unwrap_or(true);
    if !css_enabled {
        return Ok(HashMap::new());
    }

    let sources = discover_css_source_files(project_root);
    if sources.is_empty() {
        return Ok(HashMap::new());
    }

    let scan = zfb_css::scan_css_module_imports(&sources)
        .context("CSS Modules import scan failed")?;

    // Auto-discovered modules: keep only resolved paths that exist on
    // disk — mirrors `CssPipeline::collect_modules`. Bare specifiers
    // (`@org/pkg/x.module.css`) cannot be compiled by lightningcss and
    // are dropped here too.
    let module_files: Vec<PathBuf> = scan
        .modules
        .into_iter()
        .filter(|m| m.exists())
        .collect();
    if module_files.is_empty() {
        return Ok(HashMap::new());
    }

    let processor =
        zfb_css::CssModulesProcessor::new(zfb_css::modules::CssModulesConfig::default());
    let out = processor
        .process(&module_files)
        .context("CSS Modules compilation failed")?;
    Ok(out.class_maps)
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
///   page paths).
///
/// On `Ok(Some(_))` the orchestrator hashes the bytes and writes
/// `dist/assets/islands-<hash>.js`. The bundler also wrote the
/// stable-named `dist/assets/islands.js` as a side effect; the
/// renderer's HTML never references that stable file directly
/// because the rewrite step swaps it for the hashed URL.
pub(crate) fn build_default_islands_payload(
    project_root: &Path,
    outdir: &Path,
    framework: crate::config::Framework,
    plugin_config: &IslandsPluginConfig,
) -> Result<Option<AssetEmitterPayload>> {
    // Walk the conventional islands roots. The scanner DFS-walks
    // imports starting from each entry path, so seeding with the
    // pages dir is enough — anything reachable through a
    // page → component import chain gets found.
    let pages_dir = project_root.join("pages");
    let mut entries: Vec<std::path::PathBuf> = Vec::new();
    if pages_dir.is_dir() {
        for ext in ["tsx", "ts", "jsx", "js"] {
            for entry in walkdir::WalkDir::new(&pages_dir)
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
    if entries.is_empty() {
        return Ok(None);
    }

    let resolver = FsResolver::new();
    let (islands_set, scan_meta) = match scan_islands_with_meta(&entries, &resolver) {
        Ok(result) => result,
        Err(e) => {
            output::warn(format!(
                "islands scanner failed ({e}); skipping islands asset emission"
            ));
            return Ok(None);
        }
    };
    // Issue #289: a project may use `<ClientRouter />` without any
    // `"use client"` islands (a static page that only wants View
    // Transitions). When the scanner detected client-router usage, the
    // islands asset still has to be emitted so the runtime's side-effect
    // import ships — so the empty-islands short-circuit below only fires
    // when client-router is NOT in play.
    if islands_set.is_empty() && !scan_meta.uses_client_router {
        // Issue #122 / #117: this branch used to be silent, which made
        // pnpm-workspace consumers with `"use client"` islands inside a
        // workspace package look "fine" while shipping no client
        // runtime. Surface it loudly so authoring problems (a missing
        // `"use client"` directive, an island reachable only through a
        // path the scanner can't follow) become discoverable.
        output::warn(format!(
            "scanned {} page entr{} but found no \"use client\" islands; \
             dist/assets/islands.js will not be emitted. \
             Verify each island module starts with the literal directive \
             \"use client\" and is reachable from a page in pages/.",
            entries.len(),
            if entries.len() == 1 { "y" } else { "ies" }
        ));
        return Ok(None);
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
    let bundle_cfg = BundleConfig::production()
        .with_outdir(outdir.to_path_buf())
        .with_jsx_import_source(islands_jsx_import_source)
        .with_client_router(scan_meta.uses_client_router);

    match build_production_islands_asset(&bundler, &islands_set, &bundle_cfg)? {
        Some(asset) => Ok(Some(AssetEmitterPayload {
            bytes: asset.bytes,
            relative_path: asset.relative_path,
            stable_url: asset.stable_url,
        })),
        None => Ok(None),
    }
}

/// Drive the build for a fully-resolved input set. Returns the number
/// of pages written and the postBuild route manifest (#262).
fn run_build<R: BuildRunner, A: AdapterRunner>(
    args: BuildArgsResolved<'_, R, A>,
) -> Result<(usize, zfb_build::PostBuildRouteManifest)> {
    let BuildArgsResolved {
        project_root,
        outdir,
        config,
        routes,
        runner,
        adapter_runner,
        plugin_alias_entries,
        plugin_virtual_modules,
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
    let expansion =
        expand_dynamic_routes(&ssg_deferred, project_root, &mut paths_cache)?;
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
    // The intermediate `.zfb-build/` directory holds `bundle.mjs` and
    // its `.map` — the SSR worker bundle the renderer below loads. It
    // lives at `<project_root>/.zfb-build/`, NOT under `<outdir>/`,
    // because anything inside `outdir` is part of the deploy upload
    // (Cloudflare Pages, Netlify, S3, etc.) and these are build
    // intermediates, not deploy artifacts. See zfb#231 for the
    // information-disclosure / wasted-bytes rationale. The renderer +
    // adapter both consume the absolute `bundler_out.bundle_path`
    // returned below, so the location is opaque to them.
    let mut bundler_input = BundlerInput::for_project(
        project_root.to_path_buf(),
        cfg_framework_to_render(config.framework),
        BundleMode::Production,
        project_root.join(".zfb-build"),
        content_snapshot_json,
    );
    // Discover the Next-style root `mdx-components.tsx` convention (#616):
    // a project-wide element→component override map applied to every
    // `<Content>`. Gated on the file existing so a project without it gets
    // byte-for-byte identical output. The bundler copies it into the shadow
    // root and emits the `globalThis.__zfb.mdxComponents` installer.
    bundler_input.mdx_components_file = discover_mdx_components_file(project_root);
    // Inject project-side resolution context so esbuild can find
    // user dependencies + path aliases. Without these the shadow
    // tempdir has no `node_modules` to walk into and no tsconfig
    // `paths` to honour, so anything beyond a self-contained page
    // module fails to resolve.
    //
    // When the project has no node_modules at all (cargo-install scenario),
    // fall back to the binary-embedded @takazudo packages so esbuild can
    // still resolve `@takazudo/zfb` and `@takazudo/zfb-runtime`. The
    // `_embedded_nm_handle` keeps the tempdir alive for the duration of
    // the bundle step; it is dropped after `bundle(...)` returns.
    let _embedded_nm_handle: Option<tempfile::TempDir>;
    if let Some(nm) = detect_project_node_modules(project_root) {
        bundler_input.node_modules_dir = Some(nm);
        _embedded_nm_handle = None;
    } else {
        match embedded_node_modules() {
            Ok((handle, nm_path)) => {
                bundler_input.node_modules_dir = Some(nm_path);
                // Vendored / cargo-install mode: the project has no
                // `node_modules`, so the bundler injected one at a
                // tempdir. esbuild must STAY at the shadow path
                // (`<shadow>/node_modules/<pkg>`) during resolution —
                // see the `--preserve-symlinks` block in
                // `run_esbuild` and `BundlerInput::node_modules_preserve_symlinks`
                // for the full rationale. Production builds with a
                // real project `node_modules` (the branch above) leave
                // this `false` so workspace-package `@/*` aliases keep
                // resolving (#443 / #450).
                bundler_input.node_modules_preserve_symlinks = true;
                _embedded_nm_handle = Some(handle);
            }
            Err(e) => {
                // Non-fatal: log a warning and continue without injecting a
                // node_modules_dir. The build will likely fail later if the
                // project also has no ancestor node_modules, but that failure
                // produces a more useful esbuild error message than aborting here.
                crate::output::warn(format!(
                    "could not extract embedded @takazudo packages ({e}); \
                     falling back to node_modules walk"
                ));
                _embedded_nm_handle = None;
            }
        }
    }
    bundler_input.tsconfig_paths = read_tsconfig_paths(project_root);
    // Per-collection content materialisation feeds the MDX content
    // bridge (#506) — without this every doc page would render as
    // raw markdown text in a `<pre data-zfb-content-fallback>` block
    // because `globalThis.__zfb.content.get(specifier)` would return
    // `undefined`.
    bundler_input.content_collections = config
        .collections
        .iter()
        .map(|c| zfb_build::ContentCollectionSpec {
            name: c.name.clone(),
            root: c.path.clone(),
            include: c.include.clone(),
            exclude: c.exclude.clone(),
            id_strip_suffix: c.id_strip_suffix.clone(),
        })
        .collect();
    // Thread the opt-in `stripMdExt` flag from `zfb.config.ts` into the
    // bundler so the hoisted MDX pre-compile pipeline appends
    // `StripMdExtensionPlugin`. Mirrored in `commands/dev.rs` so dev
    // and build produce the same href shape (zfb#127 / #129).
    bundler_input.strip_md_ext = config.strip_md_ext;
    // Thread the opt-in `resolveMarkdownLinks` config into the bundler
    // so the hoisted MDX pre-compile pipeline appends
    // `ResolveLinksPlugin`. Without this wiring the bundler's MDX
    // pipeline only ran `StripMdExtensionPlugin`, and author-written
    // relative `.mdx` links were emitted as relative href values that
    // broke at the file→directory transformation in dist HTML
    // (sub #234 / zudolab/zudo-doc#1577). The shared helper
    // `resolve_links_routes_from_config` builds the same per-route map
    // the snapshot path uses so content_hash stays deterministic.
    if let Some(routes) = resolve_links_routes_from_config(project_root, config) {
        let on_broken_links = match config
            .resolve_markdown_links
            .as_ref()
            .map(|r| r.on_broken_links)
            .unwrap_or_default()
        {
            crate::config::OnBrokenLinks::Warn => zfb_build::bundler::OnBrokenLinks::Warn,
            crate::config::OnBrokenLinks::Error => zfb_build::bundler::OnBrokenLinks::Error,
            crate::config::OnBrokenLinks::Ignore => zfb_build::bundler::OnBrokenLinks::Ignore,
        };
        bundler_input.resolve_markdown_links = Some(zfb_build::bundler::ResolveMarkdownLinksSpec {
            routes: routes
                .into_iter()
                .map(|r| zfb_build::bundler::ResolveMarkdownLinksRoute {
                    docs_dir: r.dir,
                    route_prefix: r.route_prefix,
                })
                .collect(),
            on_broken_links,
        });
    }
    // Thread the optional `codeHighlight.theme` from `zfb.config.ts`
    // so the hoisted MDX pre-compile pipeline uses the configured
    // syntect theme instead of the default `base16-ocean.dark`.
    bundler_input.code_highlight_theme =
        config.code_highlight.as_ref().and_then(|c| c.theme.clone());
    // Thread the optional `markdown.gfm` and `markdown.cjkFriendly`
    // config into the bundler so the hoisted MDX pre-compile pipeline
    // parses the same constructs the snapshot walker uses. Both are
    // resolved from the same source, so `content_hash` inputs stay
    // byte-identical (the snapshot ↔ bundler land mine called out
    // Thread the optional `codeHighlight.themesDir` (resolved to an
    // absolute path here) so the bundler loads custom .tmTheme files
    // before constructing the SyntectPlugin.  MUST stay in sync with
    // the snapshot wiring above so both content_hash inputs agree.
    bundler_input.code_highlight_themes_dir = config
        .code_highlight
        .as_ref()
        .and_then(|c| c.themes_dir.as_ref())
        .map(|td| project_root.join(td));
    // Thread the optional `markdown.gfm` config into the bundler so
    // the hoisted MDX pre-compile pipeline parses the same GFM
    // constructs the snapshot walker uses. The snapshot wiring above
    // resolves from the same source, so both `content_hash` inputs
    // stay byte-identical (the snapshot ↔ bundler land mine called out
    // at `crates/zfb-content/src/content_bridge.rs:118-153`).
    bundler_input.gfm_constructs =
        crate::config::resolve_gfm_constructs(config.markdown.as_ref());
    // Thread the optional `site` canonical-origin URL from `zfb.config.ts`
    // so the bundler emits `globalThis.__zfb.site` in `entry.mjs` for
    // layout-side canonical tag, OG URL, and sitemap construction (sub #254).
    bundler_input.site = config.site.clone();
    // Thread `prefetch.disabled` so `zfb build` emits
    // `globalThis.__zfb.prefetchDisabled = true` in `entry.mjs` when the
    // user sets `prefetch: { disabled: true }` in `zfb.config.ts` (sub #277).
    bundler_input.prefetch_disabled = config
        .prefetch
        .as_ref()
        .and_then(|p| p.disabled)
        .unwrap_or(false);
    bundler_input.toc = config.markdown.as_ref().and_then(|m| m.toc.clone());
    // Thread `markdown.externalLinks` into the bundler so the hoisted MDX
    // pre-compile pipeline appends `ExternalLinksPlugin`. MUST mirror
    // the snapshot wiring above; divergence shifts `content_hash` and
    // breaks the snapshot ↔ bundler bridge lookup.
    // `site` (top-level config.site, #254) lets `ExternalLinksPlugin`
    // classify same-origin absolute URLs as internal.
    bundler_input.external_links = config
        .markdown
        .as_ref()
        .and_then(|m| m.external_links.clone())
        .map(|el| (el.into_content_config(), config.site.clone()));
    bundler_input.cjk_friendly =
        crate::config::resolve_cjk_friendly(config.markdown.as_ref());
    // #586 — thread `markdown.features` into the bundler so opt-in feature
    // plugins (mermaid, …) fire per the configured toggles.
    // `None` keeps the legacy always-on chain, byte-identical to today.
    bundler_input.markdown_features =
        config.markdown.as_ref().and_then(|m| m.features.clone());
    // #268 — thread plugin-registered aliases and virtual modules into the
    // main bundler's esbuild invocation so page / layout / shared SSR-only
    // modules can consume them. The SAME data already feeds the islands path
    // via `IslandsPluginConfig`; here we plumb it into `BundlerInput` so the
    // main SSR bundle path gets identical resolution behaviour.
    bundler_input.plugin_alias_entries = plugin_alias_entries;
    bundler_input.plugin_virtual_modules = plugin_virtual_modules;
    // Sub #212 follow-up — extend the embedded-binary extraction tier to
    // the bundler step. `crates/zfb-build/src/bundler.rs::resolve_esbuild_binary_with_env`
    // previously walked only `input.esbuild_binary`, then `ZFB_ESBUILD_BIN`,
    // then a fixed `crates/zfb/binaries/esbuild/esbuild` slot — and erred
    // out on consumer projects that don't ship that slot dir.
    // Mirror the tailwindcss-v4 wiring at `tailwind_for_runner_with_paths`:
    // skip when an explicit override is in play (input field or env var),
    // otherwise pre-extract the embedded esbuild and pin its path on the
    // input. The TempDir handle rides alongside the bundler input via
    // `_embedded_esbuild_handle` so it outlives `bundle(...)`.
    let _embedded_esbuild_handle: Option<tempfile::TempDir>;
    if bundler_input.esbuild_binary.is_none() && std::env::var_os("ZFB_ESBUILD_BIN").is_none() {
        match embedded_binary("esbuild") {
            Ok((handle, path)) => {
                bundler_input.esbuild_binary = Some(path);
                _embedded_esbuild_handle = Some(handle);
            }
            Err(e) => {
                // Non-fatal: log and let the bundler's own resolver fall
                // through to the on-disk slot, which still produces a useful
                // error message pointing at `crates/zfb/binaries/esbuild/`.
                crate::output::warn(format!(
                    "could not extract embedded esbuild ({e}); \
                     falling back to bundler resolver"
                ));
                _embedded_esbuild_handle = None;
            }
        }
    } else {
        _embedded_esbuild_handle = None;
    }
    // CSS Modules — compute the scoped class-name maps and hand them
    // to the bundler so `import styles from "./x.module.css"` resolves
    // to the scoped class strings at bundle time. The scoped CSS bytes
    // themselves are emitted later by the CSS asset emitter (step 2.5)
    // into `dist/assets/styles-<hash>.css`; both sides run
    // `CssModulesProcessor` with the default config so the scoped
    // names match. Empty for projects with no `.module.css` files.
    bundler_input.css_module_class_maps =
        compute_css_module_class_maps(project_root, config)
            .context("CSS Modules class-map computation failed")?;

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
    // The running host is reused for SSG rendering in step 3 (via
    // `Backend::Existing`) so we only pay the host startup cost once.
    // `_worker_handle` deliberately keeps the host alive through the
    // subsequent `render_all` call: dropping it earlier would shut the host
    // down before rendering completes. The `_` prefix suppresses
    // the unused-variable warning without triggering immediate drop
    // (only `_` alone drops immediately; `_name` lives to end of scope).
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
    let mut prod_asset_inputs = runner
        .emit_prod_assets(project_root, outdir, config)
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
    if prod_asset_inputs.css.is_some() || prod_asset_inputs.islands.is_some() {
        let prod_pages = build_prod_rendered_files(
            outdir,
            &route_universe_for_rewrite,
            &post_processable_pages,
        );
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
    // adapter. Rationale: deploy targets like Cloudflare Pages serve
    // prerendered routes through a static-asset server (ASSETS first,
    // inner worker on 404). SSG route code in the inner worker bundle is
    // dead code on the request path AND counts against the platform's
    // worker-size cap (CF Pages: 3 MiB free / 10 MiB paid). Trimming the
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
        runtime_bundler_input.worker_only_routes =
            Some(ssr_route_keys_for_runtime_bundle);
        runtime_bundler_input.bundle_basename =
            Some("bundle-runtime.mjs".to_string());
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

    // 4. Copy public/ into out_dir (under the base segment when cfg.base is set).
    //
    // Static assets in public/ must land in dist/ so they are served
    // verbatim in production. When the project mounts under a sub-path
    // (cfg.base = "/pj/test/"), files must arrive at
    // <out_dir>/<base-segment>/... so URLs emitted via withBase()
    // resolve under the sub-path mount. Missing public/ is silently
    // ignored — not every project has one.
    copy_public_dir(
        project_root,
        outdir,
        &config.public_dir,
        config.base.as_deref(),
    )
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
    static_expansion_params: &[DynamicResolvedEntry],
    runtime_expansion_params: &[DynamicResolvedEntry],
    prerender_map: &std::collections::BTreeMap<String, bool>,
) -> zfb_build::PostBuildRouteManifest {
    use std::collections::BTreeMap;
    use zfb_build::{PostBuildParamValue, PostBuildRouteEntry, PostBuildRouteManifest};
    use zfb_router::RouteKind;

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
        let source = route
            .source_path
            .strip_prefix(project_root)
            .unwrap_or(&route.source_path)
            .to_string_lossy()
            .into_owned();
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
        let source = dyn_entry
            .source_path
            .strip_prefix(project_root)
            .unwrap_or(&dyn_entry.source_path)
            .to_string_lossy()
            .into_owned();

        // Build the params map only when there are bindings.
        let params = if dyn_entry.params.scalars.is_empty()
            && dyn_entry.params.arrays.is_empty()
        {
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
/// `compilerOptions.paths` map verbatim, suitable for forwarding into
/// [`BundlerInput::tsconfig_paths`]. Used so user-facing alias maps
/// like `"@/*": ["src/*"]` resolve at bundle time without each project
/// having to repeat them in `zfb.config.ts`.
///
/// Resilient by design: missing file, malformed JSON, or absent
/// `paths` field all return an empty map. tsconfig `extends` is NOT
/// followed today — only direct `compilerOptions.paths` are read.
/// Projects with their alias map living in a base tsconfig need to
/// either inline the relevant paths into the project tsconfig or open
/// a follow-up to extend this loader.
pub(crate) fn read_tsconfig_paths(
    project_root: &Path,
) -> std::collections::BTreeMap<String, Vec<String>> {
    let tsconfig_path = project_root.join("tsconfig.json");
    let raw = match std::fs::read_to_string(&tsconfig_path) {
        Ok(s) => s,
        Err(_) => return Default::default(),
    };
    // Strip JSON-with-comments artefacts (tsconfig.json conventionally
    // allows `//` comments and trailing commas — esbuild's tsconfig
    // parser does, and so does TypeScript itself). serde_json does not,
    // so a hand-rolled minimal stripper keeps simple tsconfigs working
    // without pulling in a JSONC parser.
    let cleaned = strip_jsonc(&raw);
    let value: serde_json::Value = match serde_json::from_str(&cleaned) {
        Ok(v) => v,
        Err(_) => return Default::default(),
    };
    let paths = value
        .get("compilerOptions")
        .and_then(|co| co.get("paths"))
        .and_then(|p| p.as_object());
    let Some(paths) = paths else {
        return Default::default();
    };
    let mut out = std::collections::BTreeMap::new();
    for (key, val) in paths {
        if let Some(arr) = val.as_array() {
            // Resolve each target to an absolute path against the
            // project root. The synthetic tsconfig that the bundler
            // writes uses the *shadow tempdir* as `baseUrl`, and the
            // shadow only mirrors `pages/`, `content/`, `components/`,
            // `layouts/` — anything the user aliases at (e.g.
            // `@/* → src/*`) lives outside the shadow tree. Absolute
            // paths bypass `baseUrl` entirely so esbuild resolves
            // them straight against the real project.
            let entries: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| resolve_tsconfig_path_target(project_root, s))
                .collect();
            if !entries.is_empty() {
                out.insert(key.clone(), entries);
            }
        }
    }
    out
}

/// Resolve one tsconfig `paths` target string to an absolute path
/// against the project root, preserving any trailing `/*` glob
/// suffix that esbuild reads as a wildcard.
fn resolve_tsconfig_path_target(project_root: &Path, target: &str) -> String {
    // tsconfig paths can carry a trailing `/*` (e.g. `"src/*"`); split
    // it off so the prefix can be path-joined and the wildcard
    // re-appended verbatim.
    let (prefix, suffix) = match target.rsplit_once("/*") {
        Some((p, "")) => (p, "/*"),
        _ => (target, ""),
    };
    let abs = project_root.join(prefix);
    let mut out = abs.to_string_lossy().into_owned();
    out.push_str(suffix);
    out
}

/// Strip `//` line comments and trailing commas from a JSONC source so
/// it parses with the strict `serde_json` reader. Block comments
/// (`/* … */`) and string-literal awareness are intentionally minimal
/// — sufficient for the conventional shape of `tsconfig.json` files.
fn strip_jsonc(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escape = false;
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if in_string {
            out.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
            continue;
        }
        // Line comment.
        if c == '/' && i + 1 < bytes.len() && bytes[i + 1] as char == '/' {
            while i < bytes.len() && bytes[i] as char != '\n' {
                i += 1;
            }
            continue;
        }
        // Block comment.
        if c == '/' && i + 1 < bytes.len() && bytes[i + 1] as char == '*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] as char == '*' && bytes[i + 1] as char == '/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        out.push(c);
        i += 1;
    }
    // Strip trailing commas — minimal pass: `,` immediately preceding
    // `}` or `]` (whitespace allowed in between).
    let mut stripped = String::with_capacity(out.len());
    let chars: Vec<char> = out.chars().collect();
    let mut j = 0;
    while j < chars.len() {
        let c = chars[j];
        if c == ',' {
            let mut k = j + 1;
            while k < chars.len() && chars[k].is_whitespace() {
                k += 1;
            }
            if k < chars.len() && (chars[k] == '}' || chars[k] == ']') {
                j += 1;
                continue;
            }
        }
        stripped.push(c);
        j += 1;
    }
    stripped
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
    // Mirror the bundler's pipeline shape (theme, strip-md-ext,
    // resolve-links). Every plugin the bundler appends to its
    // `Pipeline::with_defaults_and_theme(...)` MUST also be appended
    // here, otherwise the JSX content_hash diverges and
    // `bridge.get(specifier)` misses on every collection page — dumping
    // the rendered output into a `<pre data-zfb-content-fallback>`
    // block. See zfb#188.
    let snapshot_config = zfb_content::SnapshotPipelineConfig {
        code_highlight_theme: config.code_highlight.as_ref().and_then(|c| c.theme.clone()),
        code_highlight_themes_dir: config
            .code_highlight
            .as_ref()
            .and_then(|c| c.themes_dir.as_ref())
            .map(|td| project_root.join(td)),
        strip_md_ext: config.strip_md_ext,
        resolve_source_map: build_resolve_source_map_for_snapshot(project_root, config),
        gfm_constructs: crate::config::resolve_gfm_constructs(config.markdown.as_ref()),
        toc: config.markdown.as_ref().and_then(|m| m.toc.clone()),
        // Thread `markdown.externalLinks` into the snapshot pipeline.
        // `site` (top-level config.site, #254) lets `ExternalLinksPlugin`
        // classify same-origin absolute URLs as internal.
        external_links: config
            .markdown
            .as_ref()
            .and_then(|m| m.external_links.clone())
            .map(|el| (el.into_content_config(), config.site.clone())),
        cjk_friendly: crate::config::resolve_cjk_friendly(config.markdown.as_ref()),
        // #586 — MUST match `BundlerInput::markdown_features` so the snapshot's
        // JSX `content_hash` stays byte-identical to the bundler's bridge key.
        features: config.markdown.as_ref().and_then(|m| m.features.clone()),
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
    use zfb_content::plugins::util::source_map::{
        build_docs_source_map, DocsSourceMapOptions,
    };
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
    let base_segment = base.map(|b| b.trim_matches('/')).unwrap_or("").to_string();

    let dest_root = if base_segment.is_empty() {
        outdir.to_path_buf()
    } else {
        outdir.join(&base_segment)
    };

    for entry in walkdir::WalkDir::new(&src).into_iter().filter_map(|r| match r {
        Ok(e) => Some(e),
        Err(err) => {
            output::warn(format!(
                "public dir copy: skipping unreadable entry: {err}"
            ));
            None
        }
    }) {
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
    use std::collections::BTreeMap;
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
        mock_bundle_path: PathBuf,
        /// Canned production asset emitter inputs returned from
        /// `emit_prod_assets`. Default = empty (parity with
        /// `DefaultRunner`); tests can preload bytes to exercise the
        /// hash + URL rewrite path.
        prod_asset_inputs: RefCell<ProdAssetEmitterInputs>,
    }

    impl FakeRunner {
        fn new(mock_bundle_path: PathBuf) -> Self {
            Self {
                bundle_calls: RefCell::new(Vec::new()),
                render_calls: RefCell::new(Vec::new()),
                mock_bundle_path,
                prod_asset_inputs: RefCell::new(ProdAssetEmitterInputs::default()),
            }
        }

        /// Preload canned bytes for the production asset emitters.
        /// Used by the orchestrator-wiring tests below.
        fn with_prod_asset_inputs(self, inputs: ProdAssetEmitterInputs) -> Self {
            *self.prod_asset_inputs.borrow_mut() = inputs;
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
                    }],
                },
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
            for entry in &input.route_universe {
                let dest = input.dist_dir.join(&entry.output_path);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                std::fs::write(
                    &dest,
                    format!(
                        "<html><head>{head_extra}</head><body><main>rendered {}</main></body></html>",
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
            self.render_calls.borrow_mut().push(input);
            Ok(RendererOutput {
                ssg_files_written: written,
                static_html_files_written: Vec::new(),
                ssr_manifest: SsrManifest::default(),
                runtime_logs: String::new(),
            })
        }

        fn emit_prod_assets(
            &self,
            _project_root: &Path,
            _outdir: &Path,
            _config: &Config,
        ) -> Result<ProdAssetEmitterInputs> {
            // Clone the canned inputs so multiple tests can share the
            // same FakeRunner without consuming its state.
            let inputs = self.prod_asset_inputs.borrow();
            Ok(ProdAssetEmitterInputs {
                css: inputs.css.clone(),
                islands: inputs.islands.clone(),
            })
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
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
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
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
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
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
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
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
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
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
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
                _outdir: &Path,
                _config: &Config,
            ) -> Result<ProdAssetEmitterInputs> {
                Ok(ProdAssetEmitterInputs::default())
            }
        }
        let tmp = tempdir().unwrap();
        make_runtime(tmp.path());
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        let routes = vec![static_route(vec!["about"], "pages/about.tsx")];
        let err = run_build(BuildArgsResolved {
            project_root: tmp.path(),
            outdir: &tmp.path().join("dist"),
            config: &cfg,
            routes: &routes,
            runner: &FailingRunner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
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
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
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
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
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
        let cfg = Config { adapter: Some("@takazudo/zfb-adapter-cloudflare".into()), ..Config::default() };
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
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
        let cfg = Config { adapter: Some("   ".into()), ..Config::default() };
        let fake_adapter = FakeAdapterRunner::new();
        let err = run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
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
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs")).with_prod_asset_inputs(
            ProdAssetEmitterInputs {
                css: Some(zfb_build::pipeline::AssetEmitterPayload {
                    bytes: b".btn{color:red}".to_vec(),
                    relative_path: PathBuf::from("assets/styles.css"),
                    stable_url: "/assets/styles.css".to_string(),
                }),
                islands: None,
            },
        );
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
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
                }),
                islands: Some(zfb_build::pipeline::AssetEmitterPayload {
                    bytes: b"// js".to_vec(),
                    relative_path: PathBuf::from("assets/islands.js"),
                    stable_url: "/assets/islands.js".to_string(),
                }),
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

        // None slots stay None.
        let mut inputs = ProdAssetEmitterInputs {
            css: None,
            islands: None,
        };
        apply_asset_url_base(&mut inputs, Some("/pj/zudo-doc/"));
        assert!(inputs.css.is_none());
        assert!(inputs.islands.is_none());
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
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs")).with_prod_asset_inputs(
            ProdAssetEmitterInputs {
                css: Some(zfb_build::pipeline::AssetEmitterPayload {
                    bytes: b".btn{color:red}".to_vec(),
                    relative_path: PathBuf::from("assets/styles.css"),
                    // The CSS emitter seeds this from
                    // `zfb_types::STABLE_CSS_URL`; the build path
                    // re-prefixes with `config.base` before handing
                    // it to the renderer.
                    stable_url: "/assets/styles.css".to_string(),
                }),
                islands: None,
            },
        );
        let cfg = Config { base: Some("/pj/zudo-doc/".to_string()), ..Config::default() };
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
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
        let runner = FakeRunner::new(project_root.join(".zfb-build/bundle.mjs")).with_prod_asset_inputs(
            ProdAssetEmitterInputs {
                css: Some(zfb_build::pipeline::AssetEmitterPayload {
                    bytes: b".btn{color:red}".to_vec(),
                    relative_path: PathBuf::from("assets/styles.css"),
                    stable_url: "/assets/styles.css".to_string(),
                }),
                islands: Some(zfb_build::pipeline::AssetEmitterPayload {
                    bytes: b"globalThis.__zfb_islands??=[];".to_vec(),
                    relative_path: PathBuf::from("assets/islands.js"),
                    stable_url: "/assets/islands.js".to_string(),
                }),
            },
        );
        let cfg = Config { base: Some("/pj/zudo-doc/".to_string()), ..Config::default() };
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
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
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
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

    /// Tailwind disabled in config => CSS emitter slot is `None`.
    /// This is the cheap, no-subprocess coverage point for
    /// `DefaultRunner::emit_prod_assets`'s CSS branch.
    #[test]
    fn default_runner_returns_none_css_when_tailwind_disabled_in_config() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        // Stage some sources so the empty-sources branch wouldn't
        // be the reason for None — only `tailwind.enabled = false`
        // should drive the result.
        std::fs::create_dir_all(project_root.join("pages")).unwrap();
        std::fs::write(
            project_root.join("pages/index.tsx"),
            "export default function() { return null }\n",
        )
        .unwrap();
        let cfg = Config { tailwind: Some(crate::config::TailwindConfig { enabled: false }), ..Config::default() };
        let payload = build_default_css_payload(project_root, &project_root.join("dist"), &cfg)
            .expect("should not error");
        assert!(
            payload.is_none(),
            "expected None when tailwind.enabled=false; got {payload:?}",
        );
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
        let payload = build_default_islands_payload(
            project_root,
            &project_root.join("dist"),
            crate::config::Framework::Preact,
            &IslandsPluginConfig::default(),
        )
        .expect("should not error");
        assert!(
            payload.is_none(),
            "expected None when project has no pages/; got {payload:?}",
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
        let payload = build_default_islands_payload(
            project_root,
            &project_root.join("dist"),
            crate::config::Framework::Preact,
            &IslandsPluginConfig::default(),
        )
        .expect("should not error");
        assert!(
            payload.is_none(),
            "expected None when no use-client components; got {payload:?}",
        );
    }

    /// End-to-end check that `DefaultRunner::emit_prod_assets`
    /// invokes the real Tailwind v4 CLI and returns non-empty CSS
    /// bytes for a fixture project with a single page. Mirrors the
    /// `#[ignore]` gate already used by
    /// `crates/zfb-css/tests/integration.rs::subprocess_engine_against_real_binary`
    /// — both depend on the staged Tailwind binary slot at
    /// `crates/zfb/binaries/tailwindcss-v4`, which CI does not yet
    /// populate. Run locally with `--include-ignored` once the slot
    /// is staged.
    // Requires `DefaultRunner` which carries `PluginRegistryHooks` and
    // constructs `Backend::EmbeddedV8` — only available when the
    // `embed_v8` feature is on (issue #371, sub-task 4.1a).
    #[cfg(feature = "embed_v8")]
    #[test]
    #[ignore = "Requires the real tailwindcss v4 binary at crates/zfb/binaries/tailwindcss-v4. \
                Run with --include-ignored once the slot is staged in CI."]
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
        };
        let inputs = runner
            .emit_prod_assets(project_root, &outdir, &cfg)
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

    /// Ignored end-to-end test: runs `cargo run -p zfb -- build` on a
    /// basic-blog project and asserts the post pages, paginated
    /// indexes, and tag pages exist with non-empty `<main>`. Heavy:
    /// shells out to cargo + esbuild + embedded V8. Gated behind
    /// `--ignored` so day-to-day `cargo test` stays fast.
    ///
    /// Status: the renderer call will fail today because the bundler
    /// emits a bundle WITHOUT a `default { fetch }` Worker entry — the
    /// "T7-sibling worker-wrapping sub-task" referenced in the
    /// build-command module docs. The test stays here so once that
    /// sibling lands, flipping the gate is a one-line change.
    /// The standalone demo (https://github.com/Takazudo/zfb-example-blog)
    /// is the intended target once a local checkout is wired in.
    #[test]
    #[ignore = "spawns esbuild + embedded V8; run with --include-ignored once worker wrapping lands"]
    fn end_to_end_basic_blog_build() {
        // Intentionally minimal — the assertions are described in the
        // doc-comment above; the test body is sketched so the
        // follow-up sub-task can wire it without rewriting it from
        // scratch.
        let _ = BTreeMap::<String, bool>::new(); // keep the import live
        eprintln!("[end_to_end_basic_blog_build] gated; see doc-comment.");
    }

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
        let cfg = Config { base: Some("/pj/test/".to_string()), ..Config::default() };
        let fake_adapter = FakeAdapterRunner::new();

        run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
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
        params.insert(
            "slug".into(),
            PostBuildParamValue::Scalar("hello".into()),
        );

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
        let mode = resolve_v8_mode(OutputMode::Auto, &[])
            .expect("auto + no SSR routes resolves");
        assert_eq!(mode, V8Mode::Off);
    }

    /// AC: default `output: "auto"` with `prerender = false` routes
    /// resolves to V8-on — detection-driven.
    #[test]
    fn resolve_v8_mode_auto_with_ssr_routes_is_on() {
        let routes = [ssr_route("/api/me", "/api/me")];
        let mode = resolve_v8_mode(OutputMode::Auto, &routes)
            .expect("auto + SSR route resolves");
        assert_eq!(mode, V8Mode::On);
    }

    /// AC: explicit `output: "hybrid"` on a pure-SSG project forces
    /// V8-on regardless of detection.
    #[test]
    fn resolve_v8_mode_hybrid_on_pure_ssg_forces_on() {
        let mode = resolve_v8_mode(OutputMode::Hybrid, &[])
            .expect("hybrid + no SSR routes resolves");
        assert_eq!(mode, V8Mode::On);
        // Mirror with a project that already has SSR routes — same
        // result, just confirming hybrid is "always on" not "on when
        // SSR routes happen to be present".
        let routes = [ssr_route("/api/me", "/api/me")];
        let mode = resolve_v8_mode(OutputMode::Hybrid, &routes)
            .expect("hybrid + SSR route resolves");
        assert_eq!(mode, V8Mode::On);
    }

    /// Explicit `output: "static"` on a pure-SSG project resolves
    /// cleanly to V8-off. Mirror of the auto+SSG path; the difference
    /// from auto is that the user has declared intent.
    #[test]
    fn resolve_v8_mode_static_on_pure_ssg_is_off() {
        let mode = resolve_v8_mode(OutputMode::Static, &[])
            .expect("static + no SSR routes resolves");
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
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
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
        let cfg = Config { adapter: Some("@takazudo/zfb-adapter-cloudflare".into()), output: OutputMode::Static, ..Config::default() };
        let fake_adapter = FakeAdapterRunner::new();
        let err = run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
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
        let cfg = Config { adapter: Some("@takazudo/zfb-adapter-cloudflare".into()), ..Config::default() };
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
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
        let cfg = Config { adapter: Some("@takazudo/zfb-adapter-cloudflare".into()), ..Config::default() };
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
            plugin_alias_entries: Vec::new(),
            plugin_virtual_modules: Vec::new(),
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
}
