//! `zfb build` command — one-shot production build.
//!
//! Contract:
//!   pub async fn run(args: &crate::cli::BuildArgs) -> anyhow::Result<()>
//!
//! `args.outdir` is the production output directory (default `dist`).
//! Resolved relative to the current working directory if not absolute.
//!
//! ## v1 → wave-3 transition
//!
//! Earlier waves emitted a placeholder `<h1>zfb build (v1 stub)</h1>`
//! page per static route. Wave 3 (T7) replaces that path with the real
//! SSG-render pipeline, wiring the wave-2 outputs together:
//!
//! 1. [`zfb_router::Router::scan`] enumerates the route table.
//! 2. [`crate::render_pipeline::build_prerender_map`] reads each TSX
//!    page's `export const prerender = …` flag (T5) so SSR-only routes
//!    skip the build-time render.
//! 3. [`zfb_build::bundle`] (T3) produces the ESM worker bundle for
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
//! ### Known gaps surfaced as warnings (not errors)
//!
//! - **Worker entry wrapping.** The bundler emits a bundle that
//!   exports `routes` + `hydrateIsland`. The renderer expects a Worker
//!   bundle exporting `default { fetch }`; emitting that wrapper is
//!   another T7-sibling sub-task. Until it lands, the renderer's
//!   miniflare boot surfaces a clear workerd error referencing the
//!   missing `default` export, which the CLI propagates verbatim with
//!   the rest of the renderer's diagnostics (sourcemap-projected stack
//!   frames included where applicable).
//!
//! The contract for callers (project-root sanity check, `outdir`
//! handling, `✓ N pages built in X.XXs` summary) is unchanged.

use std::env;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use zfb_build::bundler::{bundle, BundleMode, BundlerInput};
use zfb_build::renderer::{render_all, Backend, RendererInput, RendererOutput};
use zfb_router::Router;

use zfb_render::paths::PathsCache;

use crate::cli::BuildArgs;
use crate::config::Config;
use crate::output;
use crate::render_pipeline::{
    build_prerender_map, build_route_universe, cfg_framework_to_render, check_runtime_installed,
    expand_dynamic_routes, DeferredDynamicRoute, RouteUniversePlan,
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
struct BuildArgsResolved<'a, R: BuildRunner> {
    project_root: &'a Path,
    outdir: &'a Path,
    config: &'a Config,
    routes: &'a [zfb_router::Route],
    runner: &'a R,
}

/// Indirection seam over the heavy wave-2 calls (bundler + renderer).
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
    ) -> Result<zfb_build::bundler::BundlerOutput>;

    /// Run the renderer. Errors surface verbatim — the CLI relies on
    /// the renderer's
    /// [`zfb_build::renderer::RendererError::RenderFailed`] including
    /// the source-mapped user location.
    fn render_all(&self, input: RendererInput) -> Result<RendererOutput>;
}

/// Production runner — straight pass-throughs to the real wave-2 APIs.
struct DefaultRunner;
impl BuildRunner for DefaultRunner {
    fn bundle(
        &self,
        input: BundlerInput,
    ) -> Result<zfb_build::bundler::BundlerOutput> {
        bundle(input)
    }
    fn render_all(&self, input: RendererInput) -> Result<RendererOutput> {
        render_all(input).map_err(anyhow::Error::from)
    }
}

/// Drive the build for a fully-resolved input set. Returns the number
/// of pages written.
fn run_build<R: BuildRunner>(args: BuildArgsResolved<'_, R>) -> Result<usize> {
    let BuildArgsResolved {
        project_root,
        outdir,
        config,
        routes,
        runner,
    } = args;

    // Build the renderer-shaped views of the route table.
    let RouteUniversePlan {
        mut static_routes,
        deferred_dynamic,
    } = build_route_universe(routes);
    let prerender_map = build_prerender_map(routes, project_root, |msg| output::warn(msg));

    // Try static `paths()` expansion for every dynamic route. Resolved
    // entries fold into the same `route_universe` as the static routes;
    // entries that couldn't be statically expanded are surfaced as
    // warnings (per-page reason) and skipped — a follow-up adds runtime
    // evaluation for those.
    let mut paths_cache = PathsCache::new();
    let expansion = expand_dynamic_routes(&deferred_dynamic, project_root, &mut paths_cache);
    let dynamic_resolved_count = expansion.resolved.len();
    static_routes.extend(expansion.resolved);
    warn_deferred_dynamic(&expansion.deferred);

    if static_routes.is_empty() {
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
    let _ = dynamic_resolved_count; // (kept for future build-summary use)

    // Fail fast if the runtime npm package isn't on disk — miniflare
    // will fail later anyway, but we can give the user an actionable
    // hint right at build start.
    check_runtime_installed(project_root)?;

    // 1. Bundle.
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
    };
    let bundler_out = runner
        .bundle(bundler_input)
        .context("bundler step failed")?;

    // 2. Render.
    let renderer_input = RendererInput {
        bundle_path: bundler_out.bundle_path.clone(),
        sourcemap_path: bundler_out.sourcemap_path.clone(),
        manifest: bundler_out.manifest.clone(),
        dist_dir: outdir.to_path_buf(),
        route_universe: static_routes,
        prerender_map,
        backend: Backend::SpawnMiniflare,
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
        let pages = run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
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
        let pages = run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
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
        let pages = run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
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
    fn run_build_with_no_static_routes_short_circuits_without_renderer_call() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path();
        let outdir = project_root.join("dist");
        make_runtime(project_root);
        let routes = vec![dynamic_route("slug", "pages/[slug].tsx")];
        let runner = FakeRunner::new(outdir.join(".zfb-build/bundle.mjs"));
        let cfg = Config::default();
        let pages = run_build(BuildArgsResolved {
            project_root,
            outdir: &outdir,
            config: &cfg,
            routes: &routes,
            runner: &runner,
        })
        .unwrap();
        assert_eq!(pages, 0);
        assert!(runner.bundle_calls.borrow().is_empty());
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
            fn render_all(&self, _input: RendererInput) -> Result<RendererOutput> {
                Err(anyhow!("renderer crashed at pages/error.tsx:5:3"))
            }
        }
        let tmp = tempdir().unwrap();
        make_runtime(tmp.path());
        let cfg = Config::default();
        let routes = vec![static_route(vec!["about"], "pages/about.tsx")];
        let err = run_build(BuildArgsResolved {
            project_root: tmp.path(),
            outdir: &tmp.path().join("dist"),
            config: &cfg,
            routes: &routes,
            runner: &FailingRunner,
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
        let routes = vec![static_route(vec!["about"], "pages/about.tsx")];
        let runner = FakeRunner::new(tmp.path().join("dist/.zfb-build/bundle.mjs"));
        let err = run_build(BuildArgsResolved {
            project_root: tmp.path(),
            outdir: &tmp.path().join("dist"),
            config: &cfg,
            routes: &routes,
            runner: &runner,
        })
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("@takazudo/zfb-runtime"), "{msg}");
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
