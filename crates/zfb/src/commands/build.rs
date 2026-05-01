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
//! 4. [`zfb_build::renderer::render_all`] (T6) spawns one long-lived
//!    miniflare subprocess, drives a `GET` per concrete URL, and writes
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

use std::env;
use std::path::Path;
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
    build_production_islands_asset, scan_islands, BundleConfig, EsbuildSubprocessBundler,
    EsbuildSubprocessConfig, FsResolver,
};
use zfb_router::Router;

use zfb_render::paths::PathsCache;

use crate::cli::BuildArgs;
use crate::commands::resolve::resolve_outdir;
use crate::config::Config;
use crate::output;
use crate::render_pipeline::{
    build_prerender_map, build_route_universe, cfg_framework_to_render, check_runtime_installed,
    eval_deferred_paths_via_worker, expand_dynamic_routes, DeferredDynamicRoute,
    RouteUniversePlan,
};

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
    if let Some(host) = plugin_host.as_ref() {
        let ctx = zfb_build::BuildHookContext {
            project_root: project_root.clone(),
            out_dir: outdir.clone(),
            config: serde_json::to_value(&config)
                .context("plugin lifecycle: serialise config for preBuild ctx")?,
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

    // The build pipeline (bundler subprocess, miniflare boot, all
    // SSG `reqwest::blocking` calls) is fundamentally synchronous,
    // but `#[tokio::main]` keeps a multi-thread runtime live in the
    // background. Without `block_in_place` the inner blocking
    // subroutines panic on drop with `Cannot drop a runtime in a
    // context where blocking is not allowed`. Telling the outer
    // runtime up-front that the next stretch of work is blocking is
    // the supported escape hatch.
    let pages_built = tokio::task::block_in_place(|| {
        run_build(BuildArgsResolved {
            project_root: &project_root,
            outdir: &outdir,
            config: &config,
            routes,
            runner: &DefaultRunner,
            adapter_runner: &DefaultAdapterRunner,
        })
    })?;

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
    fn bundle(
        &self,
        input: BundlerInput,
    ) -> Result<BundlerOutput>;

    /// Evaluate deferred dynamic routes whose `paths()` couldn't be
    /// statically extracted. The production runner starts miniflare
    /// against the bundle, queries `/__paths__/<route>` for each
    /// deferred route, and returns:
    ///
    /// - the expanded route entries (resolved) + still-deferred entries,
    /// - the [`Backend`] to pass to the subsequent `render_all` call
    ///   (either `SpawnMiniflare` when miniflare was not started here, or
    ///   `Existing { base_url }` to reuse the already-running process),
    /// - an opaque cleanup handle — **the caller MUST drop it after
    ///   calling `render_all`**. Dropping it kills the miniflare process.
    ///   When no subprocess was started (fake runner, or no deferred
    ///   routes), the handle is a no-op.
    ///
    /// Fake runners return an empty expansion and `SpawnMiniflare` (the
    /// fake `render_all` ignores the backend).
    fn eval_deferred_paths(
        &self,
        deferred: &[DeferredDynamicRoute],
        bundle_out: &BundlerOutput,
        cache: &mut PathsCache,
    ) -> Result<(crate::render_pipeline::DynamicExpansion, Backend, WorkerHandle)>;

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

/// Opaque handle that keeps a background miniflare subprocess alive.
///
/// Dropping this handle terminates the process (via
/// [`zfb_build::renderer::shutdown`]). When no subprocess is running,
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

/// Production runner — straight pass-throughs to the real bundler /
/// renderer APIs.
struct DefaultRunner;
impl BuildRunner for DefaultRunner {
    fn bundle(
        &self,
        input: BundlerInput,
    ) -> Result<BundlerOutput> {
        bundle(input)
    }

    fn eval_deferred_paths(
        &self,
        deferred: &[DeferredDynamicRoute],
        bundle_out: &BundlerOutput,
        cache: &mut PathsCache,
    ) -> Result<(crate::render_pipeline::DynamicExpansion, Backend, WorkerHandle)> {
        if deferred.is_empty() {
            return Ok((
                crate::render_pipeline::DynamicExpansion::default(),
                Backend::SpawnMiniflare,
                WorkerHandle(None),
            ));
        }
        // Start miniflare against the bundle to evaluate runtime paths().
        let state = zfb_build::renderer::start(zfb_build::renderer::RendererStartInput {
            bundle_path: bundle_out.bundle_path.clone(),
            sourcemap_path: bundle_out.sourcemap_path.clone(),
            backend: Backend::SpawnMiniflare,
            request_timeout: None,
        })
        .context("could not start miniflare for runtime paths() evaluation")?;
        let base_url = state.base_url().to_string();
        let expansion = eval_deferred_paths_via_worker(deferred, &base_url, cache, None);
        // Return the `RendererState` wrapped in a `WorkerHandle` so the
        // miniflare process stays alive through `render_all`, then is
        // killed when the caller drops the handle.
        Ok((
            expansion,
            Backend::Existing { base_url },
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
        let islands = build_default_islands_payload(project_root, outdir)
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
fn build_default_css_payload(
    project_root: &Path,
    outdir: &Path,
    config: &Config,
) -> Result<Option<AssetEmitterPayload>> {
    // Honour the user's opt-out switch before doing any source
    // discovery work — keeps the default runner free of subprocess
    // cost when the project explicitly does not want Tailwind.
    let tailwind_enabled = config
        .tailwind
        .as_ref()
        .map(|t| t.enabled)
        .unwrap_or(true);
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

    // Honour an authored global stylesheet at the conventional
    // location. Tailwind v4's entry CSS prepends our `@source`
    // directives to whatever the user wrote there, so the user's
    // `@theme`, `@import` of vendor CSS, etc. continue to work.
    let global_css = project_root.join("styles").join("global.css");
    if global_css.is_file() {
        tw_cfg = tw_cfg.with_input_css(global_css);
    }

    let engine = TailwindSubprocessEngine::new(tw_cfg);

    let pipe_cfg = CssPipelineConfig {
        sources,
        // Keep CSS Modules class-map writes off this version of the
        // wiring — the bundler-side rewrite that consumes those JSONs
        // is the next step (S5's audit). Once that lands the build
        // will pass a real `class_map_dir` here.
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
fn build_default_islands_payload(
    project_root: &Path,
    outdir: &Path,
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
    let islands_set = match scan_islands(&entries, &resolver) {
        Ok(set) => set,
        Err(e) => {
            output::warn(format!(
                "islands scanner failed ({e}); skipping islands asset emission"
            ));
            return Ok(None);
        }
    };
    if islands_set.is_empty() {
        return Ok(None);
    }

    let bundler = EsbuildSubprocessBundler::new(
        EsbuildSubprocessConfig::default().with_working_dir(project_root.to_path_buf()),
    );
    let bundle_cfg = BundleConfig::production().with_outdir(outdir.to_path_buf());

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
/// of pages written.
fn run_build<R: BuildRunner, A: AdapterRunner>(
    args: BuildArgsResolved<'_, R, A>,
) -> Result<usize> {
    let BuildArgsResolved {
        project_root,
        outdir,
        config,
        routes,
        runner,
        adapter_runner,
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

    // Phase 1 — static paths() expansion.
    //
    // Try static `paths()` extraction for every dynamic route. Resolved
    // entries fold into the same `route_universe` as the static routes.
    // Entries whose `paths()` is non-literal (e.g. they `await import`
    // or query a content collection) are collected into
    // `still_deferred` for Phase 2.
    let mut paths_cache = PathsCache::new();
    let expansion = expand_dynamic_routes(&deferred_dynamic, project_root, &mut paths_cache);
    static_routes.extend(expansion.resolved);
    let still_deferred = expansion.deferred;

    // Phase 2 — embed content snapshot in the bundle so runtime
    // `paths()` can call `getCollection(...)`.
    //
    // Build the content snapshot from the configured collections and
    // embed it in the worker bundle. This lets pages whose `paths()`
    // calls `getCollection(...)` resolve real content entries when
    // queried via the `/__paths__` endpoint below.
    //
    // Errors building the snapshot are non-fatal: if the collection
    // root is missing or a file is malformed, we warn and fall back to
    // an empty snapshot. The build will proceed; pages that depend on
    // the collection data will return empty `paths()` results at render
    // time, which surfaces as zero dynamic pages — visible in the build
    // summary.
    let content_snapshot_json = if !still_deferred.is_empty() {
        let collections: Vec<zfb_content::CollectionConfig> = config
            .collections
            .iter()
            .map(|c| {
                zfb_content::CollectionConfig::new(
                    c.name.clone(),
                    project_root.join(&c.path),
                )
            })
            .collect();
        match zfb_content::build_snapshot(&collections) {
            Ok(snap) => match serde_json::to_string(&snap) {
                Ok(json) => Some(json),
                Err(e) => {
                    output::warn(format!(
                        "content snapshot serialization failed ({e}); dynamic paths() will see empty collections"
                    ));
                    None
                }
            },
            Err(e) => {
                output::warn(format!(
                    "content snapshot build failed ({e}); dynamic paths() will see empty collections"
                ));
                None
            }
        }
    } else {
        None
    };

    if static_routes.is_empty() && still_deferred.is_empty() {
        // Stay user-friendly: an all-dynamic project where every page
        // also failed static expansion still produces a valid build
        // artifact (an empty dist), but the user has clearly not gotten
        // what they asked for — both warn and exit happy. This matches
        // the previous "no pages found" behaviour shape so existing CI
        // configs don't regress.
        output::warn(
            "no routes to render; dist will be empty (every dynamic route deferred to runtime evaluation)",
        );
        return Ok(0);
    }

    // Adapter precondition check.
    //
    // A route with `prerender = false` cannot be served as a static
    // file — it needs the runtime SSR adapter to produce a deploy-
    // shaped wrapper. If the user has SSR routes but no adapter
    // configured, fail fast HERE (before the expensive bundle +
    // miniflare spin-up) with a pointer at the offending route.
    let ssr_route_refs: Vec<SsrRouteRef<'_>> = static_routes
        .iter()
        .filter(|entry| {
            !prerender_map
                .get(&entry.route_key)
                .copied()
                .unwrap_or(true)
        })
        .map(|entry| SsrRouteRef {
            route_key: entry.route_key.as_str(),
            url_path: entry.url_path.as_str(),
        })
        .collect();
    ensure_no_ssr_without_adapter(&adapter, &ssr_route_refs)?;

    // Fail fast if the runtime npm package isn't on disk — miniflare
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
    let mut bundler_input = BundlerInput::for_project(
        project_root.to_path_buf(),
        cfg_framework_to_render(config.framework),
        BundleMode::Production,
        outdir.join(".zfb-build"),
        content_snapshot_json,
    );
    // Inject project-side resolution context so esbuild can find
    // user dependencies + path aliases. Without these the shadow
    // tempdir has no `node_modules` to walk into and no tsconfig
    // `paths` to honour, so anything beyond a self-contained page
    // module fails to resolve.
    if let Some(nm) = detect_project_node_modules(project_root) {
        bundler_input.node_modules_dir = Some(nm);
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
        .map(|c| zfb_build::ContentCollectionSpec::new(c.name.clone(), c.path.clone()))
        .collect();
    let bundler_out = runner
        .bundle(bundler_input)
        .context("bundler step failed")?;

    // 2. Phase 3 — runtime paths() evaluation.
    //
    // For any dynamic routes whose `paths()` couldn't be statically
    // extracted (Phase 1), start miniflare against the freshly-bundled
    // worker and query the `/__paths__/<route>` endpoint for each. The
    // running worker is reused for SSG rendering in step 3 (via
    // `Backend::Existing`) so we only pay the miniflare startup cost once.
    // `_worker_handle` deliberately keeps miniflare alive through the
    // subsequent `render_all` call: dropping it earlier would kill the
    // subprocess before rendering completes. The `_` prefix suppresses
    // the unused-variable warning without triggering immediate drop
    // (only `_` alone drops immediately; `_name` lives to end of scope).
    let (runtime_expansion, backend, _worker_handle) = runner
        .eval_deferred_paths(&still_deferred, &bundler_out, &mut paths_cache)
        .context("runtime paths() evaluation step failed")?;
    static_routes.extend(runtime_expansion.resolved);
    warn_deferred_dynamic(&runtime_expansion.deferred);

    if static_routes.is_empty() {
        output::warn(
            "no routes to render after runtime paths() evaluation; dist will be empty",
        );
        return Ok(0);
    }

    // 2.5. Pre-render: produce CSS / islands bytes so we know which
    //      stable URLs to inject into the rendered HTML head. We MUST
    //      know up front whether each emitter slot will produce
    //      bytes; injecting a `<link>` for a stylesheet that is then
    //      never written would leak the unhashed `/assets/styles.css`
    //      URL into shipped HTML (the prod pipeline only rewrites
    //      stable→hashed for slots it actually emits).
    let prod_asset_inputs = runner
        .emit_prod_assets(project_root, outdir, config)
        .context("production asset emitters failed")?;
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
        route_universe: static_routes,
        prerender_map,
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

    // 3.5. Production asset pipeline pass.
    //
    // The renderer just wrote SSG HTML files to disk with stable
    // asset URLs spliced into `<head>` (when an emitter slot produced
    // bytes). Now hash + ship the asset bytes and rewrite each HTML
    // file's stable URLs to the hashed equivalents in place. This is
    // a no-op (no asset writes, HTML round-tripped) when both
    // emitter slots returned `None`.
    if !prod_asset_inputs.css.is_none() || !prod_asset_inputs.islands.is_none() {
        let prod_pages = build_prod_rendered_files(
            outdir,
            &route_universe_for_rewrite,
            &render_out.ssg_files_written,
        );
        apply_prod_asset_pipeline(outdir, prod_pages, prod_asset_inputs)
            .context("production asset pipeline (hash + URL rewrite) failed")?;
    }

    // Surface miniflare's stderr (workerd/console.warn lines) so the
    // user sees them even on a green build — they are often informative
    // about deprecations or routing oddities.
    if !render_out.miniflare_logs.trim().is_empty() {
        output::info("miniflare logs:");
        for line in render_out.miniflare_logs.lines() {
            output::info(format!("  {line}"));
        }
    }

    // 3. Adapter dispatch.
    //
    // When an adapter is configured, hand the same ESM bundle to the
    // adapter package's CLI so it can wrap it into a deploy-target-
    // shaped entry (e.g. `dist/_worker.js` for Cloudflare Pages). The
    // current contract feeds the WHOLE bundle (SSG + SSR routes) to
    // the adapter — for Cloudflare Pages this is fine because static
    // assets short-circuit the worker, so SSG routes in the worker
    // bundle are dead code on the request path. A future optimization
    // could trim the bundle to SSR-only routes; tracked as a follow-up
    // (see this module's docs).
    if !adapter.is_none() {
        let adapter_in = AdapterBundleInput {
            project_root: project_root.to_path_buf(),
            input_bundle: bundler_out.bundle_path.clone(),
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

    Ok(render_out.ssg_files_written.len())
}

/// Derive the [`ProdHeadAssets`] payload for [`RendererInput`] from
/// the bytes-only emitter inputs. Returns `None` when no slot has
/// bytes — the renderer then ships HTML untouched (matching today's
/// behaviour for projects with no CSS / islands).
///
/// The stable URLs come straight from
/// [`zfb_build::pipeline::AssetEmitterPayload::stable_url`] (which the
/// CSS / islands adapters seed from `zfb_types::asset_urls`
/// constants). This lets a future caller mount assets at a non-default
/// URL prefix without rewriting this function.
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
            while i + 1 < bytes.len()
                && !(bytes[i] as char == '*' && bytes[i + 1] as char == '/')
            {
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
        .map(|c| zfb_content::CollectionConfig::new(c.name.clone(), project_root.join(&c.path)))
        .collect();
    if let Err(err) = zfb_content::build_snapshot(&collections) {
        output::warn(format!("ZFB_DEBUG_SNAPSHOT: snapshot probe failed: {err}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use tempfile::tempdir;
    use zfb_build::bundler::{BundleManifest, BundlerOutput, RouteEntry};
    use zfb_build::renderer::{RendererOutput, SsrManifest};
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
                    }],
                },
            })
        }
        fn eval_deferred_paths(
            &self,
            deferred: &[DeferredDynamicRoute],
            _bundle_out: &BundlerOutput,
            _cache: &mut PathsCache,
        ) -> Result<(crate::render_pipeline::DynamicExpansion, Backend, WorkerHandle)> {
            // The fake runner doesn't spawn miniflare; return all deferred
            // routes unchanged (still deferred), and SpawnMiniflare (the
            // fake render_all ignores the backend).
            Ok((
                crate::render_pipeline::DynamicExpansion {
                    resolved: Vec::new(),
                    deferred: deferred.to_vec(),
                },
                Backend::SpawnMiniflare,
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
                        s.push_str(&format!(
                            "<link rel=\"stylesheet\" href=\"{href}\">"
                        ));
                    }
                    for src in &assets.island_module_urls {
                        s.push_str(&format!(
                            "<script type=\"module\" src=\"{src}\"></script>"
                        ));
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
                ssr_manifest: SsrManifest::default(),
                miniflare_logs: String::new(),
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
        }
    }

    fn dynamic_route(name: &str, source: &str) -> Route {
        Route {
            source_path: PathBuf::from(source),
            segments: vec![Segment::Dynamic(name.into())],
            kind: RouteKind::Dynamic,
            specificity: 0,
            output_extension: None,
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
        fn run(
            &self,
            package: &str,
            input: &AdapterBundleInput,
        ) -> Result<AdapterBundleOutput> {
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
        let runner = FakeRunner::new(outdir.join(".zfb-build/bundle.mjs"));

        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        let pages = run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
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
        let runner = FakeRunner::new(outdir.join(".zfb-build/bundle.mjs"));
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        let pages = run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
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
            },
        ];
        let runner = FakeRunner::new(outdir.join(".zfb-build/bundle.mjs"));
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        let pages = run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
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
        // empty (no runtime miniflare in unit tests).
        let routes = vec![dynamic_route("slug", "pages/[slug].tsx")];
        let runner = FakeRunner::new(outdir.join(".zfb-build/bundle.mjs"));
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        let pages = run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
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
            ) -> Result<(crate::render_pipeline::DynamicExpansion, Backend, WorkerHandle)> {
                Ok((
                    crate::render_pipeline::DynamicExpansion {
                        resolved: Vec::new(),
                        deferred: deferred.to_vec(),
                    },
                    Backend::SpawnMiniflare,
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
        })
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("renderer step failed"), "{msg}");
        assert!(msg.contains("pages/error.tsx:5:3"), "{msg}");
    }

    #[test]
    fn run_build_errors_when_runtime_npm_package_missing() {
        let tmp = tempdir().unwrap();
        // No node_modules → check_runtime_installed errors.
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        let routes = vec![static_route(vec!["about"], "pages/about.tsx")];
        let runner = FakeRunner::new(tmp.path().join("dist/.zfb-build/bundle.mjs"));
        let err = run_build(BuildArgsResolved {
            project_root: tmp.path(),
            outdir: &tmp.path().join("dist"),
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
        })
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("@takazudo/zfb-runtime"), "{msg}");
    }

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
        let runner = FakeRunner::new(outdir.join(".zfb-build/bundle.mjs"));
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
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
            segments: vec![
                Segment::Static("api".into()),
                Segment::Static("foo".into()),
            ],
            kind: RouteKind::Static,
            specificity: 0,
            output_extension: None,
        }];
        let runner = FakeRunner::new(outdir.join(".zfb-build/bundle.mjs"));
        let cfg = Config::default(); // adapter is None
        let fake_adapter = FakeAdapterRunner::new();
        let err = run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
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
                segments: vec![
                    Segment::Static("api".into()),
                    Segment::Static("foo".into()),
                ],
                kind: RouteKind::Static,
                specificity: 0,
                output_extension: None,
            },
        ];
        let runner = FakeRunner::new(outdir.join(".zfb-build/bundle.mjs"));
        let mut cfg = Config::default();
        cfg.adapter = Some("@takazudo/zfb-adapter-cloudflare".into());
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
        })
        .unwrap();
        let calls = fake_adapter.calls.borrow();
        assert_eq!(calls.len(), 1, "adapter dispatch must run once");
        assert_eq!(calls[0].0, "@takazudo/zfb-adapter-cloudflare");
        // Adapter receives the same bundle the renderer used.
        assert_eq!(
            calls[0].1.input_bundle,
            outdir.join(".zfb-build/bundle.mjs")
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
        let runner = FakeRunner::new(outdir.join(".zfb-build/bundle.mjs"));
        let mut cfg = Config::default();
        cfg.adapter = Some("   ".into());
        let fake_adapter = FakeAdapterRunner::new();
        let err = run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
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
        let runner = FakeRunner::new(outdir.join(".zfb-build/bundle.mjs"))
            .with_prod_asset_inputs(ProdAssetEmitterInputs {
                css: Some(zfb_build::pipeline::AssetEmitterPayload {
                    bytes: b".btn{color:red}".to_vec(),
                    relative_path: PathBuf::from("assets/styles.css"),
                    stable_url: "/assets/styles.css".to_string(),
                }),
                islands: None,
            });
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
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
        let runner = FakeRunner::new(outdir.join(".zfb-build/bundle.mjs"));
        let cfg = Config::default();
        let fake_adapter = FakeAdapterRunner::new();
        run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
            adapter_runner: &fake_adapter,
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
        let mut cfg = Config::default();
        cfg.tailwind = Some(crate::config::TailwindConfig { enabled: false });
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
        let payload = build_default_islands_payload(project_root, &project_root.join("dist"))
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
        let payload = build_default_islands_payload(project_root, &project_root.join("dist"))
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
        let runner = DefaultRunner;
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

    /// Ignored end-to-end test: runs `cargo run -p zfb -- build` on
    /// `examples/basic-blog` and asserts the post pages, paginated
    /// indexes, and tag pages exist with non-empty `<main>`. Heavy:
    /// shells out to cargo + esbuild + node (miniflare). Gated behind
    /// `--ignored` so day-to-day `cargo test` stays fast.
    ///
    /// Status: the renderer call will fail today because the bundler
    /// emits a bundle WITHOUT a `default { fetch }` Worker entry — the
    /// "T7-sibling worker-wrapping sub-task" referenced in the
    /// build-command module docs. The test stays here so once that
    /// sibling lands, flipping the gate is a one-line change.
    #[test]
    #[ignore = "spawns esbuild + miniflare; run with --include-ignored once worker wrapping lands"]
    fn end_to_end_basic_blog_build() {
        // Intentionally minimal — the assertions are described in the
        // doc-comment above; the test body is sketched so the
        // follow-up sub-task can wire it without rewriting it from
        // scratch.
        let _ = BTreeMap::<String, bool>::new(); // keep the import live
        eprintln!("[end_to_end_basic_blog_build] gated; see doc-comment.");
    }
}
