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
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use zfb_build::adapter::{
    ensure_no_ssr_without_adapter, run_adapter_bundle_with, AdapterBundleInput,
    AdapterBundleOutput, AdapterChoice, AdapterRunner, DefaultAdapterRunner, SsrRouteRef,
};
use zfb_build::bundler::{bundle, BundleMode, BundlerInput, BundlerOutput};
use zfb_build::renderer::{render_all, Backend, RendererInput, RendererOutput};
use zfb_router::Router;

use zfb_render::paths::PathsCache;

use crate::cli::BuildArgs;
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

    let router = Router::scan(&pages_dir)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("scanning routes under {}", pages_dir.display()))?;
    let routes = router.routes();

    let pages_built = run_build(BuildArgsResolved {
        project_root: &project_root,
        outdir: &outdir,
        config: &config,
        routes,
        runner: &DefaultRunner,
        adapter_runner: &DefaultAdapterRunner,
    })?;

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
    let bundler_input = BundlerInput {
        project_root: project_root.to_path_buf(),
        pages_dir: PathBuf::from("pages"),
        content_dir: PathBuf::from("content"),
        components_dir: PathBuf::from("components"),
        layouts_dir: PathBuf::from("layouts"),
        framework: cfg_framework_to_render(config.framework),
        define_vars: Default::default(),
        tsconfig_paths: Default::default(),
        external: Vec::new(),
        outdir: outdir.join(".zfb-build"),
        mode: BundleMode::Production,
        minify: false,
        esbuild_binary: None,
        mock_subprocess_output: None,
        content_snapshot_json,
        node_modules_dir: None,
    };
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
    };
    let render_out = runner
        .render_all(renderer_input)
        .context("renderer step failed")?;

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

/// Resolve `outdir` against `project_root`. If `outdir` is absolute it is
/// used as-is; if relative it is joined onto `project_root`.
fn resolve_outdir(project_root: &Path, outdir: &Path) -> PathBuf {
    if outdir.is_absolute() {
        outdir.to_path_buf()
    } else {
        project_root.join(outdir)
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
    }

    impl FakeRunner {
        fn new(mock_bundle_path: PathBuf) -> Self {
            Self {
                bundle_calls: RefCell::new(Vec::new()),
                render_calls: RefCell::new(Vec::new()),
                mock_bundle_path,
            }
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
            // path so callers that inspect `dist/` see real files.
            for entry in &input.route_universe {
                let dest = input.dist_dir.join(&entry.output_path);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                std::fs::write(
                    &dest,
                    format!("<html><body><main>rendered {}</main></body></html>", entry.url_path),
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

    #[test]
    fn resolve_outdir_keeps_absolute_paths() {
        let root = Path::new("/proj");
        let abs = PathBuf::from("/tmp/zfb-out");
        assert_eq!(resolve_outdir(root, &abs), abs);
    }

    #[test]
    fn resolve_outdir_joins_relative_paths_onto_root() {
        let root = Path::new("/proj");
        let rel = PathBuf::from("dist");
        assert_eq!(resolve_outdir(root, &rel), PathBuf::from("/proj/dist"));
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
